//! USD ASCII (.usda) exporter: Xform hierarchy, meshes, curves, UsdPreviewSurface materials,
//! UsdLux lights, cameras, baked transform animation, **UsdSkel** for skinned meshes
//! (Skeleton + SkelAnimation + joint influences + blend shapes) and baked points for the
//! other deformers (clusters, lattices, wraps, blend-shape-only meshes).

use super::{srgb_to_linear, Baked, ExportOptions, ExportSet};
use crate::app::App;
use crate::look::LightKind;
use crate::scene::{NodeId, Shape};
use glam::{Mat4, Quat, Vec3};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// Make a valid, sibling-unique USD prim name.
fn prim_name(name: &str, used: &mut HashSet<String>) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() || s.chars().next().unwrap().is_ascii_digit() {
        s.insert(0, '_');
    }
    let mut out = s.clone();
    let mut i = 1;
    while !used.insert(out.clone()) {
        i += 1;
        out = format!("{s}_{i}");
    }
    out
}

fn f(x: f32) -> String {
    let v = if x.abs() < 1e-9 { 0.0 } else { x };
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" { "0".into() } else { s }
}
fn v3(v: Vec3) -> String {
    format!("({}, {}, {})", f(v.x), f(v.y), f(v.z))
}
fn quat(q: Quat) -> String {
    // USD quaternions are (real, i, j, k).
    format!("({}, {}, {}, {})", f(q.w), f(q.x), f(q.y), f(q.z))
}
fn lin(c: [f32; 3]) -> String {
    format!("({}, {}, {})", f(srgb_to_linear(c[0])), f(srgb_to_linear(c[1])), f(srgb_to_linear(c[2])))
}
fn points(p: &[Vec3]) -> String {
    let mut s = String::from("[");
    for (i, v) in p.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&v3(*v));
    }
    s.push(']');
    s
}

fn mat4(m: &Mat4) -> String {
    // USD matrices are row-vector (rows = basis vectors, translation last); glam columns.
    let c = m.to_cols_array_2d();
    format!(
        "( ({}, {}, {}, {}), ({}, {}, {}, {}), ({}, {}, {}, {}), ({}, {}, {}, {}) )",
        f(c[0][0]),
        f(c[0][1]),
        f(c[0][2]),
        f(c[0][3]),
        f(c[1][0]),
        f(c[1][1]),
        f(c[1][2]),
        f(c[1][3]),
        f(c[2][0]),
        f(c[2][1]),
        f(c[2][2]),
        f(c[2][3]),
        f(c[3][0]),
        f(c[3][1]),
        f(c[3][2]),
        f(c[3][3])
    )
}

fn list<T>(items: &[T], fmt: impl Fn(&T) -> String) -> String {
    format!("[{}]", items.iter().map(fmt).collect::<Vec<_>>().join(", "))
}

fn trs_decompose(m: Mat4) -> (Vec3, Quat, Vec3) {
    let (s, r, t) = m.to_scale_rotation_translation();
    (t, r.normalize(), s)
}

/// One UsdSkel skeleton (a joint hierarchy) with the skinned meshes bound to it.
struct SkelGroup {
    prim: String,
    joints: Vec<NodeId>,
    /// Index of each joint's parent within `joints`.
    parents: Vec<Option<usize>>,
    paths: Vec<String>,
    /// World-space bind matrix per joint.
    bind: Vec<Mat4>,
    meshes: Vec<NodeId>,
    /// Per frame, per joint local (t, r, s); one entry when not animated.
    anim: Vec<Vec<(Vec3, Quat, Vec3)>>,
    /// Blend shape tokens (unique in the animation): (mesh, target index, token).
    blend: Vec<(NodeId, usize, String)>,
    /// Blend weights per frame (empty when not animated).
    blend_anim: Vec<Vec<f32>>,
}

/// Find skinned meshes that UsdSkel can represent and group them by skeleton root.
fn skel_groups(app: &App, set: &ExportSet) -> Vec<SkelGroup> {
    let sc = &app.scene;
    let in_set: HashSet<NodeId> = set.order.iter().copied().collect();
    let mut groups: Vec<SkelGroup> = Vec::new();
    let mut root_used: HashSet<String> = HashSet::new();
    for &id in &set.order {
        let n = sc.node(id).unwrap();
        let Some(mesh) = n.mesh() else { continue };
        let Some(sk) = n.skin.as_ref() else { continue };
        if sk.weights.len() != mesh.positions.len() || sk.joints.is_empty() || !n.clusters.is_empty() || !n.lattices.is_empty() || n.wrap.is_some() {
            continue;
        }
        if !sk.joints.iter().all(|j| in_set.contains(j) && crate::rig::is_joint(sc, *j)) {
            continue;
        }
        // Top-most exported joint above the first influence.
        let mut root = sk.joints[0];
        while let Some(p) = sc.node(root).and_then(|x| x.parent).filter(|p| crate::rig::is_joint(sc, *p) && in_set.contains(p)) {
            root = p;
        }
        if let Some(g) = groups.iter_mut().find(|g| g.joints.first() == Some(&root)) {
            if sk.joints.iter().all(|j| g.joints.contains(j)) {
                g.meshes.push(id);
            }
            continue;
        }
        let joints: Vec<NodeId> = crate::rig::joint_hierarchy(sc, &[root]).into_iter().filter(|j| in_set.contains(j)).collect();
        if !sk.joints.iter().all(|j| joints.contains(j)) {
            continue;
        }
        let parents: Vec<Option<usize>> = joints
            .iter()
            .map(|j| {
                let mut p = sc.node(*j).and_then(|x| x.parent);
                while let Some(pp) = p {
                    if let Some(i) = joints.iter().position(|x| *x == pp) {
                        return Some(i);
                    }
                    p = sc.node(pp).and_then(|x| x.parent);
                }
                None
            })
            .collect();
        let mut paths: Vec<String> = Vec::new();
        let mut sib: HashMap<Option<usize>, HashSet<String>> = HashMap::new();
        for (i, j) in joints.iter().enumerate() {
            let nm = prim_name(&sc.node(*j).unwrap().name, sib.entry(parents[i]).or_default());
            paths.push(match parents[i] {
                Some(p) => format!("{}/{nm}", paths[p]),
                None => nm,
            });
        }
        let prim = prim_name(&format!("{}_skel", sc.node(root).unwrap().name), &mut root_used);
        groups.push(SkelGroup { prim, joints, parents, paths, bind: Vec::new(), meshes: vec![id], anim: Vec::new(), blend: Vec::new(), blend_anim: Vec::new() });
    }
    // Bind matrices: from the skins (inverse of the inverse bind); joints without an influence
    // get their current offset from the parent's bind.
    for g in &mut groups {
        let mut bind: Vec<Option<Mat4>> = vec![None; g.joints.len()];
        for m in &g.meshes {
            let sk = sc.node(*m).unwrap().skin.as_ref().unwrap();
            for (j, ib) in sk.joints.iter().zip(&sk.inv_bind) {
                let i = g.joints.iter().position(|x| x == j).unwrap();
                bind[i].get_or_insert(ib.inverse());
            }
        }
        let mut out = Vec::with_capacity(g.joints.len());
        for i in 0..g.joints.len() {
            let b = bind[i].unwrap_or_else(|| match g.parents[i] {
                Some(p) => out[p] * (sc.world_matrix(g.joints[p]).inverse() * sc.world_matrix(g.joints[i])),
                None => sc.world_matrix(g.joints[i]),
            });
            out.push(b);
        }
        g.bind = out;
        let mut used = HashSet::new();
        for m in &g.meshes {
            let n = sc.node(*m).unwrap();
            if let Some(bs) = &n.blend {
                for (ti, t) in bs.targets.iter().enumerate() {
                    if t.deltas.len() == n.mesh().unwrap().positions.len() {
                        g.blend.push((*m, ti, prim_name(&format!("{}_{}", n.name, t.name), &mut used)));
                    }
                }
            }
        }
    }
    groups
}

