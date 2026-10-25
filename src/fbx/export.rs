//! FBX 7.4 binary exporter: hierarchy (joints as LimbNodes with joint orient as pre-rotation),
//! polygon meshes (normals, UVs, hard edges as edge smoothing, material slot), Phong materials
//! with texture files, skin clusters + bind pose, blend shapes, cameras, lights and baked
//! per-frame animation (transforms and blend-shape weights). Metres, Y up
//! (UnitScaleFactor 100), so Maya, Blender, Unity and Unreal import it at the right size.

use super::{write_binary, Node, Prop, CREATION_TIME, FILE_ID, TICKS_PER_SECOND};
use crate::app::App;
use crate::export::{ExportOptions, ExportSet};
use crate::look::LightKind;
use crate::scene::{NodeId, Shape};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

fn s(v: &str) -> Prop {
    Prop::Str(v.to_string())
}

/// "name\0\x01Class" object name.
fn oname(name: &str, class: &str) -> Prop {
    Prop::Str(format!("{name}\u{0}\u{1}{class}"))
}

/// A Properties70 `P` entry.
fn p(name: &str, ty: &str, label: &str, flags: &str, vals: Vec<Prop>) -> Node {
    let mut props = vec![s(name), s(ty), s(label), s(flags)];
    props.extend(vals);
    Node::new("P", props)
}

fn p_vec(name: &str, ty: &str, label: &str, flags: &str, v: Vec3) -> Node {
    p(name, ty, label, flags, vec![Prop::F64(v.x as f64), Prop::F64(v.y as f64), Prop::F64(v.z as f64)])
}

fn p_f(name: &str, v: f64) -> Node {
    p(name, "double", "Number", "", vec![Prop::F64(v)])
}

fn p_i(name: &str, v: i32) -> Node {
    p(name, "int", "Integer", "", vec![Prop::I32(v)])
}

fn p_enum(name: &str, v: i32) -> Node {
    p(name, "enum", "", "", vec![Prop::I32(v)])
}

fn p_color(name: &str, c: [f32; 3]) -> Node {
    p(name, "Color", "", "A", vec![Prop::F64(c[0] as f64), Prop::F64(c[1] as f64), Prop::F64(c[2] as f64)])
}

fn leaf(name: &str, v: Prop) -> Node {
    Node::new(name, vec![v])
}

fn mat_arr(m: Mat4) -> Prop {
    Prop::ArrF64(m.to_cols_array().iter().map(|x| *x as f64).collect())
}

fn ktime(frame: f32, fps: f32) -> i64 {
    ((frame as f64 / fps.max(1.0) as f64) * TICKS_PER_SECOND).round() as i64
}

struct Ids(i64);

impl Ids {
    fn next(&mut self) -> i64 {
        self.0 += 1;
        self.0
    }
}

