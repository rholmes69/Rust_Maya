//! USD ASCII (.usda) exporter: Xform hierarchy, meshes, curves, UsdPreviewSurface materials,
//! UsdLux lights, cameras, baked transform animation and baked deformation (points).

use super::{srgb_to_linear, Baked, ExportOptions, ExportSet};
use crate::app::App;
use crate::look::LightKind;
use crate::scene::{NodeId, Shape};
use glam::{Quat, Vec3};
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
                self.line(d + 1, "prepend apiSchemas = [\"MaterialBindingAPI\"]");
                self.line(d, ")");
                self.line(d, "{");
                let counts: Vec<String> = mesh.faces.iter().map(|f| f.len().to_string()).collect();
                let idx: Vec<String> = mesh.faces.iter().flatten().map(|v| v.to_string()).collect();
                let (lo, hi) = mesh.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
                self.line(d + 1, &format!("float3[] extent = [{}, {}]", v3(lo), v3(hi)));
                self.line(d + 1, &format!("int[] faceVertexCounts = [{}]", counts.join(", ")));
                self.line(d + 1, &format!("int[] faceVertexIndices = [{}]", idx.join(", ")));
                // Static points: the current deformed shape when deformers are active.
                let rest: Vec<Vec3> = match crate::rig::deformed_positions(sc, id) {
                    Some(wp) => {
                        let inv = sc.world_matrix(id).inverse();
                        wp.iter().map(|p| inv.transform_point3(*p)).collect()
                    }
                    None => mesh.positions.clone(),
                };
                self.line(d + 1, &format!("point3f[] points = {}", points(&rest)));
                if let Some(pp) = self.baked.points.get(&id) {
                    let mut s = String::new();
                    for (fi, p) in pp.iter().enumerate() {
                        let _ = write!(s, "{}: {}, ", f(fr[fi]), points(p));
                    }
                    self.line(d + 1, &format!("point3f[] points.timeSamples = {{ {} }}", s.trim_end_matches(", ")));
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
                self.line(d + 1, &format!("rel material:binding = <{mat_path}>"));
                self.line(d, "}");
            }
            Shape::Curve { points: pts, closed } if !pts.is_empty() => {
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
            Shape::Light(l) => {
                self.counts[1] += 1;
                let (ty, api) = match l.kind {
                    LightKind::Directional => ("DistantLight", ""),
                    LightKind::Point => ("SphereLight", ""),
                    LightKind::Spot => ("SphereLight", "ShapingAPI"),
                    LightKind::Ambient => ("DomeLight", ""),
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

    #[allow(clippy::too_many_arguments)]
    fn material(&mut self, depth: usize, name: &str, color: [f32; 3], rough: f32, metal: f32, spec: f32, emis: [f32; 3], opacity: f32, map: Option<(String, [f32; 2])>) {
        let d = depth;
        let base = format!("/World/Looks/{name}");
        self.line(d, &format!("def Material \"{name}\""));
        self.line(d, "{");
        self.line(d + 1, &format!("token outputs:surface.connect = <{base}/PreviewSurface.outputs:surface>"));
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
        self.line(d + 2, &format!("float inputs:roughness = {}", f(rough)));
        self.line(d + 2, &format!("float inputs:metallic = {}", f(metal)));
        self.line(d + 2, &format!("color3f inputs:specularColor = ({0}, {0}, {0})", f(spec)));
        if emis.iter().any(|x| *x > 0.0) {
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
    let set = ExportSet::new(sc, &app.sel, opts.selected_only);
    if set.order.is_empty() {
        return Err("Nothing to export (select objects or turn off 'selection only').".into());
    }
    let baked = Baked::sample(app, &set, opts, opts.bake_deformation);
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
        w.material(2, nm, m.color, m.roughness, m.metalness, m.specular, m.emission, 1.0 - m.transparency, map);
    }
    let mut cm: Vec<(String, String)> = w.color_mats.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    cm.sort_by(|a, b| a.1.cmp(&b.1));
    for (key, path) in cm {
        let c: Vec<f32> = key.split('_').filter_map(|x| x.parse().ok()).collect();
        let nm = path.rsplit('/').next().unwrap().to_string();
        w.material(2, &nm, [c[0], c[1], c[2]], 0.8, 0.0, 0.0, [0.0; 3], 1.0, None);
    }
    w.line(1, "}");
    w.line(0, "}");
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
        assert!(s.contains("def Mesh \"tubeShape\""));
        assert!(s.contains("point3f[] points.timeSamples"), "skinned tube baked");
        assert!(s.contains("quatf xformOp:orient.timeSamples"));
        assert!(s.contains("def SphereLight \"keyShape\""));
        assert!(s.contains("ShapingAPI"));
        assert!(s.contains("def DomeLight"));
        assert!(s.contains("def Camera \"camShape\""));
        assert!(s.contains("def BasisCurves \"ctrlShape\""));
        assert!(s.contains("rel material:binding = </World/Looks/red>"));
        // Balanced braces.
        assert_eq!(s.matches('{').count(), s.matches('}').count());
        let mut used = HashSet::new();
        assert_eq!(prim_name("box-1", &mut used), "box_1");
        assert_eq!(prim_name("box_1", &mut used), "box_1_2");
        assert_eq!(prim_name("1st", &mut used), "_1st");
        std::fs::write(std::env::temp_dir().join("rust_maya_test.usda"), &s).unwrap();
    }
}
