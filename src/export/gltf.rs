//! glTF 2.0 exporter: hierarchy, meshes (normals), PBR materials, skins, morph targets,
//! baked TRS / morph-weight animation, KHR_lights_punctual lights and cameras.

use super::{srgb_to_linear, Baked, ExportOptions, ExportSet};
use crate::app::App;
use crate::look::LightKind;
use crate::scene::{NodeId, Shape};
use glam::Vec3;
use serde_json::{json, Value};
use std::collections::HashMap;

const FLOAT: u32 = 5126;
const UINT: u32 = 5125;
const USHORT: u32 = 5123;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

#[derive(Default)]
struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Bin {
    fn view(&mut self, bytes: &[u8], target: Option<u32>, stride: Option<usize>) -> usize {
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }
        let mut v = json!({ "buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        if let Some(s) = stride {
            v["byteStride"] = json!(s);
        }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn floats(&mut self, data: &[f32], ty: &str, comps: usize, target: Option<u32>, minmax: bool) -> usize {
        let view = self.view(bytemuck::cast_slice(data), target, None);
        let count = data.len() / comps;
        let mut a = json!({ "bufferView": view, "componentType": FLOAT, "count": count, "type": ty });
        if minmax && count > 0 {
            let mut lo = vec![f32::MAX; comps];
            let mut hi = vec![f32::MIN; comps];
            for c in data.chunks(comps) {
                for i in 0..comps {
                    lo[i] = lo[i].min(c[i]);
                    hi[i] = hi[i].max(c[i]);
                }
            }
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let view = self.view(bytemuck::cast_slice(data), Some(ELEMENT_ARRAY_BUFFER), None);
        self.accessors.push(json!({ "bufferView": view, "componentType": UINT, "count": data.len(), "type": "SCALAR" }));
        self.accessors.len() - 1
    }

    fn joints(&mut self, data: &[u16]) -> usize {
        let view = self.view(bytemuck::cast_slice(data), Some(ARRAY_BUFFER), None);
        self.accessors.push(json!({ "bufferView": view, "componentType": USHORT, "count": data.len() / 4, "type": "VEC4" }));
        self.accessors.len() - 1
    }
}

fn lin3(c: [f32; 3]) -> [f32; 3] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])]
}

