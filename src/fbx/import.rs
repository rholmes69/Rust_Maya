//! FBX 7.x importer (binary and ASCII): models and hierarchy (with pre/post rotation, pivots,
//! rotation orders and geometric transforms), polygon meshes (UVs, hard edges from smoothing
//! groups or authored normals, per-face materials split into sub-meshes), materials and
//! textures (external or embedded), skin clusters, blend shapes, cameras, lights and animation
//! stacks. Units and up axis follow the file's GlobalSettings (converted to metres, Y up).

use super::{object_name, Doc, Node, Prop, TICKS_PER_SECOND};
use crate::app::App;
use crate::look::{CameraShape, Light, LightKind, MatKind};
use crate::mesh::Mesh;
use crate::rig::{BlendShape, Skin};
use crate::scene::{Channel, Curve, Key, KeyTan, NodeId, Shape, TanKind, Tangent};
use glam::{DMat4, DQuat, DVec3, Mat4, Vec2, Vec3};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Default)]
pub struct Report {
    pub nodes: usize,
    pub meshes: usize,
    pub skins: usize,
    pub morphs: usize,
    pub anim_curves: usize,
    pub materials: usize,
    pub textures: usize,
    pub lights: usize,
    pub cameras: usize,
    pub notes: Vec<String>,
    pub roots: Vec<NodeId>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Imported FBX: {} nodes, {} meshes, {} materials, {} textures, {} skins, {} blend-shape targets, {} animation curves, {} lights, {} cameras",
            self.nodes, self.meshes, self.materials, self.textures, self.skins, self.morphs, self.anim_curves, self.lights, self.cameras
        );
        if !self.notes.is_empty() {
            s.push_str(&format!(" — {}", self.notes.join("; ")));
        }
        s
    }
}

/// An FBX object: id, class ("Model", "Geometry", …), subclass ("Mesh", "LimbNode", …), name.
struct Obj<'a> {
    node: &'a Node,
    /// Position in the file's Objects list.
    order: usize,
    class: String,
    sub: String,
    name: String,
}

struct Conn {
    child: i64,
    parent: i64,
    prop: Option<String>,
}

struct Scene<'a> {
    objs: HashMap<i64, Obj<'a>>,
    conns: Vec<Conn>,
}

impl<'a> Scene<'a> {
    fn children(&self, parent: i64) -> impl Iterator<Item = (&Obj<'a>, i64, Option<&str>)> {
        self.conns.iter().filter(move |c| c.parent == parent).filter_map(|c| self.objs.get(&c.child).map(|o| (o, c.child, c.prop.as_deref())))
    }
    fn parents(&self, child: i64) -> impl Iterator<Item = (Option<&Obj<'a>>, i64, Option<&str>)> {
        self.conns.iter().filter(move |c| c.child == child).map(|c| (self.objs.get(&c.parent), c.parent, c.prop.as_deref()))
    }
    fn child_of_class(&self, parent: i64, class: &str, sub: Option<&str>) -> Option<(i64, &Obj<'a>)> {
        self.children(parent).find(|(o, _, _)| o.class == class && sub.is_none_or(|s| o.sub == s)).map(|(o, id, _)| (id, o))
    }
}

fn v3(a: [f64; 3]) -> DVec3 {
    DVec3::new(a[0], a[1], a[2])
}

/// Euler rotation (degrees) in an FBX rotation order (0 = XYZ: X applied first).
fn euler(deg: DVec3, order: i64) -> DQuat {
    let (x, y, z) = (DQuat::from_rotation_x(deg.x.to_radians()), DQuat::from_rotation_y(deg.y.to_radians()), DQuat::from_rotation_z(deg.z.to_radians()));
    match order {
        1 => y * z * x, // XZY
        2 => x * z * y, // YZX
        3 => z * x * y, // YXZ
        4 => y * x * z, // ZXY
        5 => x * y * z, // ZYX
        _ => z * y * x, // XYZ (and spheric)
    }
}

/// A model's transform properties (translations already in scene units).
#[derive(Clone, Debug)]
struct Xf {
    t: DVec3,
    r: DVec3,
    s: DVec3,
    pre: DVec3,
    post: DVec3,
    roff: DVec3,
    rpiv: DVec3,
    soff: DVec3,
    spiv: DVec3,
    order: i64,
}

impl Xf {
    fn read(n: &Node, unit: f64) -> Xf {
        let g = |k: &str, d: f64| n.p70_vec3(k).map(v3).unwrap_or(DVec3::splat(d));
        Xf {
            t: g("Lcl Translation", 0.0) * unit,
            r: g("Lcl Rotation", 0.0),
            s: g("Lcl Scaling", 1.0),
            pre: g("PreRotation", 0.0),
            post: g("PostRotation", 0.0),
            roff: g("RotationOffset", 0.0) * unit,
            rpiv: g("RotationPivot", 0.0) * unit,
            soff: g("ScalingOffset", 0.0) * unit,
            spiv: g("ScalingPivot", 0.0) * unit,
            order: n.p70_f64("RotationOrder").unwrap_or(0.0) as i64,
        }
    }
    /// Can it map straight onto translate / rotate / scale / joint orient?
    fn simple(&self) -> bool {
        let z = |v: DVec3| v.length_squared() < 1e-12;
        z(self.post) && z(self.roff) && z(self.rpiv) && z(self.soff) && z(self.spiv) && self.order == 0
    }
    fn matrix(&self) -> DMat4 {
        let tr = DMat4::from_translation;
        let ro = |e: DVec3| DMat4::from_quat(euler(e, 0));
        tr(self.t)
            * tr(self.roff)
            * tr(self.rpiv)
            * ro(self.pre)
            * DMat4::from_quat(euler(self.r, self.order))
            * ro(self.post).inverse()
            * tr(-self.rpiv)
            * tr(self.soff)
            * tr(self.spiv)
            * DMat4::from_scale(self.s)
            * tr(-self.spiv)
    }
}