/// Joint locals (relative to the parent joint; the root in world space) for the scene state.
fn joint_locals(sc: &crate::scene::Scene, g: &SkelGroup) -> Vec<(Vec3, Quat, Vec3)> {
    let world: Vec<Mat4> = g.joints.iter().map(|j| sc.world_matrix(*j)).collect();
    (0..g.joints.len())
        .map(|i| {
            let m = match g.parents[i] {
                Some(p) => world[p].inverse() * world[i],
                None => world[i],
            };
            trs_decompose(m)
        })
        .collect()
}

struct Writer<'a> {
    app: &'a App,
    set: &'a ExportSet,
    baked: &'a Baked,
    out: String,
    mat_paths: HashMap<u32, String>,
    color_mats: HashMap<String, String>,
    notes: Vec<String>,
    counts: [usize; 5], // meshes, lights, cameras, curves, animated
}

impl Writer<'_> {
    fn ind(&mut self, depth: usize) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
    }

    fn line(&mut self, depth: usize, s: &str) {
        self.ind(depth);
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn node(&mut self, id: NodeId, depth: usize, used: &mut HashSet<String>) {
        let sc = &self.app.scene;
        let n = sc.node(id).unwrap();
        let name = prim_name(&n.name, used);
        let (t, r, s) = self.set.local(sc, id);
        self.line(depth, &format!("def Xform \"{name}\" ("));
        self.line(depth + 1, &format!("customData = {{ string mayaName = \"{}\" }}", n.name.replace('"', "'")));
        self.line(depth, ")");
        self.line(depth, "{");
        let d = depth + 1;
        let anim = self.baked.trs.get(&id);
        let fr = &self.baked.frames;
        match anim {
            Some(samples) => {
                self.counts[4] += 1;
                let mut ts = String::new();
                let mut rs = String::new();
                let mut ss = String::new();
                let mut prev = samples[0].1;
                for (fi, (st, sr, sk)) in samples.iter().enumerate() {
                    let q = if prev.dot(*sr) < 0.0 { -*sr } else { *sr };
                    prev = q;
                    let _ = write!(ts, "{}: {}, ", f(fr[fi]), v3(*st));
                    let _ = write!(rs, "{}: {}, ", f(fr[fi]), quat(q));
                    let _ = write!(ss, "{}: {}, ", f(fr[fi]), v3(*sk));
                }
                self.line(d, &format!("double3 xformOp:translate.timeSamples = {{ {} }}", ts.trim_end_matches(", ")));
                self.line(d, &format!("quatf xformOp:orient.timeSamples = {{ {} }}", rs.trim_end_matches(", ")));
                self.line(d, &format!("float3 xformOp:scale.timeSamples = {{ {} }}", ss.trim_end_matches(", ")));
            }
            None => {
                self.line(d, &format!("double3 xformOp:translate = {}", v3(t)));
                self.line(d, &format!("quatf xformOp:orient = {}", quat(r)));
                self.line(d, &format!("float3 xformOp:scale = {}", v3(s)));
            }
        }
        self.line(d, "uniform token[] xformOpOrder = [\"xformOp:translate\", \"xformOp:orient\", \"xformOp:scale\"]");
        if !sc.is_visible(id) || !n.visible {
            self.line(d, "token visibility = \"invisible\"");
        }
        let mut child_used: HashSet<String> = HashSet::new();
        let shape_name = prim_name(&format!("{name}Shape"), &mut child_used);
        match &n.shape {
            Shape::Poly { mesh, .. } if !mesh.faces.is_empty() => {
                self.mesh(id, d, &shape_name, None);
            }
            Shape::Curve { points: pts, closed, degree, .. } if !pts.is_empty() && *degree > 1 => {
                // Cubic (and other) NURBS curves.
                self.counts[3] += 1;
                let c = crate::nurbs::Curve::from_shape(&n.shape).unwrap();
                let (cvs, order, knots, (a, b)) = crate::nurbs::to_usd(&c);
                let _ = closed;
                self.line(d, &format!("def NurbsCurves \"{shape_name}\""));
                self.line(d, "{");
                self.line(d + 1, &format!("int[] curveVertexCounts = [{}]", cvs.len()));
                self.line(d + 1, &format!("point3f[] points = {}", points(&cvs)));
                self.line(d + 1, &format!("int[] order = [{order}]"));
                self.line(d + 1, &format!("double[] knots = [{}]", knots.iter().map(|k| format!("{k}")).collect::<Vec<_>>().join(", ")));
                self.line(d + 1, &format!("double2[] ranges = [({a}, {b})]"));
                self.line(d + 1, "float[] widths = [0.02] (interpolation = \"constant\")");
                self.line(d + 1, &format!("color3f[] primvars:displayColor = [{}]", lin(n.color)));
                self.line(d, "}");
            }
            Shape::Curve { points: pts, closed, .. } if !pts.is_empty() => {
                self.counts[3] += 1;
                self.line(d, &format!("def BasisCurves \"{shape_name}\""));
                self.line(d, "{");
                self.line(d + 1, "uniform token type = \"linear\"");
                self.line(d + 1, &format!("uniform token wrap = \"{}\"", if *closed { "periodic" } else { "nonperiodic" }));
                self.line(d + 1, &format!("int[] curveVertexCounts = [{}]", pts.len()));
                self.line(d + 1, &format!("point3f[] points = {}", points(pts)));
                self.line(d + 1, "float[] widths = [0.02] (interpolation = \"constant\")");
                self.line(d + 1, &format!("color3f[] primvars:displayColor = [{}]", lin(n.color)));
                self.line(d, "}");
            }
            Shape::Particles(ps) => {
                // Points with per-frame positions (the count may change between samples).
                let w = sc.world_matrix(id);
                let inv = w.inverse();
                let now: Vec<Vec3> = ps.current().iter().map(|q| inv.transform_point3(q.p)).collect();
                self.line(d, &format!("def Points \"{shape_name}\""));
                self.line(d, "{");
                self.line(d + 1, &format!("point3f[] points = {}", points(&now)));
                if let Some(pp) = self.baked.particles.get(&id).filter(|pp| pp.iter().any(|p| !p.is_empty())) {
                    let fr = &self.baked.frames;
                    let mut s = String::new();
                    for (fi, p) in pp.iter().enumerate() {
                        let _ = write!(s, "{}: {}, ", f(fr[fi]), points(p));
                    }
                    self.line(d + 1, &format!("point3f[] points.timeSamples = {{ {} }}", s.trim_end_matches(", ")));
                }
                self.line(d + 1, &format!("float[] widths = [{}] (interpolation = \"constant\")", f(ps.settings.radius * 2.0)));
                self.line(d + 1, &format!("color3f[] primvars:displayColor = [{}]", lin(ps.settings.color)));
                self.line(d, "}");
            }
            Shape::Hair(h) => {
                // Linear BasisCurves (one curve per strand), tapered widths, root→tip colours.
                let per = h.points_per_strand();
                let inv = sc.world_matrix(id).inverse();
                let mut now: Vec<Vec3> = h.current().unwrap_or(&[]).iter().map(|p| inv.transform_point3(*p)).collect();
                if now.is_empty()
                    && let Some(first) = self.baked.hair.get(&id).and_then(|hh| hh.first())
                {
                    now = first.clone();
                }
                if per >= 2 && now.len() >= per {
                    let st = &h.settings;
                    let n_strands = now.len() / per;
                    self.counts[3] += 1;
                    self.line(d, &format!("def BasisCurves \"{shape_name}\""));
                    self.line(d, "{");
                    self.line(d + 1, "uniform token type = \"linear\"");
                    self.line(d + 1, "uniform token wrap = \"nonperiodic\"");
                    self.line(d + 1, &format!("int[] curveVertexCounts = [{}]", vec![per.to_string(); n_strands].join(", ")));
                    self.line(d + 1, &format!("point3f[] points = {}", points(&now)));
                    if let Some(hh) = self.baked.hair.get(&id).filter(|hh| hh.iter().all(|p| p.len() == now.len())) {
                        let fr = &self.baked.frames;
                        let mut s = String::new();
                        for (fi, p) in hh.iter().enumerate() {
                            let _ = write!(s, "{}: {}, ", f(fr[fi]), points(p));
                        }
                        self.line(d + 1, &format!("point3f[] points.timeSamples = {{ {} }}", s.trim_end_matches(", ")));
                    }
                    let widths: Vec<String> = (0..per).map(|i| f(st.width + (st.tip_width - st.width) * i as f32 / (per - 1) as f32)).collect();
                    let all: Vec<String> = (0..n_strands).flat_map(|_| widths.iter().cloned()).collect();
                    self.line(d + 1, &format!("float[] widths = [{}] (interpolation = \"vertex\")", all.join(", ")));
                    let cols: Vec<String> = (0..per)
                        .map(|i| {
                            let k = i as f32 / (per - 1) as f32;
                            lin([0, 1, 2].map(|c| st.color[c] + (st.tip_color[c] - st.color[c]) * k))
                        })
                        .collect();
                    let allc: Vec<String> = (0..n_strands).flat_map(|_| cols.iter().cloned()).collect();
                    self.line(d + 1, &format!("color3f[] primvars:displayColor = [{}] (interpolation = \"vertex\")", allc.join(", ")));
                    self.line(d, "}");
                }
            }
            Shape::Light(l) => {
                self.counts[1] += 1;
                let (ty, api) = match l.kind {
                    LightKind::Directional => ("DistantLight", ""),
                    LightKind::Point => ("SphereLight", ""),
                    LightKind::Spot => ("SphereLight", "ShapingAPI"),
                    LightKind::Ambient => ("DomeLight", ""),
                    LightKind::Area => ("RectLight", ""),
                };
                if api.is_empty() {
                    self.line(d, &format!("def {ty} \"{shape_name}\""));
                } else {
                    self.line(d, &format!("def {ty} \"{shape_name}\" ("));
                    self.line(d + 1, &format!("prepend apiSchemas = [\"{api}\"]"));
                    self.line(d, ")");
                }
                self.line(d, "{");
                self.line(d + 1, &format!("color3f inputs:color = {}", lin(l.color)));
                self.line(d + 1, &format!("float inputs:intensity = {}", f(l.intensity)));
                match l.kind {
                    LightKind::Point | LightKind::Spot => {
                        self.line(d + 1, "float inputs:radius = 0.05");
                        self.line(d + 1, "bool treatAsPoint = 1");
                    }
                    LightKind::Directional => self.line(d + 1, "float inputs:angle = 0.53"),
                    LightKind::Area => {
                        // Maya's area light is a 2×2 quad scaled by its transform.
                        self.line(d + 1, "float inputs:width = 2");
                        self.line(d + 1, "float inputs:height = 2");
                        self.line(d + 1, "bool inputs:normalize = 0");
                    }
                    _ => {}
                }
                if l.kind == LightKind::Spot {
                    self.line(d + 1, &format!("float inputs:shaping:cone:angle = {}", f(l.cone * 0.5 + l.penumbra)));
                    self.line(d + 1, &format!("float inputs:shaping:cone:softness = {}", f((l.penumbra / (l.cone * 0.5 + l.penumbra).max(1.0)).clamp(0.0, 1.0))));
                }
                if l.shadows {
                    self.line(d + 1, "bool inputs:shadow:enable = 1");
                }
                self.line(d, "}");
            }
            Shape::Camera(c) => {
                self.counts[2] += 1;
                self.line(d, &format!("def Camera \"{shape_name}\""));
                self.line(d, "{");
                self.line(d + 1, &format!("float focalLength = {}", f(c.focal)));
                self.line(d + 1, "float horizontalAperture = 36");
                self.line(d + 1, "float verticalAperture = 24");
                self.line(d + 1, &format!("float2 clippingRange = ({}, {})", f(c.near), f(c.far)));
                self.line(d + 1, "token projection = \"perspective\"");
                self.line(d, "}");
            }
            _ => {}
        }
        let kids: Vec<NodeId> = self.set.order.iter().copied().filter(|c| self.set.parent.get(c).copied().flatten() == Some(id)).collect();
        for k in kids {
            self.node(k, d, &mut child_used);
        }
        self.line(depth, "}");
    }

    /// Write a Mesh prim. With `skel` (group, prim path of the SkelRoot) the mesh is bound to
    /// the skeleton: rest points, joint influences, geomBindTransform and blend shapes.
    fn mesh(&mut self, id: NodeId, d: usize, shape_name: &str, skel: Option<(&SkelGroup, &str)>) {
        let sc = &self.app.scene;
        let n = sc.node(id).unwrap();
        let mesh = n.mesh().unwrap();
        let fr = &self.baked.frames;
        self.counts[0] += 1;
        let mat_path = match n.material.and_then(|m| self.mat_paths.get(&m)) {
            Some(p) => p.clone(),
            None => {
                let key = format!("{:.3}_{:.3}_{:.3}", n.color[0], n.color[1], n.color[2]);
                let k = self.color_mats.len();
                self.color_mats.entry(key).or_insert_with(|| format!("/World/Looks/lambert1_{k}")).clone()
            }
        };
        self.line(d, &format!("def Mesh \"{shape_name}\" ("));
        if skel.is_some() {
            self.line(d + 1, "prepend apiSchemas = [\"SkelBindingAPI\", \"MaterialBindingAPI\"]");
        } else {
            self.line(d + 1, "prepend apiSchemas = [\"MaterialBindingAPI\"]");
        }
        self.line(d, ")");
        self.line(d, "{");
        let counts: Vec<String> = mesh.faces.iter().map(|f| f.len().to_string()).collect();
        let idx: Vec<String> = mesh.faces.iter().flatten().map(|v| v.to_string()).collect();
        self.line(d + 1, &format!("int[] faceVertexCounts = [{}]", counts.join(", ")));
        self.line(d + 1, &format!("int[] faceVertexIndices = [{}]", idx.join(", ")));
        match skel {
            Some((g, root_path)) => {
                let sk = n.skin.as_ref().unwrap();
                // Rest (bind) points in object space; UsdSkel deforms them.
                self.line(d + 1, &format!("point3f[] points = {}", points(&mesh.positions)));
                let (lo, hi) = mesh.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(sk.bind_shape.transform_point3(*p)), b.max(sk.bind_shape.transform_point3(*p))));
                self.line(d + 1, &format!("float3[] extent = [{}, {}]", v3(lo), v3(hi)));
                self.line(d + 1, &format!("matrix4d primvars:skel:geomBindTransform = {}", mat4(&sk.bind_shape)));
                let el = sk.weights.iter().map(|w| w.len()).max().unwrap_or(1).max(1);
                let map: Vec<usize> = sk.joints.iter().map(|j| g.joints.iter().position(|x| x == j).unwrap()).collect();
                let mut ji = Vec::with_capacity(sk.weights.len() * el);
                let mut jw = Vec::with_capacity(sk.weights.len() * el);
                for ws in &sk.weights {
                    let mut ws = ws.clone();
                    ws.sort_by(|a, b| b.1.total_cmp(&a.1));
                    let total: f32 = ws.iter().map(|x| x.1).sum::<f32>().max(1e-9);
                    for k in 0..el {
                        match ws.get(k) {
                            Some((i, w)) => {
                                ji.push(map[*i as usize].to_string());
                                jw.push(f(w / total));
                            }
                            None => {
                                ji.push("0".into());
                                jw.push("0".into());
                            }
                        }
                    }
                }
                self.line(d + 1, &format!("int[] primvars:skel:jointIndices = [{}] (", ji.join(", ")));
                self.line(d + 2, &format!("elementSize = {el}"));
                self.line(d + 2, "interpolation = \"vertex\"");
                self.line(d + 1, ")");
                self.line(d + 1, &format!("float[] primvars:skel:jointWeights = [{}] (", jw.join(", ")));
                self.line(d + 2, &format!("elementSize = {el}"));
                self.line(d + 2, "interpolation = \"vertex\"");
                self.line(d + 1, ")");
                self.line(d + 1, &format!("rel skel:skeleton = <{root_path}/Skeleton>"));
                let shapes: Vec<&(NodeId, usize, String)> = g.blend.iter().filter(|b| b.0 == id).collect();
                if !shapes.is_empty() {
                    let toks: Vec<String> = shapes.iter().map(|b| format!("\"{}\"", b.2)).collect();
                    self.line(d + 1, &format!("uniform token[] skel:blendShapes = [{}]", toks.join(", ")));
                    let tgts: Vec<String> = shapes.iter().map(|b| format!("<{root_path}/{shape_name}/{}>", b.2)).collect();
                    self.line(d + 1, &format!("rel skel:blendShapeTargets = [{}]", tgts.join(", ")));
                }
            }
            None => {
                // Static points: the current deformed shape when deformers are active.
                let rest: Vec<Vec3> = match crate::rig::deformed_positions(sc, id) {
                    Some(wp) => {
                        let inv = sc.world_matrix(id).inverse();
                        wp.iter().map(|p| inv.transform_point3(*p)).collect()
                    }
                    None => mesh.positions.clone(),
                };
                let (lo, hi) = rest.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
                self.line(d + 1, &format!("float3[] extent = [{}, {}]", v3(lo), v3(hi)));
                self.line(d + 1, &format!("point3f[] points = {}", points(&rest)));
                if let Some(pp) = self.baked.points.get(&id) {
                    let mut s = String::new();
                    for (fi, p) in pp.iter().enumerate() {
                        let _ = write!(s, "{}: {}, ", f(fr[fi]), points(p));
                    }
                    self.line(d + 1, &format!("point3f[] points.timeSamples = {{ {} }}", s.trim_end_matches(", ")));
                }
            }
        }
        let col = n.material.and_then(|m| self.app.material(m)).map(|m| m.color).unwrap_or(n.color);
        self.line(d + 1, &format!("color3f[] primvars:displayColor = [{}]", lin(col)));
        if mesh.has_uvs() {
            let vals: Vec<String> = mesh.uvs.iter().map(|u| format!("({}, {})", f(u.x), f(u.y))).collect();
            let ix: Vec<String> = mesh.face_uvs.iter().flatten().map(|i| i.to_string()).collect();
            self.line(d + 1, &format!("texCoord2f[] primvars:st = [{}] (", vals.join(", ")));
            self.line(d + 2, "interpolation = \"faceVarying\"");
            self.line(d + 1, ")");
            self.line(d + 1, &format!("int[] primvars:st:indices = [{}]", ix.join(", ")));
        }
        self.line(d + 1, "uniform token subdivisionScheme = \"none\"");
        self.line(d + 1, "uniform token orientation = \"rightHanded\"");
        if skel.is_some() && (!sc.is_visible(id) || !n.visible) {
            self.line(d + 1, "token visibility = \"invisible\"");
        }
        self.line(d + 1, &format!("rel material:binding = <{mat_path}>"));
        if let Some((g, _)) = skel {
            for b in g.blend.iter().filter(|b| b.0 == id) {
                let t = &n.blend.as_ref().unwrap().targets[b.1];
                self.line(d + 1, &format!("def BlendShape \"{}\"", b.2));
                self.line(d + 1, "{");
                self.line(d + 2, &format!("uniform vector3f[] offsets = {}", points(&t.deltas)));
                self.line(d + 1, "}");
            }
        }
        self.line(d, "}");
    }

    /// SkelRoot with the Skeleton, its SkelAnimation and the bound meshes.
    fn skel_root(&mut self, g: &SkelGroup, depth: usize) {
        let d = depth;
        let root_path = format!("/World/{}", g.prim);
        self.line(d, &format!("def SkelRoot \"{}\"", g.prim));
        self.line(d, "{");
        let joints = list(&g.paths, |p| format!("\"{p}\""));
        self.line(d + 1, "def Skeleton \"Skeleton\" (");
        self.line(d + 2, "prepend apiSchemas = [\"SkelBindingAPI\"]");
        self.line(d + 1, ")");
        self.line(d + 1, "{");
        self.line(d + 2, &format!("uniform token[] joints = {joints}"));
        self.line(d + 2, &format!("uniform matrix4d[] bindTransforms = {}", list(&g.bind, mat4)));
        let rest: Vec<Mat4> = (0..g.joints.len())
            .map(|i| match g.parents[i] {
                Some(p) => g.bind[p].inverse() * g.bind[i],
                None => g.bind[i],
            })
            .collect();
        self.line(d + 2, &format!("uniform matrix4d[] restTransforms = {}", list(&rest, mat4)));
        self.line(d + 2, &format!("rel skel:animationSource = <{root_path}/Anim>"));
        self.line(d + 1, "}");
        // Animation.
        self.line(d + 1, "def SkelAnimation \"Anim\"");
        self.line(d + 1, "{");
        self.line(d + 2, &format!("uniform token[] joints = {joints}"));
        let fr = &self.baked.frames;
        let fmt_t = |v: &[(Vec3, Quat, Vec3)]| list(v, |x| v3(x.0));
        let fmt_r = |v: &[(Vec3, Quat, Vec3)]| list(v, |x| quat(x.1));
        let fmt_s = |v: &[(Vec3, Quat, Vec3)]| list(v, |x| v3(x.2));
        if g.anim.len() > 1 && fr.len() == g.anim.len() {
            // Keep quaternion signs continuous per joint.
            let mut anim = g.anim.clone();
            for fi in 1..anim.len() {
                for j in 0..anim[fi].len() {
                    if anim[fi - 1][j].1.dot(anim[fi][j].1) < 0.0 {
                        anim[fi][j].1 = -anim[fi][j].1;
                    }
                }
            }
            let ts = |fmt: &dyn Fn(&[(Vec3, Quat, Vec3)]) -> String| anim.iter().enumerate().map(|(i, a)| format!("{}: {}", f(fr[i]), fmt(a))).collect::<Vec<_>>().join(", ");
            self.line(d + 2, &format!("float3[] translations.timeSamples = {{ {} }}", ts(&fmt_t)));
            self.line(d + 2, &format!("quatf[] rotations.timeSamples = {{ {} }}", ts(&fmt_r)));
            self.line(d + 2, &format!("half3[] scales.timeSamples = {{ {} }}", ts(&fmt_s)));
            self.counts[4] += 1;
        } else if let Some(a) = g.anim.first() {
            self.line(d + 2, &format!("float3[] translations = {}", fmt_t(a)));
            self.line(d + 2, &format!("quatf[] rotations = {}", fmt_r(a)));
            self.line(d + 2, &format!("half3[] scales = {}", fmt_s(a)));
        }
        if !g.blend.is_empty() {
            self.line(d + 2, &format!("uniform token[] blendShapes = {}", list(&g.blend, |b| format!("\"{}\"", b.2))));
            let sc = &self.app.scene;
            let current: Vec<f32> = g.blend.iter().map(|b| sc.node(b.0).and_then(|n| n.blend.as_ref()).and_then(|bs| bs.weights.get(b.1).copied()).unwrap_or(0.0)).collect();
            if g.blend_anim.len() == fr.len() && fr.len() > 1 {
                let mut s = String::new();
                for (fi, w) in g.blend_anim.iter().enumerate() {
                    let _ = write!(s, "{}: {}, ", f(fr[fi]), list(w, |x| f(*x)));
                }
                self.line(d + 2, &format!("float[] blendShapeWeights.timeSamples = {{ {} }}", s.trim_end_matches(", ")));
            } else {
                self.line(d + 2, &format!("float[] blendShapeWeights = {}", list(&current, |x| f(*x))));
            }
        }
        self.line(d + 1, "}");
        let mut used: HashSet<String> = ["Skeleton".to_string(), "Anim".to_string()].into_iter().collect();
        for m in g.meshes.clone() {
            let nm = prim_name(&self.app.scene.node(m).unwrap().name, &mut used);
            self.mesh(m, d + 1, &nm, Some((g, &root_path)));
        }
        self.line(d, "}");
    }

    #[allow(clippy::too_many_arguments)]
    fn material(&mut self, depth: usize, name: &str, color: [f32; 3], rough: f32, metal: f32, spec: f32, emis: [f32; 3], opacity: f32, map: Option<(String, [f32; 2])>, data_maps: [Option<String>; 5]) {
        let d = depth;
        let base = format!("/World/Looks/{name}");
        self.line(d, &format!("def Material \"{name}\""));
        self.line(d, "{");
        self.line(d + 1, &format!("token outputs:surface.connect = <{base}/PreviewSurface.outputs:surface>"));
        let rep_uv = map.as_ref().map(|m| m.1).unwrap_or([1.0, 1.0]);
        if map.is_none() && data_maps.iter().any(|m| m.is_some()) {
            self.line(d + 1, "def Shader \"stReader\"");
            self.line(d + 1, "{");
            self.line(d + 2, "uniform token info:id = \"UsdPrimvarReader_float2\"");
            self.line(d + 2, "string inputs:varname = \"st\"");
            self.line(d + 2, "float2 outputs:result");
            self.line(d + 1, "}");
            self.line(d + 1, "def Shader \"place2dTexture\"");
            self.line(d + 1, "{");
            self.line(d + 2, "uniform token info:id = \"UsdTransform2d\"");
            self.line(d + 2, &format!("float2 inputs:in.connect = <{base}/stReader.outputs:result>"));
            self.line(d + 2, &format!("float2 inputs:scale = ({}, {})", f(rep_uv[0]), f(rep_uv[1])));
            self.line(d + 2, "float2 outputs:result");
            self.line(d + 1, "}");
        }
        for (k, (prim, out)) in [("normalTex", "float3 outputs:rgb"), ("roughnessTex", "float outputs:r"), ("metalnessTex", "float outputs:r"), ("occlusionTex", "float outputs:r"), ("emissionTex", "float3 outputs:rgb")].iter().enumerate() {
            let Some(file) = &data_maps[k] else { continue };
            self.line(d + 1, &format!("def Shader \"{prim}\""));
            self.line(d + 1, "{");
            self.line(d + 2, "uniform token info:id = \"UsdUVTexture\"");
            self.line(d + 2, &format!("asset inputs:file = @{}@", file.replace('\\', "/")));
            self.line(d + 2, &format!("float2 inputs:st.connect = <{base}/place2dTexture.outputs:result>"));
            self.line(d + 2, "token inputs:wrapS = \"repeat\"");
            self.line(d + 2, "token inputs:wrapT = \"repeat\"");
            self.line(d + 2, &format!("token inputs:sourceColorSpace = \"{}\"", if k == 4 { "sRGB" } else { "raw" }));
            if k == 0 {
                self.line(d + 2, "float4 inputs:scale = (2, 2, 2, 1)");
                self.line(d + 2, "float4 inputs:bias = (-1, -1, -1, 0)");
            } else if k == 4 {
                let e = [srgb_to_linear(emis[0]), srgb_to_linear(emis[1]), srgb_to_linear(emis[2])];
                self.line(d + 2, &format!("float4 inputs:scale = ({}, {}, {}, 1)", f(e[0]), f(e[1]), f(e[2])));
            } else if k == 3 {
            } else {
                // The material value multiplies the map (as in the viewport).
                let s = if k == 1 { rough } else { metal };
                self.line(d + 2, &format!("float4 inputs:scale = ({0}, {0}, {0}, 1)", f(s)));
            }
            self.line(d + 2, out);
            self.line(d + 1, "}");
        }
        if let Some((file, rep)) = &map {
            self.line(d + 1, "def Shader \"stReader\"");
            self.line(d + 1, "{");
            self.line(d + 2, "uniform token info:id = \"UsdPrimvarReader_float2\"");
            self.line(d + 2, "string inputs:varname = \"st\"");
            self.line(d + 2, "float2 outputs:result");
            self.line(d + 1, "}");
            self.line(d + 1, "def Shader \"place2dTexture\"");
            self.line(d + 1, "{");
            self.line(d + 2, "uniform token info:id = \"UsdTransform2d\"");
            self.line(d + 2, &format!("float2 inputs:in.connect = <{base}/stReader.outputs:result>"));
            self.line(d + 2, &format!("float2 inputs:scale = ({}, {})", f(rep[0]), f(rep[1])));
            self.line(d + 2, "float2 outputs:result");
            self.line(d + 1, "}");
            self.line(d + 1, "def Shader \"file1\"");
            self.line(d + 1, "{");
            self.line(d + 2, "uniform token info:id = \"UsdUVTexture\"");
            self.line(d + 2, &format!("asset inputs:file = @{}@", file.replace('\\', "/")));
            self.line(d + 2, &format!("float2 inputs:st.connect = <{base}/place2dTexture.outputs:result>"));
            self.line(d + 2, "token inputs:wrapS = \"repeat\"");
            self.line(d + 2, "token inputs:wrapT = \"repeat\"");
            self.line(d + 2, "token inputs:sourceColorSpace = \"sRGB\"");
            self.line(d + 2, "float3 outputs:rgb");
            self.line(d + 1, "}");
        }
        self.line(d + 1, "def Shader \"PreviewSurface\"");
        self.line(d + 1, "{");
        self.line(d + 2, "uniform token info:id = \"UsdPreviewSurface\"");
        if map.is_some() {
            self.line(d + 2, &format!("color3f inputs:diffuseColor.connect = <{base}/file1.outputs:rgb>"));
        } else {
            self.line(d + 2, &format!("color3f inputs:diffuseColor = {}", lin(color)));
        }
        if data_maps[0].is_some() {
            self.line(d + 2, &format!("normal3f inputs:normal.connect = <{base}/normalTex.outputs:rgb>"));
        }
        if data_maps[1].is_some() {
            self.line(d + 2, &format!("float inputs:roughness.connect = <{base}/roughnessTex.outputs:r>"));
        } else {
            self.line(d + 2, &format!("float inputs:roughness = {}", f(rough)));
        }
        if data_maps[2].is_some() {
            self.line(d + 2, &format!("float inputs:metallic.connect = <{base}/metalnessTex.outputs:r>"));
        } else {
            self.line(d + 2, &format!("float inputs:metallic = {}", f(metal)));
        }
        self.line(d + 2, &format!("color3f inputs:specularColor = ({0}, {0}, {0})", f(spec)));
        if data_maps[3].is_some() {
            self.line(d + 2, &format!("float inputs:occlusion.connect = <{base}/occlusionTex.outputs:r>"));
        }
        if data_maps[4].is_some() {
            self.line(d + 2, &format!("color3f inputs:emissiveColor.connect = <{base}/emissionTex.outputs:rgb>"));
        } else if emis.iter().any(|x| *x > 0.0) {
            self.line(d + 2, &format!("color3f inputs:emissiveColor = {}", lin(emis)));
        }
        if opacity < 0.999 {
            self.line(d + 2, &format!("float inputs:opacity = {}", f(opacity)));
        }
        self.line(d + 2, "token outputs:surface");
        self.line(d + 1, "}");
        self.line(d, "}");
    }
}