/// Build the glTF JSON document and its binary buffer.
pub fn build(app: &App, opts: &ExportOptions) -> Result<(Value, Vec<u8>, String), String> {
    let scene = &app.scene;
    let set = ExportSet::new(scene, &app.sel, opts.selected_only);
    if set.order.is_empty() {
        return Err("Nothing to export (select objects or turn off 'selection only').".into());
    }
    let index: HashMap<NodeId, usize> = set.order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut bin = Bin::default();
    let mut nodes: Vec<Value> = Vec::new();
    let mut meshes: Vec<Value> = Vec::new();
    let mut materials: Vec<Value> = Vec::new();
    let mut mat_index: HashMap<String, usize> = HashMap::new();
    let mut skins: Vec<Value> = Vec::new();
    let mut lights: Vec<Value> = Vec::new();
    let mut cameras: Vec<Value> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut skipped_ambient = 0;

    // Images / textures (embedded in the buffer).
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut tex_index: HashMap<String, Option<usize>> = HashMap::new();
    let mut texture_for = |name: &str, bin: &mut Bin, images: &mut Vec<Value>, textures: &mut Vec<Value>, notes: &mut Vec<String>| -> Option<usize> {
        if let Some(t) = tex_index.get(name) {
            return *t;
        }
        let (bytes, mime) = if name == crate::texture::CHECKER {
            (Some(crate::texture::TexImage::checker().to_png()), "image/png")
        } else if let Some(pair) = name.strip_prefix("mr:") {
            // glTF metallicRoughness: roughness in G, metalness in B.
            let (r, m) = pair.split_once('|').unwrap_or((pair, ""));
            (pack_metal_rough(r, m), "image/png")
        } else {
            let ext = std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            match ext.as_str() {
                "png" => (std::fs::read(name).ok(), "image/png"),
                "jpg" | "jpeg" => (std::fs::read(name).ok(), "image/jpeg"),
                // Other formats are re-encoded as PNG.
                _ => (crate::texture::TexImage::load(std::path::Path::new(name)).ok().map(|t| t.to_png()), "image/png"),
            }
        };
        let r = match bytes {
            Some(b) => {
                let view = bin.view(&b, None, None);
                let stem = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("texture".into());
                images.push(json!({ "name": stem, "bufferView": view, "mimeType": mime }));
                textures.push(json!({ "source": images.len() - 1, "sampler": 0 }));
                Some(textures.len() - 1)
            }
            None => {
                notes.push(format!("texture '{name}' could not be read"));
                None
            }
        };
        tex_index.insert(name.to_string(), r);
        r
    };
    let mut material_for = |id: NodeId, materials: &mut Vec<Value>, bin: &mut Bin, images: &mut Vec<Value>, textures: &mut Vec<Value>, notes: &mut Vec<String>| -> usize {
        let n = scene.node(id).unwrap();
        let (key, v) = match n.material.and_then(|m| app.material(m)) {
            Some(m) => {
                let c = lin3(m.color);
                let tex = m.color_map.as_ref().and_then(|t| texture_for(t, bin, images, textures, notes));
                // A colour texture replaces the colour (as in Maya), so the factor is white.
                let c = if tex.is_some() { [1.0, 1.0, 1.0] } else { c };
                let mut v = json!({
                    "name": m.name,
                    "pbrMetallicRoughness": {
                        "baseColorFactor": [c[0], c[1], c[2], 1.0 - m.transparency],
                        "metallicFactor": m.metalness,
                        "roughnessFactor": m.roughness.max(0.02),
                    },
                    "doubleSided": true,
                });
                if let Some(t) = tex {
                    v["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": t, "texCoord": 0 });
                }
                if let Some(t) = m.normal_map.as_ref().and_then(|t| texture_for(t, bin, images, textures, notes)) {
                    v["normalTexture"] = json!({ "index": t, "texCoord": 0 });
                }
                if let Some(t) = m.occlusion_map.as_ref().and_then(|t| texture_for(t, bin, images, textures, notes)) {
                    v["occlusionTexture"] = json!({ "index": t, "texCoord": 0 });
                }
                if let Some(t) = m.emission_map.as_ref().and_then(|t| texture_for(t, bin, images, textures, notes)) {
                    v["emissiveTexture"] = json!({ "index": t, "texCoord": 0 });
                    if !m.emission.iter().any(|x| *x > 0.0) {
                        v["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
                    }
                }
                if m.roughness_map.is_some() || m.metalness_map.is_some() {
                    let key = format!("mr:{}|{}", m.roughness_map.clone().unwrap_or_default(), m.metalness_map.clone().unwrap_or_default());
                    if let Some(t) = texture_for(&key, bin, images, textures, notes) {
                        v["pbrMetallicRoughness"]["metallicRoughnessTexture"] = json!({ "index": t, "texCoord": 0 });
                    }
                }
                if m.emission.iter().any(|x| *x > 0.0) {
                    v["emissiveFactor"] = json!(lin3(m.emission));
                }
                if m.transparency > 1e-3 {
                    v["alphaMode"] = json!("BLEND");
                }
                (format!("m{}", m.id), v)
            }
            None => {
                let c = lin3(n.color);
                let hex = format!("{:02x}{:02x}{:02x}", (n.color[0] * 255.0) as u8, (n.color[1] * 255.0) as u8, (n.color[2] * 255.0) as u8);
                (
                    format!("c{hex}"),
                    json!({
                        "name": format!("lambert1_{hex}"),
                        "pbrMetallicRoughness": { "baseColorFactor": [c[0], c[1], c[2], 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.8 },
                        "doubleSided": true,
                    }),
                )
            }
        };
        *mat_index.entry(key).or_insert_with(|| {
            materials.push(v);
            materials.len() - 1
        })
    };

    let mut n_meshes = 0;
    let mut n_skins = 0;
    let mut n_morph = 0;
    for &id in &set.order {
        let n = scene.node(id).unwrap();
        let (t, r, s) = set.local(scene, id);
        let mut node = json!({ "name": n.name });
        if t != Vec3::ZERO {
            node["translation"] = json!(t.to_array());
        }
        if r.angle_between(glam::Quat::IDENTITY) > 1e-7 {
            node["rotation"] = json!(r.to_array());
        }
        if s != Vec3::ONE {
            node["scale"] = json!(s.to_array());
        }
        let kids: Vec<usize> = set.order.iter().filter(|c| set.parent.get(c).copied().flatten() == Some(id)).map(|c| index[c]).collect();
        if !kids.is_empty() {
            node["children"] = json!(kids);
        }
        match &n.shape {
            Shape::Poly { mesh, .. } if scene.is_visible(id) && !mesh.faces.is_empty() => {
                let tri = mesh.triangulate();
                // Deduplicate corners by (vertex, normal, uv).
                let repeat = n.material.and_then(|m| app.material(m)).map(|m| glam::Vec2::from(m.repeat_uv)).unwrap_or(glam::Vec2::ONE);
                let has_uv = !tri.uv.is_empty();
                let mut remap: HashMap<(u32, [u32; 5]), u32> = HashMap::new();
                let mut corner_vert: Vec<u32> = Vec::new();
                let mut pos: Vec<f32> = Vec::new();
                let mut nrm: Vec<f32> = Vec::new();
                let mut uvs: Vec<f32> = Vec::new();
                let mut idx: Vec<u32> = Vec::with_capacity(tri.pos.len());
                for (i, &v) in tri.vert.iter().enumerate() {
                    let nn = tri.nrm[i].normalize_or(Vec3::Y);
                    let t = if has_uv { tri.uv[i] * repeat } else { glam::Vec2::ZERO };
                    let key = (v, [nn.x.to_bits(), nn.y.to_bits(), nn.z.to_bits(), t.x.to_bits(), t.y.to_bits()]);
                    let k = *remap.entry(key).or_insert_with(|| {
                        let p = mesh.positions[v as usize];
                        pos.extend_from_slice(&p.to_array());
                        nrm.extend_from_slice(&nn.to_array());
                        // glTF's UV origin is the top-left corner.
                        uvs.extend_from_slice(&[t.x, 1.0 - t.y]);
                        corner_vert.push(v);
                        (corner_vert.len() - 1) as u32
                    });
                    idx.push(k);
                }
                let mut attrs = json!({
                    "POSITION": bin.floats(&pos, "VEC3", 3, Some(ARRAY_BUFFER), true),
                    "NORMAL": bin.floats(&nrm, "VEC3", 3, Some(ARRAY_BUFFER), false),
                });
                if has_uv {
                    attrs["TEXCOORD_0"] = json!(bin.floats(&uvs, "VEC2", 2, Some(ARRAY_BUFFER), false));
                    // Tangents for normal-mapped materials (per-vertex, accumulated from triangles
                    // in glTF's UV space; w = bitangent sign).
                    if n.material.and_then(|m| app.material(m)).is_some_and(|m| m.normal_map.is_some()) {
                        let nv = corner_vert.len();
                        let mut tacc = vec![Vec3::ZERO; nv];
                        let mut bacc = vec![Vec3::ZERO; nv];
                        let p3 = |i: usize| Vec3::new(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]);
                        let t2 = |i: usize| glam::Vec2::new(uvs[i * 2], uvs[i * 2 + 1]);
                        for t in idx.chunks_exact(3) {
                            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                            let (e1, e2) = (p3(b) - p3(a), p3(c) - p3(a));
                            let (d1, d2) = (t2(b) - t2(a), t2(c) - t2(a));
                            let det = d1.x * d2.y - d2.x * d1.y;
                            if det.abs() < 1e-12 {
                                continue;
                            }
                            let r = 1.0 / det;
                            let tu = (e1 * d2.y - e2 * d1.y) * r;
                            let tv = (e2 * d1.x - e1 * d2.x) * r;
                            for k in [a, b, c] {
                                tacc[k] += tu;
                                bacc[k] += tv;
                            }
                        }
                        let mut tan: Vec<f32> = Vec::with_capacity(nv * 4);
                        for k in 0..nv {
                            let nn = Vec3::new(nrm[k * 3], nrm[k * 3 + 1], nrm[k * 3 + 2]);
                            let mut t = (tacc[k] - nn * nn.dot(tacc[k])).normalize_or_zero();
                            if t == Vec3::ZERO {
                                t = nn.any_orthonormal_vector();
                            }
                            let w = if nn.cross(t).dot(bacc[k]) < 0.0 { -1.0 } else { 1.0 };
                            tan.extend_from_slice(&[t.x, t.y, t.z, w]);
                        }
                        attrs["TANGENT"] = json!(bin.floats(&tan, "VEC4", 4, Some(ARRAY_BUFFER), false));
                    }
                }
                // Skin.
                if let Some(sk) = n.skin.as_ref().filter(|sk| sk.weights.len() == mesh.positions.len()) {
                    if sk.joints.iter().all(|j| index.contains_key(j)) {
                        let mut jd: Vec<u16> = Vec::with_capacity(corner_vert.len() * 4);
                        let mut wd: Vec<f32> = Vec::with_capacity(corner_vert.len() * 4);
                        for &v in &corner_vert {
                            let mut w: Vec<(u32, f32)> = sk.weights[v as usize].clone();
                            w.sort_by(|a, b| b.1.total_cmp(&a.1));
                            w.truncate(4);
                            let sum: f32 = w.iter().map(|x| x.1).sum::<f32>().max(1e-6);
                            for k in 0..4 {
                                let (j, x) = w.get(k).copied().unwrap_or((0, 0.0));
                                jd.push(j as u16);
                                wd.push(x / sum);
                            }
                        }
                        attrs["JOINTS_0"] = json!(bin.joints(&jd));
                        attrs["WEIGHTS_0"] = json!(bin.floats(&wd, "VEC4", 4, Some(ARRAY_BUFFER), false));
                        let ibm: Vec<f32> = sk.inv_bind.iter().flat_map(|m| (*m * sk.bind_shape).to_cols_array()).collect();
                        let ibm_acc = bin.floats(&ibm, "MAT4", 16, None, false);
                        let joints: Vec<usize> = sk.joints.iter().map(|j| index[j]).collect();
                        skins.push(json!({ "name": format!("skinCluster_{}", n.name), "joints": joints, "inverseBindMatrices": ibm_acc }));
                        node["skin"] = json!(skins.len() - 1);
                        n_skins += 1;
                    } else {
                        notes.push(format!("{}: skin skipped (joints not exported)", n.name));
                    }
                }
                let mut prim = json!({ "attributes": attrs, "indices": bin.indices(&idx), "mode": 4 });
                prim["material"] = json!(material_for(id, &mut materials, &mut bin, &mut images, &mut textures, &mut notes));
                let mut mesh_v = json!({ "name": format!("{}Shape", n.name), "primitives": [] });
                // Morph targets (blend shapes).
                if let Some(bs) = n.blend.as_ref().filter(|b| !b.targets.is_empty() && b.targets.iter().all(|t| t.deltas.len() == mesh.positions.len())) {
                    let mut targets = Vec::new();
                    for t in &bs.targets {
                        let d: Vec<f32> = corner_vert.iter().flat_map(|v| t.deltas[*v as usize].to_array()).collect();
                        targets.push(json!({ "POSITION": bin.floats(&d, "VEC3", 3, Some(ARRAY_BUFFER), true) }));
                    }
                    prim["targets"] = json!(targets);
                    mesh_v["weights"] = json!(bs.weights);
                    mesh_v["extras"] = json!({ "targetNames": bs.targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>() });
                    n_morph += bs.targets.len();
                }
                if !n.clusters.is_empty() {
                    notes.push(format!("{}: cluster deformers are not exported (baked shape only in USD)", n.name));
                }
                mesh_v["primitives"] = json!([prim]);
                meshes.push(mesh_v);
                node["mesh"] = json!(meshes.len() - 1);
                n_meshes += 1;
            }
            Shape::Light(l) => {
                let c = lin3(l.color);
                let mut lv = json!({ "name": n.name, "color": c, "intensity": l.intensity });
                match l.kind {
                    LightKind::Directional => lv["type"] = json!("directional"),
                    LightKind::Point => lv["type"] = json!("point"),
                    LightKind::Spot => {
                        lv["type"] = json!("spot");
                        let inner = (l.cone * 0.5).to_radians().min(1.55);
                        let outer = ((l.cone * 0.5) + l.penumbra.max(0.1)).to_radians().clamp(inner + 1e-3, std::f32::consts::FRAC_PI_2);
                        lv["spot"] = json!({ "innerConeAngle": inner.min(outer - 1e-3), "outerConeAngle": outer });
                    }
                    LightKind::Ambient | LightKind::Area => {
                        skipped_ambient += 1;
                        lv = Value::Null;
                    }
                }
                if !lv.is_null() {
                    lights.push(lv);
                    node["extensions"] = json!({ "KHR_lights_punctual": { "light": lights.len() - 1 } });
                }
            }
            Shape::Camera(c) => {
                cameras.push(json!({
                    "name": n.name,
                    "type": "perspective",
                    "perspective": { "yfov": c.fov_y().to_radians(), "znear": c.near.max(1e-4), "zfar": c.far, "aspectRatio": 1.5 }
                }));
                node["camera"] = json!(cameras.len() - 1);
            }
            _ => {}
        }
        nodes.push(node);
    }

    // Baked animation.
    let baked = Baked::sample(app, &set, opts, false);
    let mut animations = Vec::new();
    let mut n_channels = 0;
    if !baked.frames.is_empty() {
        let fps = scene.fps.max(1.0);
        let times: Vec<f32> = baked.frames.iter().map(|f| (f - baked.frames[0]) / fps).collect();
        let time_acc = bin.floats(&times, "SCALAR", 1, None, true);
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for &id in &set.order {
            if let Some(trs) = baked.trs.get(&id) {
                let node = index[&id];
                let t: Vec<f32> = trs.iter().flat_map(|x| x.0.to_array()).collect();
                let mut prev = trs[0].1;
                let r: Vec<f32> = trs
                    .iter()
                    .flat_map(|x| {
                        let q = if prev.dot(x.1) < 0.0 { -x.1 } else { x.1 };
                        prev = q;
                        q.to_array()
                    })
                    .collect();
                let s: Vec<f32> = trs.iter().flat_map(|x| x.2.to_array()).collect();
                for (path, data, ty, comps) in [("translation", t, "VEC3", 3), ("rotation", r, "VEC4", 4), ("scale", s, "VEC3", 3)] {
                    let varies = data.chunks(comps).collect::<Vec<_>>().windows(2).any(|w| w[0].iter().zip(w[1]).any(|(a, b)| (a - b).abs() > 1e-6));
                    if !varies {
                        continue;
                    }
                    let out = bin.floats(&data, ty, comps, None, false);
                    samplers.push(json!({ "input": time_acc, "output": out, "interpolation": "LINEAR" }));
                    channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": node, "path": path } }));
                    n_channels += 1;
                }
            }
            if let Some(ws) = baked.weights.get(&id)
                && nodes[index[&id]].get("mesh").is_some()
            {
                let data: Vec<f32> = ws.iter().flatten().copied().collect();
                let out = bin.floats(&data, "SCALAR", 1, None, false);
                samplers.push(json!({ "input": time_acc, "output": out, "interpolation": "LINEAR" }));
                channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": index[&id], "path": "weights" } }));
                n_channels += 1;
            }
        }
        if !channels.is_empty() {
            animations.push(json!({ "name": "take1", "samplers": samplers, "channels": channels }));
        }
    }

    let roots: Vec<usize> = set.order.iter().filter(|id| set.parent[id].is_none()).map(|id| index[id]).collect();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "Rust Maya glTF exporter" },
        "scene": 0,
        "scenes": [{ "name": "Scene", "nodes": roots }],
        "nodes": nodes,
    });
    if !textures.is_empty() {
        doc["samplers"] = json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
    }
    for (k, v) in [("meshes", meshes), ("materials", materials), ("skins", skins), ("cameras", cameras), ("animations", animations), ("images", images), ("textures", textures)] {
        if !v.is_empty() {
            doc[k] = json!(v);
        }
    }
    let n_lights = lights.len();
    if !lights.is_empty() {
        doc["extensionsUsed"] = json!(["KHR_lights_punctual"]);
        doc["extensions"] = json!({ "KHR_lights_punctual": { "lights": lights } });
    }
    while bin.data.len() % 4 != 0 {
        bin.data.push(0);
    }
    if !bin.data.is_empty() {
        doc["buffers"] = json!([{ "byteLength": bin.data.len() }]);
        doc["bufferViews"] = json!(bin.views);
        doc["accessors"] = json!(bin.accessors);
    }
    if skipped_ambient > 0 {
        notes.push(format!("{skipped_ambient} ambient light(s) skipped (not in glTF)"));
    }
    let mut summary = format!(
        "glTF: {} nodes, {n_meshes} meshes, {} materials, {n_skins} skins, {n_morph} morph targets, {n_channels} animation channels ({} frames), {n_lights} lights, {} cameras",
        set.order.len(),
        doc["materials"].as_array().map(|a| a.len()).unwrap_or(0),
        baked.frames.len(),
        doc["cameras"].as_array().map(|a| a.len()).unwrap_or(0),
    );
    if !notes.is_empty() {
        summary.push_str(&format!(" — note: {}", notes.join("; ")));
    }
    Ok((doc, bin.data, summary))
}