fn mat16(v: &[f64], unit: f64) -> Option<DMat4> {
    if v.len() < 16 {
        return None;
    }
    let mut m = DMat4::from_cols_slice(&v[..16]);
    m.w_axis.x *= unit;
    m.w_axis.y *= unit;
    m.w_axis.z *= unit;
    Some(m)
}

fn to_f32(m: DMat4) -> Mat4 {
    m.as_mat4()
}

fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) { format!("n_{s}") } else { s }
}

/// A layer element (UVs, normals, …) mapped onto polygon corners.
struct Layer {
    mapping: u8,
    index: Option<Vec<i64>>,
    data: Vec<f64>,
    width: usize,
}

impl Layer {
    fn new(le: &Node, data_name: &str, index_name: &str, width: usize) -> Layer {
        let mapping = match le.str_of("MappingInformationType").unwrap_or("ByPolygonVertex") {
            "ByPolygonVertex" => 0,
            "ByVertex" | "ByVertice" | "ByControlPoint" => 1,
            "ByPolygon" => 2,
            "AllSame" => 3,
            _ => 4,
        };
        let reference = le.str_of("ReferenceInformationType").unwrap_or("Direct");
        let index = (reference == "IndexToDirect" || reference == "Index").then(|| le.array_i64(index_name));
        Layer { mapping, index, data: le.array_f64(data_name), width }
    }
    /// The element for a corner (`corner` = running corner index, `v` = control point, `f` = polygon).
    fn get(&self, corner: usize, v: usize, f: usize) -> Option<&[f64]> {
        let i = match self.mapping {
            0 => corner,
            1 => v,
            2 => f,
            3 => 0,
            _ => return None,
        };
        let k = match &self.index {
            Some(idx) => usize::try_from(*idx.get(i)?).ok()?,
            None => i,
        };
        self.data.get(k * self.width..(k + 1) * self.width)
    }
}

/// Evaluate an FBX animation curve at `t` seconds (constant / linear; cubic keys are
/// interpolated linearly, which is exact for the per-frame keys most exporters write).
#[derive(Clone, Debug)]
struct FCurve {
    times: Vec<f64>,
    values: Vec<f64>,
    /// Per key: 0 = constant, 1 = linear, 2 = cubic.
    interp: Vec<u8>,
}

impl FCurve {
    fn read(n: &Node) -> FCurve {
        let times: Vec<f64> = n.array_i64("KeyTime").into_iter().map(|t| t as f64 / TICKS_PER_SECOND).collect();
        let values = n.array_f64("KeyValueFloat");
        let flags = n.array_i64("KeyAttrFlags");
        let refs = n.array_i64("KeyAttrRefCount");
        let mut interp = Vec::with_capacity(times.len());
        for (f, c) in flags.iter().zip(refs.iter().chain(std::iter::repeat(&(times.len() as i64)))) {
            let m = if f & 0x2 != 0 { 0 } else if f & 0x4 != 0 { 1 } else { 2 };
            for _ in 0..*c {
                interp.push(m);
            }
        }
        interp.resize(times.len(), 2);
        let n = times.len().min(values.len());
        FCurve { times: times[..n].to_vec(), values: values[..n].to_vec(), interp }
    }
    fn eval(&self, t: f64) -> Option<f64> {
        let n = self.times.len();
        if n == 0 {
            return None;
        }
        if t <= self.times[0] {
            return Some(self.values[0]);
        }
        if t >= self.times[n - 1] {
            return Some(self.values[n - 1]);
        }
        let i = self.times.partition_point(|x| *x <= t).saturating_sub(1);
        let (t0, t1) = (self.times[i], self.times[i + 1]);
        if self.interp[i] == 0 || t1 <= t0 {
            return Some(self.values[i]);
        }
        let k = (t - t0) / (t1 - t0);
        Some(self.values[i] + (self.values[i + 1] - self.values[i]) * k)
    }
}

pub fn import(app: &mut App, path: &Path, take: Option<&str>) -> Result<Report, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = super::parse(&bytes)?;
    import_doc(app, &doc, path, take)
}