/// Build the FBX node tree for the scene.
pub fn build(app: &App, opts: &ExportOptions) -> Result<(Vec<Node>, String), String> {
    let scene = &app.scene;
    let set = ExportSet::new(scene, &app.sel, opts.selected_only);
    if set.order.is_empty() {
        return Err("Nothing to export (select objects or turn off 'selection only').".into());
    }
    let fps = scene.fps.max(1.0);
    let mut ids = Ids(1_000_000);
    let mut objects: Vec<Node> = Vec::new();
    let mut conns: Vec<Node> = Vec::new();
    let oo = |c: i64, p: i64| Node::new("C", vec![s("OO"), Prop::I64(c), Prop::I64(p)]);
    let op = |c: i64, p: i64, prop: &str| Node::new("C", vec![s("OP"), Prop::I64(c), Prop::I64(p), s(prop)]);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut notes: Vec<String> = Vec::new();
    let model_of: HashMap<NodeId, i64> = set.order.iter().map(|id| (*id, ids.next())).collect();
    let mut mat_ids: HashMap<String, i64> = HashMap::new();
    let mut blend_channels: Vec<(NodeId, usize, i64)> = Vec::new(); // (node, target, channel id)
    let (mut n_mesh, mut n_skin, mut n_morph) = (0, 0, 0);

    // ---- models
    for &id in &set.order {
        let n = scene.node(id).unwrap();
        let mid = model_of[&id];
        let direct = set.parent.get(&id).copied().flatten() == n.parent;
        let (t, r, sc, jo) = if direct && !(opts.fbx_bake_orient && n.joint_orient != Vec3::ZERO) {
            (n.translate, n.rotate, n.scale, n.joint_orient)
        } else if direct {
            (n.translate, crate::rig::quat_euler(crate::rig::euler_quat(n.joint_orient) * crate::rig::euler_quat(n.rotate)), n.scale, Vec3::ZERO)
        } else {
            let (t, q, sc) = set.local(scene, id);
            (t, crate::rig::quat_euler(q), sc, Vec3::ZERO)
        };
        let class = match &n.shape {
            Shape::Poly { mesh, .. } if !mesh.faces.is_empty() => "Mesh",
            Shape::Joint { .. } => "LimbNode",
            Shape::Camera(_) => "Camera",
            Shape::Light(l) if l.kind != LightKind::Ambient => "Light",
            _ => "Null",
        };
        let mut props = vec![
            p("RotationActive", "bool", "", "", vec![Prop::I32(1)]),
            p_enum("InheritType", 1),
            p_vec("ScalingMax", "Vector3D", "Vector", "", Vec3::ZERO),
            p_i("DefaultAttributeIndex", 0),
            p_vec("Lcl Translation", "Lcl Translation", "", "A", t),
            p_vec("Lcl Rotation", "Lcl Rotation", "", "A", r),
            p_vec("Lcl Scaling", "Lcl Scaling", "", "A", sc),
        ];
        if jo != Vec3::ZERO {
            props.push(p_vec("PreRotation", "Vector3D", "Vector", "", jo));
        }
        // FBX cameras look down +X and lights down -Y; ours look down -Z.
        match class {
            "Camera" => props.push(p_vec("PostRotation", "Vector3D", "Vector", "", Vec3::new(0.0, -90.0, 0.0))),
            "Light" => props.push(p_vec("PostRotation", "Vector3D", "Vector", "", Vec3::new(-90.0, 0.0, 0.0))),
            _ => {}
        }
        if !scene.is_visible(id) {
            props.push(p("Visibility", "Visibility", "", "A", vec![Prop::F64(0.0)]));
        }
        objects.push(Node::new("Model", vec![Prop::I64(mid), oname(&n.name, "Model"), s(class)]).with(vec![
            leaf("Version", Prop::I32(232)),
            Node::new("Properties70", vec![]).with(props),
            leaf("Shading", Prop::Bool(true)),
            leaf("Culling", s("CullingOff")),
        ]));
        *counts.entry("Model").or_default() += 1;
        let parent = set.parent.get(&id).copied().flatten().and_then(|p| model_of.get(&p).copied()).unwrap_or(0);
        conns.push(oo(mid, parent));

        // Node attributes.
        let attr = match &n.shape {
            Shape::Joint { radius } => Some(Node::new("NodeAttribute", vec![Prop::I64(0), oname(&n.name, "NodeAttribute"), s("LimbNode")]).with(vec![
                Node::new("Properties70", vec![]).with(vec![p_f("Size", (*radius as f64 * 100.0).max(1.0))]),
                leaf("TypeFlags", s("Skeleton")),
            ])),
            Shape::Camera(c) => Some(Node::new("NodeAttribute", vec![Prop::I64(0), oname(&n.name, "NodeAttribute"), s("Camera")]).with(vec![
                Node::new("Properties70", vec![]).with(vec![
                    p_enum("ApertureMode", 3),
                    p("FilmWidth", "double", "Number", "", vec![Prop::F64(36.0 / 25.4)]),
                    p("FilmHeight", "double", "Number", "", vec![Prop::F64(24.0 / 25.4)]),
                    p("FocalLength", "Number", "", "A", vec![Prop::F64(c.focal as f64)]),
                    p_f("NearPlane", c.near as f64),
                    p_f("FarPlane", c.far as f64),
                ]),
                leaf("TypeFlags", s("Camera")),
                leaf("GeometryVersion", Prop::I32(124)),
                leaf("CameraOrthoZoom", Prop::F64(1.0)),
            ])),
            Shape::Light(l) if class == "Light" => {
                let ty = match l.kind {
                    LightKind::Directional => 1,
                    LightKind::Spot => 2,
                    LightKind::Area => 3,
                    _ => 0,
                };
                Some(Node::new("NodeAttribute", vec![Prop::I64(0), oname(&n.name, "NodeAttribute"), s("Light")]).with(vec![
                    Node::new("Properties70", vec![]).with(vec![
                        p_enum("LightType", ty),
                        p_color("Color", l.color),
                        p("Intensity", "Number", "", "A", vec![Prop::F64(l.intensity as f64 * 100.0)]),
                        p("InnerAngle", "Number", "", "A", vec![Prop::F64(l.cone as f64)]),
                        p("OuterAngle", "Number", "", "A", vec![Prop::F64((l.cone + 2.0 * l.penumbra) as f64)]),
                        p("CastShadows", "bool", "", "", vec![Prop::I32(l.shadows as i32)]),
                    ]),
                    leaf("TypeFlags", s("Light")),
                    leaf("GeometryVersion", Prop::I32(124)),
                ]))
            }
            Shape::Poly { .. } if class == "Mesh" => None,
            _ => Some(Node::new("NodeAttribute", vec![Prop::I64(0), oname(&n.name, "NodeAttribute"), s("Null")]).with(vec![leaf("TypeFlags", s("Null"))])),
        };
        if let Some(mut a) = attr {
            let aid = ids.next();
            a.props[0] = Prop::I64(aid);
            objects.push(a);
            *counts.entry("NodeAttribute").or_default() += 1;
            conns.push(oo(aid, mid));
        }

        // Geometry.
        let Shape::Poly { mesh, .. } = &n.shape else { continue };
        if class != "Mesh" {
            continue;
        }
        let gid = ids.next();
        let verts: Vec<f64> = mesh.positions.iter().flat_map(|p| [p.x as f64, p.y as f64, p.z as f64]).collect();
        let mut pvi: Vec<i32> = Vec::new();
        let mut corner_of: Vec<Vec<usize>> = Vec::new();
        for f in &mesh.faces {
            let mut c = Vec::new();
            for (k, v) in f.iter().enumerate() {
                c.push(pvi.len());
                pvi.push(if k + 1 == f.len() { -(*v as i32) - 1 } else { *v as i32 });
            }
            corner_of.push(c);
        }
        let cn = mesh.corner_normals();
        let normals: Vec<f64> = cn.iter().flatten().flat_map(|n| [n.x as f64, n.y as f64, n.z as f64]).collect();
        // Edges (first corner of each unique edge) and their smoothing (1 = soft).
        let hard = mesh.effective_hard_edges();
        let mut seen: HashMap<(u32, u32), ()> = HashMap::new();
        let mut edges: Vec<i32> = Vec::new();
        let mut smooth: Vec<i32> = Vec::new();
        for (fi, f) in mesh.faces.iter().enumerate() {
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                let e = (a.min(b), a.max(b));
                if seen.insert(e, ()).is_none() {
                    edges.push(corner_of[fi][k] as i32);
                    smooth.push(if hard.contains(&e) { 0 } else { 1 });
                }
            }
        }
        let mut gchildren = vec![
            Node::new("Properties70", vec![]),
            leaf("GeometryVersion", Prop::I32(124)),
            leaf("Vertices", Prop::ArrF64(verts)),
            leaf("PolygonVertexIndex", Prop::ArrI32(pvi)),
            leaf("Edges", Prop::ArrI32(edges)),
            Node::new("LayerElementNormal", vec![Prop::I32(0)]).with(vec![
                leaf("Version", Prop::I32(102)),
                leaf("Name", s("")),
                leaf("MappingInformationType", s("ByPolygonVertex")),
                leaf("ReferenceInformationType", s("Direct")),
                leaf("Normals", Prop::ArrF64(normals)),
            ]),
            Node::new("LayerElementSmoothing", vec![Prop::I32(0)]).with(vec![
                leaf("Version", Prop::I32(102)),
                leaf("Name", s("")),
                leaf("MappingInformationType", s("ByEdge")),
                leaf("ReferenceInformationType", s("Direct")),
                leaf("Smoothing", Prop::ArrI32(smooth)),
            ]),
        ];
        let mut layer = vec![
            leaf("Version", Prop::I32(100)),
            Node::new("LayerElement", vec![]).with(vec![leaf("Type", s("LayerElementNormal")), leaf("TypedIndex", Prop::I32(0))]),
            Node::new("LayerElement", vec![]).with(vec![leaf("Type", s("LayerElementSmoothing")), leaf("TypedIndex", Prop::I32(0))]),
        ];
        if mesh.has_uvs() {
            let uv: Vec<f64> = mesh.uvs.iter().flat_map(|u| [u.x as f64, u.y as f64]).collect();
            let uvi: Vec<i32> = mesh.face_uvs.iter().flatten().map(|x| *x as i32).collect();
            gchildren.push(Node::new("LayerElementUV", vec![Prop::I32(0)]).with(vec![
                leaf("Version", Prop::I32(101)),
                leaf("Name", s("map1")),
                leaf("MappingInformationType", s("ByPolygonVertex")),
                leaf("ReferenceInformationType", s("IndexToDirect")),
                leaf("UV", Prop::ArrF64(uv)),
                leaf("UVIndex", Prop::ArrI32(uvi)),
            ]));
            layer.push(Node::new("LayerElement", vec![]).with(vec![leaf("Type", s("LayerElementUV")), leaf("TypedIndex", Prop::I32(0))]));
        }
        gchildren.push(Node::new("LayerElementMaterial", vec![Prop::I32(0)]).with(vec![
            leaf("Version", Prop::I32(101)),
            leaf("Name", s("")),
            leaf("MappingInformationType", s("AllSame")),
            leaf("ReferenceInformationType", s("IndexToDirect")),
            leaf("Materials", Prop::ArrI32(vec![0])),
        ]));
        layer.push(Node::new("LayerElement", vec![]).with(vec![leaf("Type", s("LayerElementMaterial")), leaf("TypedIndex", Prop::I32(0))]));
        gchildren.push(Node::new("Layer", vec![Prop::I32(0)]).with(layer));
        objects.push(Node::new("Geometry", vec![Prop::I64(gid), oname(&n.name, "Geometry"), s("Mesh")]).with(gchildren));
        *counts.entry("Geometry").or_default() += 1;
        conns.push(oo(gid, mid));
        n_mesh += 1;

        // Material (shared between meshes).
        let mat = n.material.and_then(|m| app.material(m));
        let mat_id = match mat {
            Some(m) => *mat_ids.entry(format!("m{}", m.id)).or_insert_with(|| {
                let id = ids.next();
                let shin = (2.0 / (m.roughness.max(0.02) as f64).powi(2) - 2.0).clamp(2.0, 1024.0);
                objects.push(Node::new("Material", vec![Prop::I64(id), oname(&m.name, "Material"), s("")]).with(vec![
                    leaf("Version", Prop::I32(102)),
                    leaf("ShadingModel", s("phong")),
                    leaf("MultiLayer", Prop::I32(0)),
                    Node::new("Properties70", vec![]).with(vec![
                        p("ShadingModel", "KString", "", "", vec![s("Phong")]),
                        p_color("DiffuseColor", m.color),
                        p("DiffuseFactor", "Number", "", "A", vec![Prop::F64(1.0)]),
                        p_color("EmissiveColor", m.emission),
                        p("EmissiveFactor", "Number", "", "A", vec![Prop::F64(1.0)]),
                        p_color("SpecularColor", [m.specular * 0.5, m.specular * 0.5, m.specular * 0.5]),
                        p("ShininessExponent", "Number", "", "A", vec![Prop::F64(shin)]),
                        p("TransparencyFactor", "Number", "", "A", vec![Prop::F64(m.transparency as f64)]),
                        p("Opacity", "Number", "", "A", vec![Prop::F64(1.0 - m.transparency as f64)]),
                        p("ReflectionFactor", "Number", "", "A", vec![Prop::F64(m.metalness as f64)]),
                    ]),
                ]));
                *counts.entry("Material").or_default() += 1;
                // Textures (file references).
                for (path, slot) in [(&m.color_map, "DiffuseColor"), (&m.normal_map, "NormalMap"), (&m.emission_map, "EmissiveColor")] {
                    let Some(path) = path.as_ref().filter(|p| std::path::Path::new(p).is_file()) else { continue };
                    let abs = std::fs::canonicalize(path).map(|p| p.display().to_string()).unwrap_or(path.clone());
                    let rel = opts.out_path.as_ref().and_then(|o| o.parent()).and_then(|d| std::path::Path::new(&abs).strip_prefix(d).ok()).map(|p| p.display().to_string()).unwrap_or_else(|| std::path::Path::new(&abs).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
                    let tname = std::path::Path::new(&abs).file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or("tex".into());
                    let (tid, vid) = (ids.next(), ids.next());
                    objects.push(Node::new("Video", vec![Prop::I64(vid), oname(&tname, "Video"), s("Clip")]).with(vec![
                        leaf("Type", s("Clip")),
                        Node::new("Properties70", vec![]).with(vec![p("Path", "KString", "XRefUrl", "", vec![s(&abs)])]),
                        leaf("UseMipMap", Prop::I32(0)),
                        leaf("Filename", s(&abs)),
                        leaf("RelativeFilename", s(&rel)),
                    ]));
                    objects.push(Node::new("Texture", vec![Prop::I64(tid), oname(&tname, "Texture"), s("")]).with(vec![
                        leaf("Type", s("TextureVideoClip")),
                        leaf("Version", Prop::I32(202)),
                        leaf("TextureName", oname(&tname, "Texture")),
                        Node::new("Properties70", vec![]).with(vec![p("UseMaterial", "bool", "", "", vec![Prop::I32(1)])]),
                        leaf("Media", oname(&tname, "Video")),
                        leaf("FileName", s(&abs)),
                        leaf("RelativeFilename", s(&rel)),
                        Node::new("ModelUVTranslation", vec![Prop::F64(0.0), Prop::F64(0.0)]),
                        Node::new("ModelUVScaling", vec![Prop::F64(1.0), Prop::F64(1.0)]),
                        leaf("Texture_Alpha_Source", s("None")),
                        Node::new("Cropping", vec![Prop::I32(0), Prop::I32(0), Prop::I32(0), Prop::I32(0)]),
                    ]));
                    *counts.entry("Texture").or_default() += 1;
                    *counts.entry("Video").or_default() += 1;
                    conns.push(oo(vid, tid));
                    conns.push(op(tid, id, slot));
                }
                id
            }),
            None => {
                // A default material from the object colour.
                let key = format!("c{:?}", n.color);
                *mat_ids.entry(key).or_insert_with(|| {
                    let id2 = ids.next();
                    objects.push(Node::new("Material", vec![Prop::I64(id2), oname(&format!("{}_color", n.name), "Material"), s("")]).with(vec![
                        leaf("Version", Prop::I32(102)),
                        leaf("ShadingModel", s("lambert")),
                        leaf("MultiLayer", Prop::I32(0)),
                        Node::new("Properties70", vec![]).with(vec![p_color("DiffuseColor", n.color), p("DiffuseFactor", "Number", "", "A", vec![Prop::F64(1.0)])]),
                    ]));
                    *counts.entry("Material").or_default() += 1;
                    id2
                })
            }
        };
        conns.push(oo(mat_id, mid));

        // Skin.
        if let Some(sk) = n.skin.as_ref().filter(|sk| sk.weights.len() == mesh.positions.len() && sk.joints.iter().all(|j| model_of.contains_key(j))) {
            let skid = ids.next();
            objects.push(Node::new("Deformer", vec![Prop::I64(skid), oname(&n.name, "Deformer"), s("Skin")]).with(vec![leaf("Version", Prop::I32(101)), leaf("Link_DeformAcuracy", Prop::F64(50.0))]));
            *counts.entry("Deformer").or_default() += 1;
            conns.push(oo(skid, gid));
            let mut pose_nodes = vec![Node::new("PoseNode", vec![]).with(vec![leaf("Node", Prop::I64(mid)), leaf("Matrix", mat_arr(sk.bind_shape))])];
            for (ji, j) in sk.joints.iter().enumerate() {
                let (idx, w): (Vec<i32>, Vec<f64>) = sk.weights.iter().enumerate().filter_map(|(v, ws)| ws.iter().find(|x| x.0 as usize == ji).map(|x| (v as i32, x.1 as f64))).unzip();
                let tl = sk.inv_bind.get(ji).map(|m| m.inverse()).unwrap_or(Mat4::IDENTITY);
                // Stored "Transform" is relative to the bone: TransformLink⁻¹ · mesh at bind.
                let tf = tl.inverse() * sk.bind_shape;
                let cid = ids.next();
                let jname = scene.node(*j).map(|x| x.name.clone()).unwrap_or_default();
                objects.push(Node::new("Deformer", vec![Prop::I64(cid), oname(&jname, "SubDeformer"), s("Cluster")]).with(vec![
                    leaf("Version", Prop::I32(100)),
                    Node::new("UserData", vec![s(""), s("")]),
                    leaf("Indexes", Prop::ArrI32(idx)),
                    leaf("Weights", Prop::ArrF64(w)),
                    leaf("Transform", mat_arr(tf)),
                    leaf("TransformLink", mat_arr(tl)),
                ]));
                *counts.entry("Deformer").or_default() += 1;
                conns.push(oo(cid, skid));
                conns.push(oo(model_of[j], cid));
                pose_nodes.push(Node::new("PoseNode", vec![]).with(vec![leaf("Node", Prop::I64(model_of[j])), leaf("Matrix", mat_arr(tl))]));
            }
            let pid = ids.next();
            let mut pc = vec![leaf("Type", s("BindPose")), leaf("Version", Prop::I32(100)), leaf("NbPoseNodes", Prop::I32(pose_nodes.len() as i32))];
            pc.extend(pose_nodes);
            objects.push(Node::new("Pose", vec![Prop::I64(pid), oname(&format!("{}_BindPose", n.name), "Pose"), s("BindPose")]).with(pc));
            *counts.entry("Pose").or_default() += 1;
            n_skin += 1;
        } else if n.skin.is_some() {
            notes.push(format!("{}: skin skipped (its joints aren't exported)", n.name));
        }

        // Blend shapes.
        if let Some(bs) = n.blend.as_ref().filter(|b| !b.targets.is_empty()) {
            let bid = ids.next();
            objects.push(Node::new("Deformer", vec![Prop::I64(bid), oname(&n.name, "Deformer"), s("BlendShape")]).with(vec![leaf("Version", Prop::I32(100))]));
            *counts.entry("Deformer").or_default() += 1;
            conns.push(oo(bid, gid));
            for (k, t) in bs.targets.iter().enumerate() {
                if t.deltas.len() != mesh.positions.len() {
                    continue;
                }
                let (idx, d): (Vec<i32>, Vec<[f64; 3]>) = t.deltas.iter().enumerate().filter(|(_, d)| d.length_squared() > 1e-14).map(|(i, d)| (i as i32, [d.x as f64, d.y as f64, d.z as f64])).unzip();
                let chid = ids.next();
                let shid = ids.next();
                objects.push(Node::new("Deformer", vec![Prop::I64(chid), oname(&t.name, "SubDeformer"), s("BlendShapeChannel")]).with(vec![
                    leaf("Version", Prop::I32(100)),
                    leaf("DeformPercent", Prop::F64(bs.weights.get(k).copied().unwrap_or(0.0) as f64 * 100.0)),
                    leaf("FullWeights", Prop::ArrF64(vec![100.0])),
                    Node::new("Properties70", vec![]).with(vec![p("DeformPercent", "Number", "", "A", vec![Prop::F64(bs.weights.get(k).copied().unwrap_or(0.0) as f64 * 100.0)])]),
                ]));
                objects.push(Node::new("Geometry", vec![Prop::I64(shid), oname(&t.name, "Geometry"), s("Shape")]).with(vec![
                    leaf("Version", Prop::I32(100)),
                    leaf("Indexes", Prop::ArrI32(idx.clone())),
                    leaf("Vertices", Prop::ArrF64(d.iter().flatten().copied().collect())),
                    leaf("Normals", Prop::ArrF64(vec![0.0; idx.len() * 3])),
                ]));
                *counts.entry("Deformer").or_default() += 1;
                *counts.entry("Geometry").or_default() += 1;
                conns.push(oo(chid, bid));
                conns.push(oo(shid, chid));
                blend_channels.push((id, k, chid));
                n_morph += 1;
            }
        }
    }

    // ---- animation: sample every frame, keep channels that change
    let mut n_curves = 0;
    let animate = opts.animation && opts.end > opts.start;
    let frames: Vec<f32> = if animate { (opts.start.floor() as i64..=opts.end.ceil() as i64).map(|f| f as f32).collect() } else { Vec::new() };
    let stack_id = ids.next();
    let layer_id = ids.next();
    if frames.len() > 1 {
        let mut sc = scene.clone();
        // node → 9 channels per frame
        let mut trs: HashMap<NodeId, Vec<[f32; 9]>> = HashMap::new();
        let mut weights: HashMap<(NodeId, usize), Vec<f32>> = HashMap::new();
        for &f in &frames {
            sc.time = f;
            sc.evaluate();
            crate::rig::evaluate(&mut sc);
            for &id in &set.order {
                let n = sc.node(id).unwrap();
                let direct = set.parent.get(&id).copied().flatten() == n.parent;
                let (t, r, s3) = if direct && !(opts.fbx_bake_orient && n.joint_orient != Vec3::ZERO) {
                    (n.translate, n.rotate, n.scale)
                } else if direct {
                    (n.translate, crate::rig::quat_euler(crate::rig::euler_quat(n.joint_orient) * crate::rig::euler_quat(n.rotate)), n.scale)
                } else {
                    let (t, q, s3) = set.local(&sc, id);
                    (t, crate::rig::quat_euler(q), s3)
                };
                let e = trs.entry(id).or_default();
                let r = match e.last() {
                    Some(prev) => unwrap(Vec3::new(prev[3], prev[4], prev[5]), r),
                    None => r,
                };
                e.push([t.x, t.y, t.z, r.x, r.y, r.z, s3.x, s3.y, s3.z]);
            }
            for &(nid, k, _) in &blend_channels {
                if let Some(w) = sc.node(nid).and_then(|n| n.blend.as_ref()).and_then(|b| b.weights.get(k)) {
                    weights.entry((nid, k)).or_default().push(*w * 100.0);
                }
            }
        }
        let times: Vec<i64> = frames.iter().map(|f| ktime(*f, fps)).collect();
        let curve = |ids: &mut Ids, objects: &mut Vec<Node>, vals: &[f32]| -> i64 {
            let id = ids.next();
            objects.push(Node::new("AnimationCurve", vec![Prop::I64(id), oname("", "AnimCurve"), s("")]).with(vec![
                leaf("Default", Prop::F64(vals[0] as f64)),
                leaf("KeyVer", Prop::I32(4009)),
                leaf("KeyTime", Prop::ArrI64(times.clone())),
                leaf("KeyValueFloat", Prop::ArrF32(vals.to_vec())),
                // Linear interpolation for every key.
                leaf("KeyAttrFlags", Prop::ArrI32(vec![0x4 | 0x100 | 0x2000 | 0x4000])),
                leaf("KeyAttrDataFloat", Prop::ArrF32(vec![0.0, 0.0, f32::from_bits(218_434_821), 0.0])),
                leaf("KeyAttrRefCount", Prop::ArrI32(vec![vals.len() as i32])),
            ]));
            id
        };
        // Ignore solver noise (IK, constraints) well below what anyone could see.
        let changes = |v: &[f32]| v.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-4);
        for &id in &set.order {
            let Some(rows) = trs.get(&id) else { continue };
            for (gi, (cname, prop)) in [("T", "Lcl Translation"), ("R", "Lcl Rotation"), ("S", "Lcl Scaling")].iter().enumerate() {
                let cols: Vec<Vec<f32>> = (0..3).map(|a| rows.iter().map(|r| r[gi * 3 + a]).collect()).collect();
                if !cols.iter().any(|c| changes(c)) {
                    continue;
                }
                let cnid = ids.next();
                objects.push(Node::new("AnimationCurveNode", vec![Prop::I64(cnid), oname(cname, "AnimCurveNode"), s("")]).with(vec![Node::new("Properties70", vec![]).with(vec![
                    p("d|X", "Number", "", "A", vec![Prop::F64(cols[0][0] as f64)]),
                    p("d|Y", "Number", "", "A", vec![Prop::F64(cols[1][0] as f64)]),
                    p("d|Z", "Number", "", "A", vec![Prop::F64(cols[2][0] as f64)]),
                ])]));
                *counts.entry("AnimationCurveNode").or_default() += 1;
                conns.push(oo(cnid, layer_id));
                conns.push(op(cnid, model_of[&id], prop));
                for (a, axis) in ["d|X", "d|Y", "d|Z"].iter().enumerate() {
                    let cid = curve(&mut ids, &mut objects, &cols[a]);
                    *counts.entry("AnimationCurve").or_default() += 1;
                    conns.push(op(cid, cnid, axis));
                    n_curves += 1;
                }
            }
        }
        for &(nid, k, chid) in &blend_channels {
            let Some(w) = weights.get(&(nid, k)).filter(|w| changes(w)) else { continue };
            let cnid = ids.next();
            objects.push(Node::new("AnimationCurveNode", vec![Prop::I64(cnid), oname("DeformPercent", "AnimCurveNode"), s("")]).with(vec![Node::new("Properties70", vec![]).with(vec![p("d|DeformPercent", "Number", "", "A", vec![Prop::F64(w[0] as f64)])])]));
            *counts.entry("AnimationCurveNode").or_default() += 1;
            conns.push(oo(cnid, layer_id));
            conns.push(op(cnid, chid, "DeformPercent"));
            let cid = curve(&mut ids, &mut objects, w);
            *counts.entry("AnimationCurve").or_default() += 1;
            conns.push(op(cid, cnid, "d|DeformPercent"));
            n_curves += 1;
        }
    }
    let (t0, t1) = (ktime(opts.start, fps), ktime(opts.end.max(opts.start), fps));
    objects.push(Node::new("AnimationStack", vec![Prop::I64(stack_id), oname("Take 001", "AnimStack"), s("")]).with(vec![Node::new("Properties70", vec![]).with(vec![
        p("LocalStart", "KTime", "Time", "", vec![Prop::I64(t0)]),
        p("LocalStop", "KTime", "Time", "", vec![Prop::I64(t1)]),
        p("ReferenceStart", "KTime", "Time", "", vec![Prop::I64(t0)]),
        p("ReferenceStop", "KTime", "Time", "", vec![Prop::I64(t1)]),
    ])]));
    objects.push(Node::new("AnimationLayer", vec![Prop::I64(layer_id), oname("BaseLayer", "AnimLayer"), s("")]).with(vec![Node::new("Properties70", vec![])]));
    *counts.entry("AnimationStack").or_default() += 1;
    *counts.entry("AnimationLayer").or_default() += 1;
    conns.push(oo(layer_id, stack_id));

    // ---- document
    let time_mode = match fps.round() as i32 {
        120 => 1,
        100 => 2,
        60 => 3,
        50 => 4,
        48 => 5,
        30 => 6,
        25 => 10,
        24 => 11,
        _ => 14,
    };
    let header = Node::new("FBXHeaderExtension", vec![]).with(vec![
        leaf("FBXHeaderVersion", Prop::I32(1003)),
        leaf("FBXVersion", Prop::I32(7400)),
        leaf("EncryptionType", Prop::I32(0)),
        Node::new("CreationTimeStamp", vec![]).with(vec![
            leaf("Version", Prop::I32(1000)),
            leaf("Year", Prop::I32(1970)),
            leaf("Month", Prop::I32(1)),
            leaf("Day", Prop::I32(1)),
            leaf("Hour", Prop::I32(10)),
            leaf("Minute", Prop::I32(0)),
            leaf("Second", Prop::I32(0)),
            leaf("Millisecond", Prop::I32(0)),
        ]),
        leaf("Creator", s("Rust Maya FBX exporter")),
    ]);
    let global = Node::new("GlobalSettings", vec![]).with(vec![
        leaf("Version", Prop::I32(1000)),
        Node::new("Properties70", vec![]).with(vec![
            p_i("UpAxis", 1),
            p_i("UpAxisSign", 1),
            p_i("FrontAxis", 2),
            p_i("FrontAxisSign", 1),
            p_i("CoordAxis", 0),
            p_i("CoordAxisSign", 1),
            p_i("OriginalUpAxis", 1),
            p_i("OriginalUpAxisSign", 1),
            p_f("UnitScaleFactor", 100.0),
            p_f("OriginalUnitScaleFactor", 100.0),
            p("AmbientColor", "ColorRGB", "Color", "", vec![Prop::F64(0.0), Prop::F64(0.0), Prop::F64(0.0)]),
            p("DefaultCamera", "KString", "", "", vec![s("Producer Perspective")]),
            p_enum("TimeMode", time_mode),
            p_enum("TimeProtocol", 2),
            p_enum("SnapOnFrameMode", 0),
            p("TimeSpanStart", "KTime", "Time", "", vec![Prop::I64(t0)]),
            p("TimeSpanStop", "KTime", "Time", "", vec![Prop::I64(t1)]),
            p_f("CustomFrameRate", fps as f64),
        ]),
    ]);
    let doc_id = ids.next();
    let documents = Node::new("Documents", vec![]).with(vec![
        leaf("Count", Prop::I32(1)),
        Node::new("Document", vec![Prop::I64(doc_id), s("Scene"), s("Scene")]).with(vec![
            Node::new("Properties70", vec![]).with(vec![p("SourceObject", "object", "", "", vec![]), p("ActiveAnimStackName", "KString", "", "", vec![s("Take 001")])]),
            leaf("RootNode", Prop::I64(0)),
        ]),
    ]);
    let mut defs = vec![leaf("Version", Prop::I32(100)), leaf("Count", Prop::I32(counts.values().sum::<usize>() as i32 + 1))];
    defs.push(Node::new("ObjectType", vec![s("GlobalSettings")]).with(vec![leaf("Count", Prop::I32(1))]));
    let mut keys: Vec<&&str> = counts.keys().collect();
    keys.sort();
    for k in keys {
        defs.push(Node::new("ObjectType", vec![s(k)]).with(vec![leaf("Count", Prop::I32(counts[*k] as i32))]));
    }
    let takes = Node::new("Takes", vec![]).with(vec![
        leaf("Current", s("Take 001")),
        Node::new("Take", vec![s("Take 001")]).with(vec![
            leaf("FileName", s("Take_001.tak")),
            Node::new("LocalTime", vec![Prop::I64(t0), Prop::I64(t1)]),
            Node::new("ReferenceTime", vec![Prop::I64(t0), Prop::I64(t1)]),
        ]),
    ]);
    let nodes = vec![
        header,
        leaf("FileId", Prop::Raw(FILE_ID.to_vec())),
        leaf("CreationTime", s(CREATION_TIME)),
        leaf("Creator", s("Rust Maya FBX exporter")),
        global,
        documents,
        Node::new("References", vec![]),
        Node::new("Definitions", vec![]).with(defs),
        Node::new("Objects", vec![]).with(objects),
        Node::new("Connections", vec![]).with(conns),
        takes,
    ];
    let mut summary = format!("FBX: {} nodes, {n_mesh} meshes, {} materials, {n_skin} skins, {n_morph} blend-shape targets, {n_curves} animation curves ({} frames)", set.order.len(), counts.get("Material").copied().unwrap_or(0), if n_curves > 0 { frames.len() } else { 0 });
    if !notes.is_empty() {
        summary.push_str(&format!(" — note: {}", notes.join("; ")));
    }
    Ok((nodes, summary))
}