pub fn export(app: &App, opts: &ExportOptions) -> Result<(Vec<u8>, String), String> {
    let sc = &app.scene;
    let full = ExportSet::new(sc, &app.sel, opts.selected_only);
    if full.order.is_empty() {
        return Err("Nothing to export (select objects or turn off 'selection only').".into());
    }
    let mut groups = if opts.usd_skel { skel_groups(app, &full) } else { Vec::new() };
    // Joints and skinned meshes of UsdSkel groups leave the Xform hierarchy; their other
    // descendants are re-parented to the nearest exported ancestor.
    let excluded: HashSet<NodeId> = groups.iter().flat_map(|g| g.joints.iter().chain(&g.meshes).copied()).collect();
    let set = {
        let order: Vec<NodeId> = full.order.iter().copied().filter(|id| !excluded.contains(id)).collect();
        let mut parent = HashMap::new();
        for &id in &order {
            let mut p = full.parent.get(&id).copied().flatten();
            while let Some(pp) = p.filter(|pp| excluded.contains(pp)) {
                p = full.parent.get(&pp).copied().flatten();
            }
            parent.insert(id, p);
        }
        ExportSet { order, parent }
    };
    let baked = Baked::sample(app, &set, opts, opts.bake_deformation);
    // Joint animation per frame (skeleton space = world).
    if !groups.is_empty() {
        if baked.frames.len() >= 2 {
            let mut sc2 = sc.clone();
            for &fr in &baked.frames {
                sc2.time = fr;
                sc2.evaluate();
                crate::rig::evaluate(&mut sc2);
                for g in &mut groups {
                    let l = joint_locals(&sc2, g);
                    g.anim.push(l);
                    let w: Vec<f32> = g.blend.iter().map(|b| sc2.node(b.0).and_then(|n| n.blend.as_ref()).and_then(|bs| bs.weights.get(b.1).copied()).unwrap_or(0.0)).collect();
                    g.blend_anim.push(w);
                }
            }
            for g in &mut groups {
                let moving = g.anim.windows(2).any(|w| w[0].iter().zip(&w[1]).any(|(a, b)| a.0.distance(b.0) > 1e-5 || a.1.angle_between(b.1) > 1e-5 || a.2.distance(b.2) > 1e-5));
                if !moving {
                    g.anim.truncate(1);
                }
                if !g.blend_anim.windows(2).any(|w| w[0].iter().zip(&w[1]).any(|(a, b)| (a - b).abs() > 1e-6)) {
                    g.blend_anim.clear();
                }
            }
        } else {
            for g in &mut groups {
                g.anim = vec![joint_locals(sc, g)];
            }
        }
    }
    let mut w = Writer { app, set: &set, baked: &baked, out: String::new(), mat_paths: HashMap::new(), color_mats: HashMap::new(), notes: Vec::new(), counts: [0; 5] };
    // Material names (unique prim names under /World/Looks).
    let mut look_used: HashSet<String> = HashSet::new();
    let mats: Vec<(u32, String)> = sc.materials.iter().map(|m| (m.id, prim_name(&m.name, &mut look_used))).collect();
    for (id, nm) in &mats {
        w.mat_paths.insert(*id, format!("/World/Looks/{nm}"));
    }
    let header_start = if baked.frames.is_empty() { sc.time } else { baked.frames[0] };
    let header_end = baked.frames.last().copied().unwrap_or(sc.time);
    let _ = writeln!(w.out, "#usda 1.0");
    let _ = writeln!(w.out, "(");
    let _ = writeln!(w.out, "    defaultPrim = \"World\"");
    let _ = writeln!(w.out, "    doc = \"Exported from Rust Maya\"");
    let _ = writeln!(w.out, "    metersPerUnit = 1");
    let _ = writeln!(w.out, "    upAxis = \"Y\"");
    let _ = writeln!(w.out, "    startTimeCode = {}", f(header_start));
    let _ = writeln!(w.out, "    endTimeCode = {}", f(header_end));
    let _ = writeln!(w.out, "    timeCodesPerSecond = {}", f(sc.fps));
    let _ = writeln!(w.out, "    framesPerSecond = {}", f(sc.fps));
    let _ = writeln!(w.out, ")");
    let _ = writeln!(w.out);
    w.line(0, "def Xform \"World\"");
    w.line(0, "{");
    let mut used: HashSet<String> = HashSet::new();
    used.insert("Looks".into());
    let roots: Vec<NodeId> = set.order.iter().copied().filter(|id| set.parent[id].is_none()).collect();
    for r in roots {
        w.node(r, 1, &mut used);
    }
    for g in &mut groups {
        let p = prim_name(&g.prim, &mut used);
        g.prim = p;
    }
    for g in &groups {
        w.skel_root(g, 1);
    }
    // Looks scope: shared materials, then per-object colour lamberts.
    w.line(1, "def Scope \"Looks\"");
    w.line(1, "{");
    for (id, nm) in &mats {
        let m = app.material(*id).unwrap().clone();
        let map = m.color_map.as_ref().map(|t| {
            if t == crate::texture::CHECKER {
                // Write the checker next to the .usda so the reference resolves.
                let file = opts
                    .out_path
                    .as_ref()
                    .map(|p| p.with_file_name(format!("{}_uvchecker.png", p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("scene".into()))))
                    .unwrap_or_else(|| std::path::PathBuf::from("uvchecker.png"));
                if opts.out_path.is_some() {
                    let _ = std::fs::write(&file, crate::texture::TexImage::checker().to_png());
                }
                (format!("./{}", file.file_name().unwrap().to_string_lossy()), m.repeat_uv)
            } else {
                (t.clone(), m.repeat_uv)
            }
        });
        w.material(2, nm, m.color, m.roughness, m.metalness, m.specular, m.emission, 1.0 - m.transparency, map, [m.normal_map.clone(), m.roughness_map.clone(), m.metalness_map.clone(), m.occlusion_map.clone(), m.emission_map.clone()]);
    }
    let mut cm: Vec<(String, String)> = w.color_mats.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    cm.sort_by(|a, b| a.1.cmp(&b.1));
    for (key, path) in cm {
        let c: Vec<f32> = key.split('_').filter_map(|x| x.parse().ok()).collect();
        let nm = path.rsplit('/').next().unwrap().to_string();
        w.material(2, &nm, [c[0], c[1], c[2]], 0.8, 0.0, 0.0, [0.0; 3], 1.0, None, [None, None, None, None, None]);
    }
    w.line(1, "}");
    w.line(0, "}");
    if !groups.is_empty() {
        w.notes.push(format!(
            "UsdSkel: {} skeleton(s), {} joints, {} skinned mesh(es){}",
            groups.len(),
            groups.iter().map(|g| g.joints.len()).sum::<usize>(),
            groups.iter().map(|g| g.meshes.len()).sum::<usize>(),
            match groups.iter().map(|g| g.blend.len()).sum::<usize>() {
                0 => String::new(),
                b => format!(", {b} blend shape(s)"),
            }
        ));
    }
    if !baked.points.is_empty() {
        w.notes.push(format!("{} deformed mesh(es) baked as point samples", baked.points.len()));
    }
    let [m, l, c, cu, a] = w.counts;
    let mut summary = format!("USD: {} prims, {m} meshes, {} materials, {l} lights, {c} cameras, {cu} curves, {a} animated transforms ({} frames)", set.order.len(), sc.materials.len() + w.color_mats.len(), baked.frames.len());
    if !w.notes.is_empty() {
        summary.push_str(&format!(" — {}", w.notes.join("; ")));
    }
    Ok((w.out.into_bytes(), summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn usda_structure() {
        let mut app = App::new();
        app.scene.nodes.clear();
        for m in [
            "polyCube -n box",
            "shadingNode -asShader blinn -n red -rgb 1 0 0",
            "hyperShade -assign red box",
            "select -cl",
            "joint -p 0 0 0 -n root",
            "joint -p 0 2 0 -n tip",
            "polyCylinder -h 2 -sh 4 -n tube",
            "move 0 1 0",
            "skinCluster root tube",
            "setKeyframe -t 1 -at rz -v 0 root",
            "setKeyframe -t 5 -at rz -v 60 root",
            "playbackOptions -min 1 -max 5",
            "spotLight -n key -sh on",
            "ambientLight",
            "camera -n cam",
            "circle -n ctrl",
        ] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let (bytes, summary) = export(&app, &ExportOptions::from_app(&app, false)).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        eprintln!("{summary}");
        assert!(s.starts_with("#usda 1.0"));
        assert!(s.contains("def SkelRoot \"root_skel\""), "{summary}");
        assert!(s.contains("uniform token[] joints = [\"root\", \"root/tip\"]"));
        assert!(s.contains("def Mesh \"tube\""));
        assert!(s.contains("rel skel:skeleton = </World/root_skel/Skeleton>"));
        assert!(s.contains("quatf[] rotations.timeSamples"), "joint animation");
        assert!(s.contains("primvars:skel:jointWeights"));
        assert!(!s.contains("point3f[] points.timeSamples"), "no baked points with UsdSkel");
        assert!(summary.contains("UsdSkel: 1 skeleton(s), 2 joints, 1 skinned mesh(es)"), "{summary}");
        // Point baking on request.
        let mut opts = ExportOptions::from_app(&app, false);
        opts.usd_skel = false;
        let (bytes2, _) = export(&app, &opts).unwrap();
        let s2 = String::from_utf8(bytes2).unwrap();
        assert!(s2.contains("def Mesh \"tubeShape\"") && s2.contains("point3f[] points.timeSamples"));
        assert!(!s2.contains("SkelRoot"));
        assert!(s.contains("def SphereLight \"keyShape\""));
        assert!(s.contains("ShapingAPI"));
        assert!(s.contains("def DomeLight"));
        assert!(s.contains("def Camera \"camShape\""));
        assert!(s.contains("def NurbsCurves \"ctrlShape\""));
        assert!(s.contains("rel material:binding = </World/Looks/red>"));
        // Balanced braces.
        assert_eq!(s.matches('{').count(), s.matches('}').count());
        let mut used = HashSet::new();
        assert_eq!(prim_name("box-1", &mut used), "box_1");
        assert_eq!(prim_name("box_1", &mut used), "box_1_2");
        assert_eq!(prim_name("1st", &mut used), "_1st");
        std::fs::write(std::env::temp_dir().join("rust_maya_test.usda"), &s).unwrap();
    }

    /// Validates the UsdSkel export with Pixar's USD: `pip install usd-core`, then
    /// `cargo test usdskel_matches -- --ignored`.
    #[test]
    #[ignore]
    fn usdskel_matches_baked_points_in_pxr() {
        let mut app = App::new();
        app.scene.nodes.clear();
        for m in [
            "polyCube -w 0.5 -h 0.8 -d 0.3 -sy 4 -n torso; move 0 1.2 0; polyCube -w 1.6 -h 0.12 -d 0.12 -sx 12 -n arms; move 0 1.47 0",
            "polyCube -w 0.4 -h 0.9 -d 0.15 -sy 6 -sx 2 -n legs; move 0 0.45 0; polyUnite torso arms legs; rename polySurface1 body",
            "duplicate -n bodyFat body; scale -r 1.3 1 1.3 bodyFat; blendShape -n bs bodyFat body; delete bodyFat",
            "move 5 0 0 body",
            "rigAnimate -anim walk -s 1 -e 24 body",
            "setKeyframe -t 1 -at bodyFat -v 0 body; setKeyframe -t 24 -at bodyFat -v 1 body",
            "group -n char_grp root_ctrl",
            "move 2 0 1 char_grp; rotate 0 30 0 char_grp",
            "polySphere -n prop; move 0 3 0 prop",
        ] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let dir = std::env::temp_dir();
        let a = dir.join("rm_skel.usda");
        let b = dir.join("rm_baked.usda");
        let ra = execute(&mut app, &format!("file -export {}", a.display())).unwrap();
        let rb = execute(&mut app, &format!("file -export {} -bp", b.display())).unwrap();
        eprintln!("{ra}\n{rb}");
        assert!(ra.contains("UsdSkel") && ra.contains("blend shape"), "{ra}");
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tools/check_usdskel.py");
        let out = std::process::Command::new("python3").arg(script).arg(&a).arg(&b).output().expect("python3");
        let txt = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        eprintln!("{txt}");
        assert!(out.status.success(), "{txt}");
    }
}