pub fn import_doc(app: &mut App, doc: &Doc, path: &Path, take: Option<&str>) -> Result<Report, String> {
    if doc.version != 0 && doc.version < 7000 {
        return Err(format!("FBX {}.{} files (before FBX 2011) aren't supported: re-save as FBX 2011 or newer", doc.version / 1000, doc.version % 1000 / 100));
    }
    let objects = doc.get("Objects").ok_or("FBX: no Objects section")?;
    let mut rep = Report::default();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("fbx".into());

    // ---- global settings: units and axes
    let gs = doc.get("GlobalSettings");
    let unit = gs.and_then(|g| g.p70_f64("UnitScaleFactor")).unwrap_or(1.0) / 100.0;
    let axis = |k: &str, d: f64| gs.and_then(|g| g.p70_f64(k)).unwrap_or(d) as i64;
    let (up, up_s, front, front_s, coord, coord_s) = (axis("UpAxis", 1.0), axis("UpAxisSign", 1.0), axis("FrontAxis", 2.0), axis("FrontAxisSign", 1.0), axis("CoordAxis", 0.0), axis("CoordAxisSign", 1.0));
    let e = |i: i64| match i {
        0 => DVec3::X,
        1 => DVec3::Y,
        _ => DVec3::Z,
    };
    // Rows of the file → scene axis change: scene x = coord, y = up, z = front.
    let a = glam::DMat3::from_cols(e(coord) * coord_s as f64, e(up) * up_s as f64, e(front) * front_s as f64).transpose();
    let axis_fix = (a - glam::DMat3::IDENTITY).to_cols_array().iter().any(|x| x.abs() > 1e-9);

    // ---- objects and connections
    let mut sc = Scene { objs: HashMap::new(), conns: Vec::new() };
    for (order, n) in objects.children.iter().enumerate() {
        let Some(id) = n.props.first().and_then(|p| p.as_i64()) else { continue };
        let raw = n.props.get(1).and_then(|p| p.as_str()).unwrap_or("");
        let sub = n.props.get(2).and_then(|p| p.as_str()).unwrap_or("").to_string();
        sc.objs.insert(id, Obj { node: n, order, class: n.name.clone(), sub, name: object_name(raw) });
    }
    if let Some(c) = doc.get("Connections") {
        for n in &c.children {
            let kind = n.props.first().and_then(|p| p.as_str()).unwrap_or("");
            let (Some(child), Some(parent)) = (n.props.get(1).and_then(|p| p.as_i64()), n.props.get(2).and_then(|p| p.as_i64())) else { continue };
            let prop = if kind == "OP" { n.props.get(3).and_then(|p| p.as_str()).map(String::from) } else { None };
            sc.conns.push(Conn { child, parent, prop });
        }
    }

    app.begin_batch();
    app.checkpoint();

    // ---- root (axis conversion)
    let root = if axis_fix {
        let g = app.scene.add_node(&format!("{}_fbx", clean(&stem)), Shape::Group, None);
        app.apply_name(g, Some(&format!("{}_fbx", clean(&stem))));
        let q = DQuat::from_mat3(&a);
        let (sx, sy, sz) = (a.x_axis.length(), a.y_axis.length(), a.z_axis.length());
        let det = a.determinant();
        if let Some(n) = app.scene.node_mut(g) {
            if det > 0.0 {
                n.rotate = crate::rig::quat_euler(q.as_quat());
            } else {
                // Mirrored axis system: rotate + flip X.
                let m = a * glam::DMat3::from_diagonal(DVec3::new(-1.0, 1.0, 1.0));
                n.rotate = crate::rig::quat_euler(DQuat::from_mat3(&m).as_quat());
                n.scale = Vec3::new(-1.0, 1.0, 1.0);
            }
            let _ = (sx, sy, sz);
        }
        rep.notes.push("converted to Y-up".into());
        Some(g)
    } else {
        None
    };

    // ---- models
    let mut model_ids: Vec<i64> = sc.objs.iter().filter(|(_, o)| o.class == "Model").map(|(id, _)| *id).collect();
    model_ids.sort_by_key(|id| sc.objs[id].order);
    let mut node_of: HashMap<i64, NodeId> = HashMap::new();
    let mut xf_of: HashMap<i64, Xf> = HashMap::new();
    let mut joints: Vec<NodeId> = Vec::new();
    for &mid in &model_ids {
        let o = &sc.objs[&mid];
        let is_joint = matches!(o.sub.as_str(), "LimbNode" | "Limb" | "Root") || sc.child_of_class(mid, "NodeAttribute", Some("LimbNode")).is_some();
        let shape = if is_joint { Shape::Joint { radius: 0.05 } } else { Shape::Group };
        let name = clean(if o.name.is_empty() { "node" } else { &o.name });
        let id = app.scene.add_node(&name, shape, None);
        app.apply_name(id, Some(&name));
        let xf = Xf::read(o.node, unit);
        if let Some(n) = app.scene.node_mut(id) {
            if xf.simple() {
                n.translate = xf.t.as_vec3();
                n.rotate = xf.r.as_vec3();
                n.scale = xf.s.as_vec3();
                n.joint_orient = xf.pre.as_vec3();
            } else {
                let (s, r, t) = xf.matrix().to_scale_rotation_translation();
                n.translate = t.as_vec3();
                n.rotate = crate::rig::quat_euler(r.as_quat());
                n.scale = s.as_vec3();
            }
            if o.node.p70_f64("Visibility").is_some_and(|v| v < 0.5) {
                n.visible = false;
            }
        }
        if is_joint {
            joints.push(id);
        }
        node_of.insert(mid, id);
        xf_of.insert(mid, xf);
    }
    // Parents.
    for &mid in &model_ids {
        let parent = sc.parents(mid).find_map(|(o, pid, _)| o.filter(|o| o.class == "Model").map(|_| pid));
        let p = parent.and_then(|p| node_of.get(&p).copied()).or(root);
        if let Some(n) = app.scene.node_mut(node_of[&mid]) {
            n.parent = p;
        }
    }
    rep.nodes = model_ids.len();
    if joints.len() > 1 {
        let jp: Vec<Vec3> = joints.iter().map(|j| crate::rig::world_pos(&app.scene, *j)).collect();
        let (lo, hi) = jp.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let r = ((hi - lo).length() * 0.012).max(1e-3);
        for j in &joints {
            if let Some(Shape::Joint { radius }) = app.scene.node_mut(*j).map(|n| &mut n.shape) {
                *radius = r;
            }
        }
    }

    // ---- materials and textures
    let fbm = dir.join(format!("{stem}.fbm"));
    let tex_path_of = |tid: i64, rep: &mut Report| -> Option<String> {
        let t = sc.objs.get(&tid)?;
        let rel = t.node.str_of("RelativeFilename").unwrap_or("").replace('\\', "/");
        let abs = t.node.str_of("FileName").unwrap_or("").replace('\\', "/");
        let file_name = Path::new(if abs.is_empty() { &rel } else { &abs }).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        for cand in [dir.join(&rel), std::path::PathBuf::from(&abs), dir.join(&file_name), fbm.join(&file_name), dir.join("textures").join(&file_name)] {
            if !cand.as_os_str().is_empty() && cand.is_file() {
                return Some(cand.display().to_string());
            }
        }
        // Embedded in a Video object.
        let video = sc.children(tid).find(|(o, _, _)| o.class == "Video").map(|(o, _, _)| o)?;
        let content = video.node.child("Content")?;
        let bytes: Vec<u8> = match content.props.first()? {
            Prop::Raw(b) => b.clone(),
            // ASCII files: base64 text, possibly split into several strings.
            _ => {
                let text: String = content.props.iter().filter_map(|p| p.as_str()).collect();
                crate::importer::gltf::base64_decode(text.trim()).ok()?
            }
        };
        if bytes.is_empty() {
            return None;
        }
        let _ = std::fs::create_dir_all(&fbm);
        let name = if file_name.is_empty() { format!("{}.png", clean(&video.name)) } else { file_name };
        let out = fbm.join(name);
        std::fs::write(&out, &bytes).ok()?;
        rep.notes.push(format!("extracted {}", out.display()));
        Some(out.display().to_string())
    };
    let mut mat_of: HashMap<i64, u32> = HashMap::new();
    for (&mid, o) in sc.objs.iter().filter(|(_, o)| o.class == "Material") {
        let n = o.node;
        let id = app.create_material(MatKind::StandardSurface, Some(&clean(&o.name)));
        let mut m = app.material(id).unwrap().clone();
        let col = n.p70_vec3("DiffuseColor").or_else(|| n.p70_vec3("Diffuse")).unwrap_or([0.8, 0.8, 0.8]);
        let fac = n.p70_f64("DiffuseFactor").unwrap_or(1.0);
        m.color = [(col[0] * fac) as f32, (col[1] * fac) as f32, (col[2] * fac) as f32];
        if let Some(e) = n.p70_vec3("EmissiveColor").or_else(|| n.p70_vec3("Emissive")) {
            let f = n.p70_f64("EmissiveFactor").unwrap_or(1.0);
            m.emission = [(e[0] * f) as f32, (e[1] * f) as f32, (e[2] * f) as f32];
        }
        let transp = match (n.p70_f64("Opacity"), n.p70_f64("TransparencyFactor")) {
            (Some(o), _) if o < 1.0 => 1.0 - o,
            (_, Some(t)) => t,
            _ => 0.0,
        };
        m.transparency = transp.clamp(0.0, 1.0) as f32;
        let shin = n.p70_f64("ShininessExponent").or_else(|| n.p70_f64("Shininess")).unwrap_or(20.0).max(0.0);
        m.roughness = ((2.0 / (shin + 2.0)).sqrt() as f32).clamp(0.05, 1.0);
        m.metalness = 0.0;
        for (to, tid, prop) in sc.children(mid) {
            if to.class != "Texture" {
                continue;
            }
            let Some(p) = tex_path_of(tid, &mut rep) else {
                rep.notes.push(format!("{}: texture '{}' not found", o.name, to.name));
                continue;
            };
            match prop.unwrap_or("") {
                "DiffuseColor" | "Diffuse" | "3dsMax|maps|base_color_map" | "Maya|baseColor" => m.color_map = Some(p),
                "NormalMap" | "Bump" | "3dsMax|maps|norm_map" | "Maya|normalCamera" => m.normal_map = Some(p),
                "EmissiveColor" | "Emissive" => {
                    m.emission_map = Some(p);
                    if m.emission == [0.0; 3] {
                        m.emission = [1.0; 3];
                    }
                }
                _ => continue,
            }
            rep.textures += 1;
        }
        *app.material_mut(id).unwrap() = m;
        mat_of.insert(mid, id);
        rep.materials += 1;
    }

    // ---- geometry
    let mut blend_channel_of: HashMap<i64, (NodeId, usize)> = HashMap::new(); // channel → (node, target)
    for &mid in &model_ids {
        let Some((gid, geom)) = sc.child_of_class(mid, "Geometry", Some("Mesh")) else { continue };
        let g = geom.node;
        let xf = &xf_of[&mid];
        let gt = sc.objs[&mid].node.p70_vec3("GeometricTranslation").map(v3).unwrap_or(DVec3::ZERO) * unit;
        let gr = sc.objs[&mid].node.p70_vec3("GeometricRotation").map(v3).unwrap_or(DVec3::ZERO);
        let gsc = sc.objs[&mid].node.p70_vec3("GeometricScaling").map(v3).unwrap_or(DVec3::ONE);
        let geo_m = DMat4::from_scale_rotation_translation(gsc, euler(gr, 0), gt);
        let _ = xf;
        let verts = g.array_f64("Vertices");
        let positions: Vec<Vec3> = verts.chunks_exact(3).map(|c| geo_m.transform_point3(DVec3::new(c[0], c[1], c[2]) * unit).as_vec3()).collect();
        let pvi = g.array_i64("PolygonVertexIndex");
        // Faces as corner lists (corner index into pvi).
        let mut polys: Vec<Vec<(usize, u32)>> = Vec::new();
        let mut cur: Vec<(usize, u32)> = Vec::new();
        for (ci, &x) in pvi.iter().enumerate() {
            let (v, end) = if x < 0 { ((-x - 1) as u32, true) } else { (x as u32, false) };
            if (v as usize) < positions.len() {
                cur.push((ci, v));
            }
            if end {
                // Drop repeated corners (degenerate polygons exported as "lines").
                let mut poly = std::mem::take(&mut cur);
                poly.dedup_by(|a, b| a.1 == b.1);
                while poly.len() > 1 && poly.first().map(|x| x.1) == poly.last().map(|x| x.1) {
                    poly.pop();
                }
                let unique: std::collections::HashSet<u32> = poly.iter().map(|c| c.1).collect();
                polys.push(if unique.len() >= 3 { poly } else { Vec::new() });
            }
        }
        // UVs (first set), normals, materials, smoothing.
        let uv_le = g.child("LayerElementUV").map(|l| Layer::new(l, "UV", "UVIndex", 2));
        let nrm_le = g.child("LayerElementNormal").map(|l| Layer::new(l, "Normals", "NormalsIndex", 3));
        let mat_le = g.child("LayerElementMaterial");
        let mat_idx = mat_le.map(|l| l.array_i64("Materials")).unwrap_or_default();
        let smooth_le = g.child("LayerElementSmoothing");
        let model_mats: Vec<i64> = sc.children(mid).filter(|(o, _, _)| o.class == "Material").map(|(_, id, _)| id).collect();
        let mut face_mat: Vec<usize> = Vec::with_capacity(polys.len());
        for (f, _) in polys.iter().enumerate() {
            let k = match mat_le.map(|l| l.str_of("MappingInformationType").unwrap_or("AllSame")) {
                Some("ByPolygon") => mat_idx.get(f).copied().unwrap_or(0),
                _ => mat_idx.first().copied().unwrap_or(0),
            };
            face_mat.push(k.max(0) as usize);
        }
        let corner_uv = |corner: usize, v: u32, f: usize| -> Vec2 {
            uv_le.as_ref().and_then(|l| l.get(corner, v as usize, f)).map(|d| Vec2::new(d[0] as f32, d[1] as f32)).unwrap_or(Vec2::ZERO)
        };
        let corner_nrm = |corner: usize, v: u32, f: usize| -> Option<Vec3> {
            nrm_le.as_ref().and_then(|l| l.get(corner, v as usize, f)).map(|d| geo_m.transform_vector3(DVec3::new(d[0], d[1], d[2])).normalize_or_zero().as_vec3())
        };
        // Hard edges: from smoothing (by edge or by polygon groups), else from authored normals.
        let mut hard: Option<BTreeSet<(u32, u32)>> = None;
        let edge_key = |a: u32, b: u32| (a.min(b), a.max(b));
        let mut edge_faces: HashMap<(u32, u32), Vec<(usize, usize)>> = HashMap::new(); // edge → (face, corner position)
        for (f, p) in polys.iter().enumerate() {
            for i in 0..p.len() {
                edge_faces.entry(edge_key(p[i].1, p[(i + 1) % p.len()].1)).or_default().push((f, i));
            }
        }
        if let Some(sl) = smooth_le {
            let sm = sl.array_i64("Smoothing");
            match sl.str_of("MappingInformationType").unwrap_or("") {
                "ByEdge" => {
                    let edges = g.array_i64("Edges");
                    let mut set = BTreeSet::new();
                    // Map polygon-vertex index → its polygon corner to find the edge's end.
                    let mut next_of: HashMap<usize, u32> = HashMap::new();
                    for p in &polys {
                        for i in 0..p.len() {
                            next_of.insert(p[i].0, p[(i + 1) % p.len()].1);
                        }
                    }
                    for (ei, &c) in edges.iter().enumerate() {
                        if sm.get(ei).copied().unwrap_or(1) == 0
                            && let (Some(a), Some(b)) = (pvi.get(c as usize).map(|x| if *x < 0 { (-x - 1) as u32 } else { *x as u32 }), next_of.get(&(c as usize)))
                        {
                            set.insert(edge_key(a, *b));
                        }
                    }
                    hard = Some(set);
                }
                "ByPolygon" => {
                    let set = edge_faces.iter().filter(|(_, fs)| fs.len() == 2 && (sm.get(fs[0].0).copied().unwrap_or(0) & sm.get(fs[1].0).copied().unwrap_or(0)) == 0).map(|(e, _)| *e).collect();
                    hard = Some(set);
                }
                _ => {}
            }
        } else if nrm_le.is_some() {
            let mut set = BTreeSet::new();
            for (e, fs) in &edge_faces {
                if fs.len() != 2 {
                    continue;
                }
                let differs = [e.0, e.1].iter().any(|&v| {
                    let n_at = |(f, _): (usize, usize)| polys[f].iter().find(|c| c.1 == v).and_then(|c| corner_nrm(c.0, v, f));
                    match (n_at(fs[0]), n_at(fs[1])) {
                        (Some(a), Some(b)) => a.dot(b) < 0.999,
                        _ => false,
                    }
                });
                if differs {
                    set.insert(*e);
                }
            }
            hard = Some(set);
        }
        // Split by material when several are used.
        let mut groups: Vec<usize> = face_mat.clone();
        groups.sort_unstable();
        groups.dedup();
        let multi = groups.len() > 1;
        // Skin clusters on this geometry.
        let skin = sc.children(gid).find(|(o, _, _)| o.class == "Deformer" && o.sub == "Skin").map(|(_, id, _)| id);
        let mut clusters: Vec<(NodeId, Vec<(usize, f64)>, Option<DMat4>, Option<DMat4>)> = Vec::new();
        if let Some(sid) = skin {
            for (co, cid, _) in sc.children(sid) {
                if !(co.class == "Deformer" && co.sub == "Cluster") {
                    continue;
                }
                let Some(jm) = sc.children(cid).find(|(o, _, _)| o.class == "Model").map(|(_, id, _)| id).and_then(|m| node_of.get(&m).copied()) else { continue };
                let idx = co.node.array_i64("Indexes");
                let w = co.node.array_f64("Weights");
                let pairs: Vec<(usize, f64)> = idx.iter().zip(w.iter()).map(|(i, w)| (*i as usize, *w)).collect();
                clusters.push((jm, pairs, mat16(&co.node.array_f64("Transform"), unit), mat16(&co.node.array_f64("TransformLink"), unit)));
            }
        }
        // Blend shapes on this geometry: (channel id, name, deltas by control point, default weight).
        let mut channels: Vec<(i64, String, Vec<Vec3>, f32)> = Vec::new();
        for (bo, bid, _) in sc.children(gid) {
            if !(bo.class == "Deformer" && bo.sub == "BlendShape") {
                continue;
            }
            for (ch, chid, _) in sc.children(bid) {
                if ch.sub != "BlendShapeChannel" {
                    continue;
                }
                // The last (full-weight) shape of the channel.
                let Some((shape, _, _)) = sc.children(chid).filter(|(o, _, _)| o.class == "Geometry").last() else { continue };
                let idx = shape.node.array_i64("Indexes");
                let d = shape.node.array_f64("Vertices");
                let mut deltas = vec![Vec3::ZERO; positions.len()];
                for (k, i) in idx.iter().enumerate() {
                    if let (Some(dv), Some(slot)) = (d.get(k * 3..k * 3 + 3), deltas.get_mut(*i as usize)) {
                        *slot = geo_m.transform_vector3(DVec3::new(dv[0], dv[1], dv[2]) * unit).as_vec3();
                    }
                }
                let w = ch.node.p70_f64("DeformPercent").unwrap_or(0.0) as f32 / 100.0;
                let nm = if ch.name.is_empty() { shape.name.clone() } else { ch.name.clone() };
                channels.push((chid, clean(&nm), deltas, w));
            }
        }
        let model_node = node_of[&mid];
        for &gi in &groups {
            // Faces of this material, with vertices renumbered.
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut vmap: Vec<u32> = Vec::new();
            let mut mesh = Mesh::default();
            if !multi {
                // One material: keep the file's vertex order (stable for skins, blend shapes and
                // point caches).
                for (i, p) in positions.iter().enumerate() {
                    remap.insert(i as u32, i as u32);
                    vmap.push(i as u32);
                    mesh.positions.push(*p);
                }
            }
            let mut corners: Vec<Vec<Vec2>> = Vec::new();
            for (f, p) in polys.iter().enumerate() {
                if face_mat[f] != gi || p.len() < 3 {
                    continue;
                }
                let face: Vec<u32> = p
                    .iter()
                    .map(|&(_, v)| {
                        *remap.entry(v).or_insert_with(|| {
                            vmap.push(v);
                            mesh.positions.push(positions[v as usize]);
                            (vmap.len() - 1) as u32
                        })
                    })
                    .collect();
                corners.push(p.iter().map(|&(c, v)| corner_uv(c, v, f)).collect());
                mesh.faces.push(face);
            }
            if mesh.faces.is_empty() {
                continue;
            }
            if uv_le.is_some() {
                mesh.set_corner_uvs(&corners);
            }
            if let Some(h) = &hard {
                mesh.hard_edges = Some(h.iter().filter_map(|(a, b)| Some(edge_key(*remap.get(a)?, *remap.get(b)?))).collect());
            }
            let target = if multi {
                let mname = model_mats.get(gi).and_then(|m| sc.objs.get(m)).map(|o| clean(&o.name)).unwrap_or(format!("mat{gi}"));
                let nm = format!("{}_{mname}", app.scene.node(model_node).map(|n| n.name.clone()).unwrap_or_default());
                let c = app.scene.add_node(&nm, Shape::Group, Some(model_node));
                app.apply_name(c, Some(&nm));
                c
            } else {
                model_node
            };
            let nverts = mesh.positions.len();
            if let Some(n) = app.scene.node_mut(target) {
                n.shape = Shape::Poly { history: None, mesh, rev: crate::scene::next_rev() };
                n.material = model_mats.get(gi).and_then(|m| mat_of.get(m)).copied();
            }
            // Skin (cluster weights remapped to this piece).
            if !clusters.is_empty() {
                let mut weights: Vec<Vec<(u32, f32)>> = vec![Vec::new(); nverts];
                let mut back: HashMap<u32, u32> = HashMap::new();
                for (k, v) in vmap.iter().enumerate() {
                    back.insert(*v, k as u32);
                }
                for (ji, (_, pairs, _, _)) in clusters.iter().enumerate() {
                    for (v, w) in pairs {
                        if let Some(k) = back.get(&(*v as u32))
                            && *w > 1e-6
                        {
                            weights[*k as usize].push((ji as u32, *w as f32));
                        }
                    }
                }
                let mut maxi = 0;
                for w in &mut weights {
                    w.sort_by(|a, b| b.1.total_cmp(&a.1));
                    w.truncate(8);
                    let s: f32 = w.iter().map(|x| x.1).sum::<f32>().max(1e-6);
                    w.iter_mut().for_each(|x| x.1 /= s);
                    maxi = maxi.max(w.len());
                }
                // The cluster "Transform" is stored relative to the bone (Transform = TransformLink⁻¹ ·
                // mesh at bind), so the mesh's own rest transform is the bind shape.
                let mesh_world = app.scene.world_matrix(target).as_dmat4();
                let inv_bind: Vec<Mat4> = clusters.iter().map(|c| to_f32(c.3.map(|m| m.inverse()).unwrap_or_else(|| app.scene.world_matrix(c.0).as_dmat4().inverse()))).collect();
                let jl: Vec<NodeId> = clusters.iter().map(|c| c.0).collect();
                let bind_pose = jl.iter().filter_map(|j| app.scene.node(*j).map(|n| (*j, n.translate, n.rotate, n.joint_orient))).collect();
                app.scene.node_mut(target).unwrap().skin = Some(Skin { joints: jl, inv_bind, bind_shape: to_f32(mesh_world), weights, max_influences: maxi.clamp(1, 8), bind_pose });
                rep.skins += 1;
            }
            // Blend shapes.
            if !channels.is_empty() {
                let mut bs = BlendShape::default();
                for (k, (chid, name, deltas, w)) in channels.iter().enumerate() {
                    bs.add_target(name.clone(), vmap.iter().map(|v| deltas[*v as usize]).collect());
                    bs.weights[k] = *w;
                    blend_channel_of.entry(*chid).or_insert((target, k));
                }
                rep.morphs += bs.targets.len();
                app.scene.node_mut(target).unwrap().blend = Some(bs);
            }
            rep.meshes += 1;
        }
        if multi && let Some(n) = app.scene.node_mut(model_node) {
            n.shape = Shape::Group;
        }
    }

    // ---- cameras and lights (as a shape child, turned to our -Z viewing axis)
    for &mid in &model_ids {
        let Some((_, attr)) = sc.child_of_class(mid, "NodeAttribute", None) else { continue };
        let shape = match attr.sub.as_str() {
            "Camera" => {
                let n = attr.node;
                let focal = n.p70_f64("FocalLength").map(|f| f as f32).or_else(|| n.p70_f64("FieldOfView").map(|fov| 18.0 / (fov.to_radians() / 2.0).tan() as f32)).unwrap_or(35.0);
                rep.cameras += 1;
                Shape::Camera(CameraShape { focal, near: (n.p70_f64("NearPlane").unwrap_or(10.0) * unit) as f32, far: (n.p70_f64("FarPlane").unwrap_or(100000.0) * unit) as f32, ..Default::default() })
            }
            "Light" => {
                let n = attr.node;
                let kind = match n.p70_f64("LightType").unwrap_or(0.0) as i64 {
                    1 => LightKind::Directional,
                    2 => LightKind::Spot,
                    3 => LightKind::Area,
                    _ => LightKind::Point,
                };
                let c = n.p70_vec3("Color").unwrap_or([1.0; 3]);
                let cone = n.p70_f64("OuterAngle").or_else(|| n.p70_f64("Cone angle")).unwrap_or(45.0) as f32;
                let inner = n.p70_f64("InnerAngle").unwrap_or(cone as f64) as f32;
                rep.lights += 1;
                Shape::Light(Light { kind, color: [c[0] as f32, c[1] as f32, c[2] as f32], intensity: (n.p70_f64("Intensity").unwrap_or(100.0) / 100.0) as f32, cone: inner.max(1.0), penumbra: ((cone - inner) * 0.5).max(0.0), shadows: n.p70_f64("CastShadows").is_some_and(|v| v > 0.5) })
            }
            _ => continue,
        };
        let parent = node_of[&mid];
        let nm = format!("{}Shape", app.scene.node(parent).map(|n| n.name.clone()).unwrap_or_default());
        let c = app.scene.add_node(&nm, shape, Some(parent));
        app.apply_name(c, Some(&nm));
        // FBX cameras look down +X and lights down -Y; ours look down -Z.
        let is_light = attr.sub == "Light";
        if let Some(n) = app.scene.node_mut(c) {
            n.rotate = if is_light { Vec3::new(-90.0, 0.0, 0.0) } else { Vec3::new(0.0, -90.0, 0.0) };
        }
    }

    // ---- animation
    let stacks: Vec<(i64, &Obj)> = {
        let mut v: Vec<(i64, &Obj)> = sc.objs.iter().filter(|(_, o)| o.class == "AnimationStack").map(|(id, o)| (*id, o)).collect();
        v.sort_by_key(|x| x.1.order);
        v
    };
    let pick = match take {
        Some(t) => Some(stacks.iter().position(|(_, o)| o.name.eq_ignore_ascii_case(t)).ok_or_else(|| format!("no take named '{t}' (file has: {})", stacks.iter().map(|s| s.1.name.clone()).collect::<Vec<_>>().join(", ")))?),
        None => (!stacks.is_empty()).then_some(0),
    };
    if stacks.len() > 1 {
        let names: Vec<String> = stacks.iter().map(|s| s.1.name.clone()).collect();
        rep.notes.push(format!("{} takes in file, imported '{}' (others: {}; choose with file -import path -take name)", stacks.len(), names[pick.unwrap_or(0)], names.iter().enumerate().filter(|(k, _)| Some(*k) != pick).map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(pi) = pick {
        let fps = app.scene.fps.max(1.0) as f64;
        let (sid, _) = stacks[pi];
        let layers: Vec<i64> = sc.children(sid).filter(|(o, _, _)| o.class == "AnimationLayer").map(|(_, id, _)| id).collect();
        if layers.len() > 1 {
            rep.notes.push(format!("{} animation layers: only the first is imported", layers.len()));
        }
        // model → [(prop, curve node id)]
        let mut model_curves: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
        let mut blend_curves: Vec<(i64, i64)> = Vec::new();
        let (mut fmin, mut fmax) = (f64::MAX, f64::MIN);
        if let Some(&lid) = layers.first() {
            for (cn, cnid, _) in sc.children(lid) {
                if cn.class != "AnimationCurveNode" {
                    continue;
                }
                for (po, pid, prop) in sc.parents(cnid) {
                    let (Some(po), Some(prop)) = (po, prop) else { continue };
                    if po.class == "Model" {
                        model_curves.entry(pid).or_default().push((prop.to_string(), cnid));
                    } else if prop == "DeformPercent" {
                        blend_curves.push((pid, cnid));
                    }
                }
            }
        }
        // Curves of a curve node, per component name ("d|X" …).
        let curves_of = |cnid: i64| -> HashMap<String, FCurve> { sc.children(cnid).filter(|(o, _, _)| o.class == "AnimationCurve").filter_map(|(o, _, p)| p.map(|p| (p.to_string(), FCurve::read(o.node)))).collect() };
        for (mid, props) in &model_curves {
            let Some(&nid) = node_of.get(mid) else { continue };
            let xf = xf_of[mid].clone();
            let mut comp: HashMap<(&str, usize), FCurve> = HashMap::new();
            for (prop, cnid) in props {
                let which = match prop.as_str() {
                    "Lcl Translation" => "t",
                    "Lcl Rotation" => "r",
                    "Lcl Scaling" => "s",
                    _ => continue,
                };
                for (k, c) in curves_of(*cnid) {
                    let axis = match k.as_str() {
                        "d|X" | "X" => 0,
                        "d|Y" | "Y" => 1,
                        "d|Z" | "Z" => 2,
                        _ => continue,
                    };
                    if let (Some(a), Some(b)) = (c.times.first(), c.times.last()) {
                        fmin = fmin.min(*a * fps);
                        fmax = fmax.max(*b * fps);
                    }
                    comp.insert((which, axis), c);
                }
            }
            if comp.is_empty() {
                continue;
            }
            let base = |w: &str, a: usize| -> f64 {
                match w {
                    "t" => xf.t[a] / unit.max(1e-12),
                    "r" => xf.r[a],
                    _ => xf.s[a],
                }
            };
            let n = app.scene.node_mut(nid).unwrap();
            if xf.simple() {
                for ((w, a), c) in &comp {
                    let chan = match *w {
                        "t" => Channel::translate()[*a],
                        "r" => Channel::rotate()[*a],
                        _ => Channel::scale()[*a],
                    };
                    let k = if *w == "t" { unit } else { 1.0 };
                    let mut curve = Curve::default();
                    let all_lin = c.interp.iter().all(|i| *i == 1);
                    if all_lin {
                        curve.tangent = Tangent::Linear;
                    }
                    for (i, (t, v)) in c.times.iter().zip(&c.values).enumerate() {
                        let mut key = Key { frame: (t * fps) as f32, value: (v * k) as f32, tan: None };
                        if c.interp[i] == 0 {
                            key.tan = Some(KeyTan { kind: TanKind::Stepped, in_slope: 0.0, out_slope: 0.0 });
                        }
                        curve.keys.push(key);
                    }
                    curve.keys.sort_by(|a, b| a.frame.total_cmp(&b.frame));
                    curve.keys.dedup_by(|a, b| (a.frame - b.frame).abs() < 1e-4);
                    n.anim.insert(chan, curve);
                    rep.anim_curves += 1;
                }
            } else {
                // Pivots / post-rotation / rotation order: bake every frame to TRS.
                let times: Vec<f64> = comp.values().flat_map(|c| c.times.iter().copied()).collect();
                let (a0, a1) = times.iter().fold((f64::MAX, f64::MIN), |(a, b), t| (a.min(*t), b.max(*t)));
                let (f0, f1) = ((a0 * fps).floor() as i64, (a1 * fps).ceil() as i64);
                let mut curves: Vec<Curve> = (0..9).map(|_| Curve::default()).collect();
                let mut prev = n.rotate;
                for f in f0..=f1.max(f0) {
                    let t = f as f64 / fps;
                    let mut x = xf.clone();
                    for a in 0..3 {
                        x.t[a] = comp.get(&("t", a)).and_then(|c| c.eval(t)).unwrap_or(base("t", a)) * unit;
                        x.r[a] = comp.get(&("r", a)).and_then(|c| c.eval(t)).unwrap_or(base("r", a));
                        x.s[a] = comp.get(&("s", a)).and_then(|c| c.eval(t)).unwrap_or(base("s", a));
                    }
                    let (s, r, tr) = x.matrix().to_scale_rotation_translation();
                    let e = unwrap_euler(prev, crate::rig::quat_euler(r.as_quat()));
                    prev = e;
                    let vals = [tr.x as f32, tr.y as f32, tr.z as f32, e.x, e.y, e.z, s.x as f32, s.y as f32, s.z as f32];
                    for (c, v) in curves.iter_mut().zip(vals) {
                        c.keys.push(Key { frame: f as f32, value: v, tan: None });
                    }
                }
                let chans = [Channel::translate(), Channel::rotate(), Channel::scale()].concat();
                for (c, ch) in curves.into_iter().zip(chans) {
                    n.anim.insert(ch, c);
                    rep.anim_curves += 1;
                }
            }
        }
        for (chid, cnid) in blend_curves {
            let Some(&(nid, k)) = blend_channel_of.get(&chid) else { continue };
            let Some(c) = curves_of(cnid).into_values().next() else { continue };
            let mut curve = Curve::default();
            for (t, v) in c.times.iter().zip(&c.values) {
                curve.keys.push(Key { frame: (t * fps) as f32, value: (*v / 100.0) as f32, tan: None });
                fmin = fmin.min(t * fps);
                fmax = fmax.max(t * fps);
            }
            if let Some(bs) = app.scene.node_mut(nid).and_then(|n| n.blend.as_mut())
                && let Some(slot) = bs.curves.get_mut(k)
            {
                *slot = Some(curve);
                rep.anim_curves += 1;
            }
        }
        if fmax > fmin {
            app.scene.play_start = app.scene.play_start.min(fmin.floor() as f32);
            app.scene.play_end = app.scene.play_end.max(fmax.ceil() as f32);
            app.scene.anim_start = app.scene.anim_start.min(app.scene.play_start);
            app.scene.anim_end = app.scene.anim_end.max(app.scene.play_end);
        }
        let t = app.scene.time;
        app.set_time(t);
    }

    rep.roots = match root {
        Some(r) => vec![r],
        None => model_ids.iter().filter_map(|m| node_of.get(m).copied()).filter(|id| app.scene.node(*id).is_some_and(|n| n.parent.is_none())).collect(),
    };
    app.sel = rep.roots.clone();
    app.end_batch();
    Ok(rep)
}

/// Keep euler angles continuous with the previous key (avoid ±180° jumps).
fn unwrap_euler(prev: Vec3, e: Vec3) -> Vec3 {
    let mut out = e;
    for i in 0..3 {
        while out[i] - prev[i] > 180.0 {
            out[i] -= 360.0;
        }
        while out[i] - prev[i] < -180.0 {
            out[i] += 360.0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::scene::Shape;

    /// `FBX_CORPUS=dir cargo test --release fbx_corpus -- --ignored --nocapture`
    /// Imports every .fbx under the directory and prints counts and world bounds.
    #[test]
    #[ignore]
    fn fbx_corpus() {
        let Ok(dir) = std::env::var("FBX_CORPUS") else { return };
        let mut files = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fbx")) {
                    files.push(p);
                }
            }
        }
        files.sort();
        let (mut ok, mut fail) = (0, 0);
        for f in files {
            let mut app = App::new();
            app.scene.nodes.clear();
            let t = std::time::Instant::now();
            match super::import(&mut app, &f, None) {
                Ok(r) => {
                    ok += 1;
                    crate::rig::evaluate(&mut app.scene);
                    let mut faces = 0;
                    let mut verts = 0;
                    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
                    for n in &app.scene.nodes {
                        if let Shape::Poly { mesh, .. } = &n.shape {
                            faces += mesh.faces.iter().map(|f| f.len().saturating_sub(2)).sum::<usize>();
                            verts += mesh.positions.len();
                            let pts = crate::rig::deformed_positions(&app.scene, n.id).unwrap_or_else(|| {
                                let w = app.scene.world_matrix(n.id);
                                mesh.positions.iter().map(|p| w.transform_point3(*p)).collect()
                            });
                            for p in pts {
                                lo = lo.min(p);
                                hi = hi.max(p);
                            }
                        }
                    }
                    let joints = app.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Joint { .. })).count();
                    println!("OK {} | tris {faces} verts {verts} joints {joints} mats {} skins {} morphs {} curves {} cams {} lights {} | bounds {lo:.2}..{hi:.2} | {:.0} ms | {}", f.display(), r.materials, r.skins, r.morphs, r.anim_curves, r.cameras, r.lights, t.elapsed().as_secs_f32() * 1000.0, r.notes.join("; "));
                }
                Err(e) => {
                    fail += 1;
                    println!("FAIL {} | {e}", f.display());
                }
            }
        }
        println!("{ok} imported, {fail} failed");
    }
}