fn unwrap(prev: Vec3, e: Vec3) -> Vec3 {
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

pub fn export(app: &App, opts: &ExportOptions) -> Result<(Vec<u8>, String), String> {
    let (nodes, summary) = build(app, opts)?;
    Ok((write_binary(&nodes, 7400), summary))
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;
    use crate::scene::Shape;
    use glam::Vec3;

    fn world_pts(app: &App, name: &str) -> Vec<Vec3> {
        let id = app.scene.find_by_name(name).unwrap_or_else(|| panic!("no {name}"));
        crate::rig::deformed_positions(&app.scene, id).unwrap_or_else(|| {
            let w = app.scene.world_matrix(id);
            app.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        })
    }

    /// Export a rigged, animated, blend-shaped, textured scene to FBX and import it back.
    #[test]
    fn fbx_round_trip() {
        let dir = std::env::temp_dir().join("rm_fbx_rt");
        let _ = std::fs::create_dir_all(&dir);
        let png = dir.join("skin.png");
        std::fs::write(&png, crate::texture::TexImage::checker().to_png()).unwrap();
        let mut a = App::new();
        a.scene.nodes.clear();
        for m in [
            "playbackOptions -min 1 -max 24".to_string(),
            "character -style human -n hero".into(),
            "rigAnimate -anim wave -s 1 -e 24 hero".into(),
            "shadingNode -asShader standardSurface -n skin_mat -rgb 0.9 0.6 0.5".into(),
            format!("setAttr skin_mat.colorMap \"{}\"", png.display()),
            "hyperShade -assign skin_mat hero".into(),
            "polyCube -w 1 -h 1 -d 1 -n box; move 3 0.5 0 box".into(),
            "duplicate -n box_tgt box; scale 1 2 1 box_tgt; blendShape box_tgt box; setKeyframe -t 1 -at box_tgt -v 0 box; setKeyframe -t 24 -at box_tgt -v 1 box; delete box_tgt".into(),
            "polySoftEdge -a 0 box".into(),
            "camera -n cam -p 0 1.5 6 -aim 0 1 0; spotLight -n spot; move 0 4 0 spot; rotate -90 0 0 spot".into(),
        ] {
            execute(&mut a, &m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let fbx = dir.join("rt.fbx");
        let msg = execute(&mut a, &format!("file -export {}", fbx.display())).unwrap();
        eprintln!("{msg}");
        assert!(msg.contains("1 skins") && msg.contains("1 blend-shape targets"), "{msg}");
        let mut b = App::new();
        b.scene.nodes.clear();
        let msg = execute(&mut b, &format!("file -import {}", fbx.display())).unwrap();
        eprintln!("{msg}");
        // Same skinned shape at several frames (skin + animation survive).
        for f in [1.0, 8.0, 16.0, 24.0] {
            a.set_time(f);
            crate::rig::evaluate(&mut a.scene);
            b.set_time(f);
            crate::rig::evaluate(&mut b.scene);
            let (pa, pb) = (world_pts(&a, "hero"), world_pts(&b, "hero"));
            assert_eq!(pa.len(), pb.len());
            let err = pa.iter().zip(&pb).map(|(x, y)| x.distance(*y)).fold(0.0, f32::max);
            if err > 2e-3 {
                for n in a.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Joint { .. })) {
                    let nb = b.scene.find_by_name(&n.name).unwrap();
                    let (wa, wb) = (crate::rig::world_pos(&a.scene, n.id), crate::rig::world_pos(&b.scene, nb));
                    let (qa, qb) = (crate::rig::world_rotation(&a.scene, n.id), crate::rig::world_rotation(&b.scene, nb));
                    if wa.distance(wb) > 1e-3 || qa.angle_between(qb) > 1e-3 {
                        eprintln!("joint {} pos {wa} vs {wb} rot diff {} | a r{} b r{} | anim a {} b {}", n.name, qa.angle_between(qb), n.rotate, b.scene.node(nb).unwrap().rotate, n.anim.len(), b.scene.node(nb).unwrap().anim.len());
                    }
                }
            }
            assert!(err < 2e-3, "frame {f}: max error {err}");
            let (ba, bb) = (world_pts(&a, "box"), world_pts(&b, "box"));
            let err = ba.iter().zip(&bb).map(|(x, y)| x.distance(*y)).fold(0.0, f32::max);
            assert!(err < 1e-3, "blend shape frame {f}: {err}");
        }
        // Material + texture, hard edges, joints, camera and light directions.
        let hero = b.scene.find_by_name("hero").unwrap();
        let m = b.scene.node(hero).unwrap().material.and_then(|m| b.material(m)).unwrap();
        assert_eq!(m.name, "skin_mat");
        assert!(m.color_map.as_deref().is_some_and(|p| p.ends_with("skin.png")));
        let bx = b.scene.find_by_name("box").unwrap();
        assert_eq!(b.scene.node(bx).unwrap().mesh().unwrap().hard_edges.as_ref().map(|h| h.len()), Some(12));
        let ja = a.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Joint { .. })).count();
        let jb = b.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Joint { .. })).count();
        assert_eq!(ja, jb);
        let fwd = |app: &App, name: &str| {
            let id = app.scene.nodes.iter().find(|n| n.name.starts_with(name) && matches!(n.shape, Shape::Camera(_) | Shape::Light(_))).unwrap().id;
            app.scene.world_matrix(id).transform_vector3(-Vec3::Z).normalize()
        };
        assert!(fwd(&a, "cam").distance(fwd(&b, "cam")) < 1e-3, "camera aims the same way");
        assert!(fwd(&a, "spot").distance(fwd(&b, "spot")) < 1e-3, "light aims the same way");
    }
}
#[cfg(test)]
mod xcheck {
    use crate::app::App;
    use crate::commands::execute;
    /// Compare the original scene with the FBX → assimp → glTF → import route.
    #[test]
    #[ignore]
    fn via_assimp() {
        let mut a = App::new();
        a.scene.nodes.clear();
        let anim = std::env::var("ANIM").is_ok();
        execute(&mut a, "playbackOptions -min 1 -max 24; character -style human -n hero").unwrap();
        execute(&mut a, if anim { "rigAnimate -anim wave -s 1 -e 24 hero" } else { "select hero; quickRig" }).unwrap();
        execute(&mut a, if anim { "file -export /tmp/rm_fbx_rt/x.fbx -bo" } else { "file -export /tmp/rm_fbx_rt/x.fbx" }).unwrap();
        let st = std::process::Command::new("assimp").args(["export", "/tmp/rm_fbx_rt/x.fbx", "/tmp/rm_fbx_rt/x.glb"]).output().unwrap();
        assert!(st.status.success());
        let mut b = App::new();
        b.scene.nodes.clear();
        let msg = execute(&mut b, "file -import /tmp/rm_fbx_rt/x.glb").unwrap();
        println!("{msg}");
        for f in [1.0, 12.0, 24.0] {
            a.set_time(f);
            crate::rig::evaluate(&mut a.scene);
            b.set_time(f);
            crate::rig::evaluate(&mut b.scene);
            let ha = a.scene.find_by_name("hero").unwrap();
            let pa = crate::rig::deformed_positions(&a.scene, ha).unwrap();
            let hb = b.scene.nodes.iter().find(|n| n.skin.is_some()).unwrap().id;
            let pb = crate::rig::deformed_positions(&b.scene, hb).unwrap();
            // assimp splits vertices; compare bounding boxes and nearest-point distances.
            let mut worst = 0.0f32;
            for p in pb.iter().step_by(7) {
                let d = pa.iter().map(|q| q.distance(*p)).fold(f32::MAX, f32::min);
                worst = worst.max(d);
            }
            println!("frame {f}: {} vs {} verts, worst nearest distance {worst:.4}", pa.len(), pb.len());
            let bb = |v: &[glam::Vec3]| v.iter().fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(a, c), p| (a.min(*p), c.max(*p)));
            println!("  a {:?}\n  b {:?}", bb(&pa), bb(&pb));
            let raw: Vec<glam::Vec3> = b.scene.node(hb).unwrap().mesh().unwrap().positions.clone();
            println!("  b raw {:?} world {:?}", bb(&raw), b.scene.world_matrix(hb).to_scale_rotation_translation());
            for nm in ["chest", "L_clavicle", "L_shoulder", "L_elbow", "L_wrist"] {
                if let (Some(x), Some(y)) = (a.scene.find_by_name(nm), b.scene.find_by_name(nm)) {
                    let n = a.scene.node(x).unwrap();
                    println!("  {nm}: {} vs {} | t {} r {} jo {} s {}", crate::rig::world_pos(&a.scene, x), crate::rig::world_pos(&b.scene, y), n.translate, n.rotate, n.joint_orient, n.scale);
                }
            }
        }
    }
}