/// Binary glTF (.glb).
/// Combine greyscale roughness / metalness maps into a glTF metallicRoughness PNG
/// (G = roughness, B = metalness; a missing map is white so the factor applies unchanged).
fn pack_metal_rough(rough: &str, metal: &str) -> Option<Vec<u8>> {
    let load = |p: &str| if p.is_empty() { None } else { image::open(p).ok().map(|i| i.to_luma8()) };
    let (r, m) = (load(rough), load(metal));
    let (w, h) = r.as_ref().or(m.as_ref()).map(|i| i.dimensions())?;
    let fit = |img: Option<image::GrayImage>| img.map(|i| if i.dimensions() == (w, h) { i } else { image::imageops::resize(&i, w, h, image::imageops::FilterType::Triangle) });
    let (r, m) = (fit(r), fit(m));
    let out = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([0, r.as_ref().map(|i| i.get_pixel(x, y)[0]).unwrap_or(255), m.as_ref().map(|i| i.get_pixel(x, y)[0]).unwrap_or(255)]));
    let mut buf = std::io::Cursor::new(Vec::new());
    out.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

pub fn export(app: &App, opts: &ExportOptions) -> Result<(Vec<u8>, String), String> {
    let (doc, data, summary) = build(app, opts)?;
    let mut js = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + if data.is_empty() { 0 } else { 8 + data.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&js);
    if !data.is_empty() {
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&data);
    }
    Ok((out, summary))
}

/// Read back a .glb (used by tests): returns the JSON and the BIN chunk.
#[cfg(test)]
pub fn parse_glb(b: &[u8]) -> (Value, Vec<u8>) {
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(u(0), 0x4654_6C67);
    assert_eq!(u(8), b.len());
    let jl = u(12);
    let doc: Value = serde_json::from_slice(&b[20..20 + jl]).unwrap();
    let bin = if b.len() > 20 + jl { b[28 + jl..28 + jl + u(20 + jl)].to_vec() } else { Vec::new() };
    (doc, bin)
}

/// Text glTF (.gltf) with the buffer embedded as a base64 data URI.
pub fn export_embedded(app: &App, opts: &ExportOptions) -> Result<(Vec<u8>, String), String> {
    let (mut doc, data, summary) = build(app, opts)?;
    if !data.is_empty() {
        doc["buffers"][0]["uri"] = json!(format!("data:application/octet-stream;base64,{}", base64(&data)));
    }
    Ok((serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?, summary))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    fn floats(doc: &Value, bin: &[u8], acc: usize) -> Vec<f32> {
        let a = &doc["accessors"][acc];
        let v = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let off = v["byteOffset"].as_u64().unwrap() as usize;
        let len = v["byteLength"].as_u64().unwrap() as usize;
        bytemuck::cast_slice::<u8, f32>(&bin[off..off + len]).to_vec()
    }

    #[test]
    fn export_rigged_animated_scene() {
        let mut app = App::new();
        app.scene.nodes.clear();
        for m in [
            "select -cl",
            "joint -p 0 0 0 -n root",
            "joint -p 0 2 0 -n mid",
            "joint -p 0 4 0 -n tip",
            "polyCylinder -r 0.3 -h 4 -sh 6 -n tube",
            "move 0 2 0",
            "skinCluster root tube",
            "polyCube -n box",
            "duplicate -n box_tall",
            "scale 1 2 1 box_tall",
            "makeIdentity box_tall",
            "hide box_tall",
            "blendShape box_tall box",
            "setKeyframe -t 1 -at box_tall -v 0 box",
            "setKeyframe -t 10 -at box_tall -v 1 box",
            "setKeyframe -t 1 -at rz -v 0 mid",
            "setKeyframe -t 10 -at rz -v 45 mid",
            "shadingNode -asShader standardSurface -n chrome -rgb 0.9 0.9 0.9",
            "setAttr chrome.metalness 1",
            "hyperShade -assign chrome tube",
            "directionalLight -n sun -sh on",
            "spotLight -n spot -ca 30",
            "ambientLight",
            "camera -n cam1 -p 0 2 10 -aim 0 2 0",
            "playbackOptions -min 1 -max 10",
        ] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let opts = ExportOptions::from_app(&app, false);
        let (bytes, summary) = export(&app, &opts).unwrap();
        eprintln!("{summary}");
        let (doc, bin) = parse_glb(&bytes);
        assert_eq!(doc["asset"]["version"], "2.0");
        assert_eq!(doc["buffers"][0]["byteLength"].as_u64().unwrap() as usize, bin.len());
        let nodes = doc["nodes"].as_array().unwrap();
        let find = |n: &str| nodes.iter().position(|x| x["name"] == n).unwrap();
        // Skin with 3 joints, IBM accessor of 3 MAT4.
        let tube = &nodes[find("tube")];
        let skin = &doc["skins"][tube["skin"].as_u64().unwrap() as usize];
        assert_eq!(skin["joints"].as_array().unwrap().len(), 3);
        let ibm = floats(&doc, &bin, skin["inverseBindMatrices"].as_u64().unwrap() as usize);
        assert_eq!(ibm.len(), 48);
        // Material is metallic.
        let mesh = &doc["meshes"][tube["mesh"].as_u64().unwrap() as usize];
        let mat = &doc["materials"][mesh["primitives"][0]["material"].as_u64().unwrap() as usize];
        assert_eq!(mat["name"], "chrome");
        assert_eq!(mat["pbrMetallicRoughness"]["metallicFactor"], 1.0);
        // Weights sum to 1.
        let wacc = mesh["primitives"][0]["attributes"]["WEIGHTS_0"].as_u64().unwrap() as usize;
        for w in floats(&doc, &bin, wacc).chunks(4) {
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        // Morph target on box with a target name.
        let boxm = &doc["meshes"][nodes[find("box")]["mesh"].as_u64().unwrap() as usize];
        assert_eq!(boxm["extras"]["targetNames"][0], "box_tall");
        assert_eq!(boxm["primitives"][0]["targets"].as_array().unwrap().len(), 1);
        // Hidden target mesh is a node without mesh.
        assert!(nodes[find("box_tall")].get("mesh").is_none());
        // Animation: rotation of mid + morph weights, 10 frames.
        let anim = &doc["animations"][0];
        let chans = anim["channels"].as_array().unwrap();
        assert!(chans.iter().any(|c| c["target"]["path"] == "rotation" && c["target"]["node"] == find("mid")));
        assert!(chans.iter().any(|c| c["target"]["path"] == "weights"));
        let t = floats(&doc, &bin, anim["samplers"][0]["input"].as_u64().unwrap() as usize);
        assert_eq!(t.len(), 10);
        assert!((t[9] - 9.0 / app.scene.fps).abs() < 1e-5);
        // Lights: sun + spot (ambient skipped), camera.
        assert_eq!(doc["extensions"]["KHR_lights_punctual"]["lights"].as_array().unwrap().len(), 2);
        assert!(nodes[find("cam1")]["camera"].is_number());
        assert!(summary.contains("ambient"));
        // Selection-only export keeps just the subtree.
        execute(&mut app, "select cam1").unwrap();
        let (b2, _) = export(&app, &ExportOptions::from_app(&app, true)).unwrap();
        let (d2, _) = parse_glb(&b2);
        assert_eq!(d2["nodes"].as_array().unwrap().len(), 1);
        std::fs::write(std::env::temp_dir().join("rust_maya_test.glb"), &bytes).unwrap();
        let (txt, _) = export_embedded(&app, &opts).unwrap();
        std::fs::write(std::env::temp_dir().join("rust_maya_test.gltf"), &txt).unwrap();
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }
}
