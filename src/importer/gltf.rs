//! glTF 2.0 importer (.glb / .gltf with embedded or external buffers and images).
//! Brings in the node hierarchy, meshes (with UVs), PBR materials and base-colour textures,
//! skins (as joints + skinCluster), morph targets (as blend shapes), animation (as keyframes),
//! KHR_lights_punctual lights and perspective cameras.

use crate::app::App;
use crate::look::{CameraShape, Light, LightKind, MatKind};
use crate::mesh::Mesh;
use crate::rig::{BlendShape, Skin};
use crate::scene::{Channel, Curve, NodeId, Shape, Tangent};
use glam::{Mat4, Quat, Vec2, Vec3};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Report {
    pub nodes: usize,
    pub meshes: usize,
    pub skins: usize,
    pub morphs: usize,
    pub anim_curves: usize,
    pub lights: usize,
    pub cameras: usize,
    pub textures: usize,
    pub notes: Vec<String>,
    pub roots: Vec<NodeId>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Imported glTF: {} nodes, {} meshes, {} skins, {} blend-shape targets, {} animation curves, {} lights, {} cameras, {} textures",
            self.nodes, self.meshes, self.skins, self.morphs, self.anim_curves, self.lights, self.cameras, self.textures
        );
        if !self.notes.is_empty() {
            s.push_str(&format!(" — {}", self.notes.join("; ")));
        }
        s
    }
}

pub(crate) fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return Err("invalid base64".into()),
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

struct Doc {
    json: Value,
    buffers: Vec<Vec<u8>>,
    dir: PathBuf,
}

impl Doc {
    fn load(path: &Path) -> Result<Doc, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let (json, glb_bin): (Value, Option<Vec<u8>>) = if bytes.len() >= 12 && &bytes[0..4] == b"glTF" {
            let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
            let mut off = 12;
            let mut json = None;
            let mut bin = None;
            while off + 8 <= bytes.len() {
                let len = u(off);
                let ty = u(off + 4);
                let data = bytes.get(off + 8..off + 8 + len).ok_or("truncated GLB chunk")?;
                match ty {
                    0x4E4F_534A => json = Some(serde_json::from_slice(data).map_err(|e| e.to_string())?),
                    0x004E_4942 => bin = Some(data.to_vec()),
                    _ => {}
                }
                off += 8 + len;
            }
            (json.ok_or("GLB has no JSON chunk")?, bin)
        } else {
            (serde_json::from_slice(&bytes).map_err(|e| format!("not a glTF file: {e}"))?, None)
        };
        if json["asset"]["version"].as_str().is_some_and(|v| !v.starts_with('2')) {
            return Err("only glTF 2.0 is supported".into());
        }
        let mut buffers = Vec::new();
        for (i, b) in json["buffers"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let data = match b["uri"].as_str() {
                None => glb_bin.clone().ok_or(format!("buffer {i} has no data"))?,
                Some(uri) if uri.starts_with("data:") => base64_decode(uri.split_once(',').ok_or("bad data URI")?.1)?,
                Some(uri) => std::fs::read(dir.join(percent_decode(uri))).map_err(|e| format!("{uri}: {e}"))?,
            };
            buffers.push(data);
        }
        Ok(Doc { json, buffers, dir })
    }

    fn view(&self, v: usize) -> Result<(&[u8], usize), String> {
        let bv = &self.json["bufferViews"][v];
        let buf = &self.buffers[bv["buffer"].as_u64().ok_or("bufferView without buffer")? as usize];
        let off = bv["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = bv["byteLength"].as_u64().ok_or("bufferView without length")? as usize;
        Ok((buf.get(off..off + len).ok_or("bufferView out of range")?, bv["byteStride"].as_u64().unwrap_or(0) as usize))
    }

    /// Read an accessor as floats (normalised integers are converted), `comps` per element.
    fn floats(&self, a: usize) -> Result<(Vec<f32>, usize), String> {
        let acc = &self.json["accessors"][a];
        let count = acc["count"].as_u64().ok_or("accessor without count")? as usize;
        let comps = match acc["type"].as_str().unwrap_or("SCALAR") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            t => return Err(format!("unsupported accessor type {t}")),
        };
        let ct = acc["componentType"].as_u64().unwrap_or(5126);
        let norm = acc["normalized"].as_bool().unwrap_or(false);
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            _ => 4,
        };
        let mut out = vec![0.0f32; count * comps];
        if let Some(v) = acc["bufferView"].as_u64() {
            let (data, stride) = self.view(v as usize)?;
            let off = acc["byteOffset"].as_u64().unwrap_or(0) as usize;
            let stride = if stride == 0 { csize * comps } else { stride };
            for i in 0..count {
                for c in 0..comps {
                    let o = off + i * stride + c * csize;
                    let b = data.get(o..o + csize).ok_or("accessor out of range")?;
                    out[i * comps + c] = match ct {
                        5126 => f32::from_le_bytes(b.try_into().unwrap()),
                        5125 => u32::from_le_bytes(b.try_into().unwrap()) as f32,
                        5123 => {
                            let x = u16::from_le_bytes(b.try_into().unwrap()) as f32;
                            if norm { x / 65535.0 } else { x }
                        }
                        5122 => {
                            let x = i16::from_le_bytes(b.try_into().unwrap()) as f32;
                            if norm { (x / 32767.0).max(-1.0) } else { x }
                        }
                        5121 => {
                            let x = b[0] as f32;
                            if norm { x / 255.0 } else { x }
                        }
                        5120 => {
                            let x = b[0] as i8 as f32;
                            if norm { (x / 127.0).max(-1.0) } else { x }
                        }
                        _ => return Err(format!("unsupported componentType {ct}")),
                    };
                }
            }
        }
        // Sparse accessors: overwrite the listed elements.
        if let Some(sp) = acc.get("sparse") {
            let n = sp["count"].as_u64().unwrap_or(0) as usize;
            let (idata, _) = self.view(sp["indices"]["bufferView"].as_u64().ok_or("sparse indices")? as usize)?;
            let ioff = sp["indices"]["byteOffset"].as_u64().unwrap_or(0) as usize;
            let ict = sp["indices"]["componentType"].as_u64().unwrap_or(5125);
            let (vdata, _) = self.view(sp["values"]["bufferView"].as_u64().ok_or("sparse values")? as usize)?;
            let voff = sp["values"]["byteOffset"].as_u64().unwrap_or(0) as usize;
            for k in 0..n {
                let idx = match ict {
                    5121 => idata[ioff + k] as usize,
                    5123 => u16::from_le_bytes(idata[ioff + k * 2..ioff + k * 2 + 2].try_into().unwrap()) as usize,
                    _ => u32::from_le_bytes(idata[ioff + k * 4..ioff + k * 4 + 4].try_into().unwrap()) as usize,
                };
                for c in 0..comps {
                    let o = voff + (k * comps + c) * 4;
                    if let (Some(b), true) = (vdata.get(o..o + 4), ct == 5126) {
                        out[idx * comps + c] = f32::from_le_bytes(b.try_into().unwrap());
                    }
                }
            }
        }
        Ok((out, comps))
    }

    fn indices(&self, a: usize) -> Result<Vec<u32>, String> {
        Ok(self.floats(a)?.0.into_iter().map(|x| x as u32).collect())
    }

    /// Image bytes and a file extension.
    fn image(&self, i: usize) -> Result<(Vec<u8>, &'static str), String> {
        let img = &self.json["images"][i];
        let mime = img["mimeType"].as_str().unwrap_or("");
        let ext_of = |s: &str| if s.contains("jpeg") || s.ends_with(".jpg") || s.ends_with(".jpeg") { "jpg" } else { "png" };
        if let Some(v) = img["bufferView"].as_u64() {
            return Ok((self.view(v as usize)?.0.to_vec(), ext_of(mime)));
        }
        match img["uri"].as_str() {
            Some(uri) if uri.starts_with("data:") => {
                let (head, data) = uri.split_once(',').ok_or("bad image data URI")?;
                Ok((base64_decode(data)?, ext_of(head)))
            }
            Some(uri) => Ok((std::fs::read(self.dir.join(percent_decode(uri))).map_err(|e| format!("{uri}: {e}"))?, ext_of(&uri.to_ascii_lowercase()))),
            None => Err("image without data".into()),
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn node_trs(n: &Value) -> (Vec3, Quat, Vec3) {
    if let Some(m) = n["matrix"].as_array() {
        let v: Vec<f32> = m.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
        if v.len() == 16 {
            let (s, r, t) = Mat4::from_cols_slice(&v).to_scale_rotation_translation();
            return (t, r, s);
        }
    }
    let f = |k: &str, n_: usize, d: f32| -> Vec<f32> { n[k].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(d as f64) as f32).collect()).unwrap_or(vec![d; n_]) };
    let t = f("translation", 3, 0.0);
    let s = f("scale", 3, 1.0);
    let r = n["rotation"].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect::<Vec<_>>()).unwrap_or(vec![0.0, 0.0, 0.0, 1.0]);
    (Vec3::new(t[0], t[1], t[2]), Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(), Vec3::new(s[0], s[1], s[2]))
}

/// Unwrap Euler angles so consecutive keys don't jump by 360°.
fn unwrap(prev: Vec3, mut e: Vec3) -> Vec3 {
    for i in 0..3 {
        while e[i] - prev[i] > 180.0 {
            e[i] -= 360.0;
        }
        while e[i] - prev[i] < -180.0 {
            e[i] += 360.0;
        }
    }
    e
}

pub fn import(app: &mut App, path: &Path, take: Option<&str>) -> Result<Report, String> {
    let doc = Doc::load(path)?;
    let j = &doc.json;
    let mut rep = Report { nodes: 0, meshes: 0, skins: 0, morphs: 0, anim_curves: 0, lights: 0, cameras: 0, textures: 0, notes: Vec::new(), roots: Vec::new() };
    for ext in j["extensionsRequired"].as_array().cloned().unwrap_or_default() {
        let e = ext.as_str().unwrap_or("");
        if !matches!(e, "KHR_lights_punctual" | "KHR_materials_emissive_strength" | "KHR_texture_transform") {
            return Err(format!("this file requires the unsupported extension {e}"));
        }
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("gltf".into());
    let gnodes = j["nodes"].as_array().cloned().unwrap_or_default();
    // Which nodes are joints.
    let mut is_joint = vec![false; gnodes.len()];
    for sk in j["skins"].as_array().cloned().unwrap_or_default() {
        for jn in sk["joints"].as_array().cloned().unwrap_or_default() {
            if let Some(i) = jn.as_u64() {
                is_joint[i as usize] = true;
            }
        }
    }
    app.begin_batch();
    app.checkpoint();

    // ---- materials (and textures saved next to the file)
    let tex_dir = doc.dir.join(format!("{stem}_textures"));
    let mut image_paths: HashMap<usize, Option<String>> = HashMap::new();
    let mut mats: Vec<u32> = Vec::new();
    for (mi, m) in j["materials"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        let pbr = &m["pbrMetallicRoughness"];
        let fac: Vec<f32> = pbr["baseColorFactor"].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(1.0) as f32).collect()).unwrap_or(vec![1.0; 4]);
        let name = m["name"].as_str().map(String::from).unwrap_or(format!("{stem}_mat{mi}"));
        let id = app.create_material(MatKind::StandardSurface, Some(&name));
        let mut mat = app.material(id).unwrap().clone();
        mat.color = [linear_to_srgb(fac[0]), linear_to_srgb(fac[1]), linear_to_srgb(fac[2])];
        mat.transparency = if m["alphaMode"].as_str() == Some("BLEND") { (1.0 - fac.get(3).copied().unwrap_or(1.0)).clamp(0.0, 1.0) } else { 0.0 };
        mat.metalness = pbr["metallicFactor"].as_f64().unwrap_or(1.0) as f32;
        mat.roughness = pbr["roughnessFactor"].as_f64().unwrap_or(1.0) as f32;
        mat.specular = 1.0;
        if let Some(e) = m["emissiveFactor"].as_array() {
            let e: Vec<f32> = e.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
            if e.len() == 3 {
                mat.emission = [linear_to_srgb(e[0]), linear_to_srgb(e[1]), linear_to_srgb(e[2])];
            }
        }
        // Save a glTF image next to the file once; returns its path.
        let mut image_file = |tex: &serde_json::Value, rep: &mut Report| -> Option<String> {
            let ti = tex["index"].as_u64()?;
            let src = j["textures"][ti as usize]["source"].as_u64()? as usize;
            image_paths
                .entry(src)
                .or_insert_with(|| match doc.image(src) {
                    Ok((bytes, ext)) => {
                        let _ = std::fs::create_dir_all(&tex_dir);
                        let nm = j["images"][src]["name"].as_str().map(|s| s.replace(['/', '\\', ':'], "_")).unwrap_or(format!("image{src}"));
                        let file = tex_dir.join(format!("{nm}.{ext}"));
                        std::fs::write(&file, bytes).ok().map(|_| file.display().to_string())
                    }
                    Err(e) => {
                        rep.notes.push(format!("image {src}: {e}"));
                        None
                    }
                })
                .clone()
        };
        if let Some(p) = image_file(&pbr["baseColorTexture"], &mut rep) {
            // glTF multiplies texture × factor; Maya-style maps replace the colour.
            mat.color_map = Some(p);
            rep.textures += 1;
        }
        if let Some(p) = image_file(&m["normalTexture"], &mut rep) {
            mat.normal_map = Some(p);
            rep.textures += 1;
        }
        if let Some(p) = image_file(&m["emissiveTexture"], &mut rep) {
            mat.emission_map = Some(p);
            if m.get("emissiveFactor").is_none() {
                // glTF's default factor is black, but a map without a factor is meant to glow.
                mat.emission = [1.0, 1.0, 1.0];
            }
            rep.textures += 1;
        }
        // Occlusion is in R (often packed with metallic-roughness): extract it.
        if let Some(p) = image_file(&m["occlusionTexture"], &mut rep) {
            match image::open(&p) {
                Ok(img) => {
                    let rgb = img.to_rgb8();
                    let g = image::GrayImage::from_fn(rgb.width(), rgb.height(), |x, y| image::Luma([rgb.get_pixel(x, y)[0]]));
                    let out = format!("{}_occlusion.png", std::path::Path::new(&p).with_extension("").display());
                    if g.save(&out).is_ok() {
                        mat.occlusion_map = Some(out);
                        rep.textures += 1;
                    }
                }
                Err(e) => rep.notes.push(format!("{name}: occlusion map unreadable ({e})")),
            }
        }
        // glTF packs roughness in G and metalness in B: split into two greyscale maps.
        if let Some(p) = image_file(&pbr["metallicRoughnessTexture"], &mut rep) {
            match image::open(&p) {
                Ok(img) => {
                    let rgb = img.to_rgb8();
                    let stem_p = std::path::Path::new(&p).with_extension("");
                    let chan = |c: usize, suffix: &str| -> Option<String> {
                        let g = image::GrayImage::from_fn(rgb.width(), rgb.height(), |x, y| image::Luma([rgb.get_pixel(x, y)[c]]));
                        let out = format!("{}_{suffix}.png", stem_p.display());
                        g.save(&out).ok().map(|_| out)
                    };
                    mat.roughness_map = chan(1, "roughness");
                    mat.metalness_map = chan(2, "metalness");
                    rep.textures += 2;
                }
                Err(e) => rep.notes.push(format!("{name}: metallic-roughness map unreadable ({e})")),
            }
        }
        *app.material_mut(id).unwrap() = mat;
        mats.push(id);
    }

    // ---- nodes (create all, then parent)
    let mut ids: Vec<NodeId> = Vec::with_capacity(gnodes.len());
    for (i, n) in gnodes.iter().enumerate() {
        let name = n["name"].as_str().filter(|s| !s.is_empty()).map(|s| s.replace([' ', '.', ':', '|'], "_")).unwrap_or(format!("{}{i}", if is_joint[i] { "joint" } else { "node" }));
        let shape = if is_joint[i] { Shape::Joint { radius: 0.05 } } else { Shape::Group };
        let id = app.scene.add_node(&name, shape, None);
        app.apply_name(id, Some(&name));
        let (t, r, s) = node_trs(n);
        if let Some(nd) = app.scene.node_mut(id) {
            nd.translate = t;
            nd.rotate = crate::rig::quat_euler(r);
            nd.scale = s;
        }
        ids.push(id);
    }
    for (i, n) in gnodes.iter().enumerate() {
        for c in n["children"].as_array().cloned().unwrap_or_default() {
            if let Some(ci) = c.as_u64().map(|x| x as usize).filter(|x| *x < ids.len())
                && let Some(nd) = app.scene.node_mut(ids[ci])
            {
                nd.parent = Some(ids[i]);
            }
        }
    }
    rep.nodes = ids.len();
    // Joint radius from the skeleton's size.
    let jpos: Vec<Vec3> = ids.iter().zip(&is_joint).filter(|(_, j)| **j).map(|(id, _)| crate::rig::world_pos(&app.scene, *id)).collect();
    if jpos.len() > 1 {
        let (lo, hi) = jpos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let r = ((hi - lo).length() * 0.012).max(1e-3);
        for (id, j) in ids.iter().zip(&is_joint) {
            if *j && let Some(Shape::Joint { radius }) = app.scene.node_mut(*id).map(|n| &mut n.shape) {
                *radius = r;
            }
        }
    }

    // ---- lights & cameras
    let glights = j["extensions"]["KHR_lights_punctual"]["lights"].as_array().cloned().unwrap_or_default();
    for (i, n) in gnodes.iter().enumerate() {
        if let Some(li) = n["extensions"]["KHR_lights_punctual"]["light"].as_u64()
            && let Some(l) = glights.get(li as usize)
        {
            let col: Vec<f32> = l["color"].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(1.0) as f32).collect()).unwrap_or(vec![1.0; 3]);
            let kind = match l["type"].as_str() {
                Some("directional") => LightKind::Directional,
                Some("spot") => LightKind::Spot,
                _ => LightKind::Point,
            };
            let inner = l["spot"]["innerConeAngle"].as_f64().unwrap_or(0.0) as f32;
            let outer = l["spot"]["outerConeAngle"].as_f64().unwrap_or(std::f64::consts::FRAC_PI_4) as f32;
            let light = Light {
                kind,
                color: [linear_to_srgb(col[0]), linear_to_srgb(col[1]), linear_to_srgb(col[2])],
                intensity: l["intensity"].as_f64().unwrap_or(1.0) as f32,
                cone: (inner.max(0.01) * 2.0).to_degrees(),
                penumbra: (outer - inner).max(0.0).to_degrees(),
                shadows: false,
            };
            set_shape(app, ids[i], Shape::Light(light));
            rep.lights += 1;
        }
        if let Some(ci) = n["camera"].as_u64() {
            let c = &j["cameras"][ci as usize];
            if c["type"].as_str() == Some("perspective") {
                let yfov = c["perspective"]["yfov"].as_f64().unwrap_or(0.8) as f32;
                let cam = CameraShape { focal: 12.0 / (yfov / 2.0).tan(), near: c["perspective"]["znear"].as_f64().unwrap_or(0.1) as f32, far: c["perspective"]["zfar"].as_f64().unwrap_or(10000.0) as f32, ..Default::default() };
                set_shape(app, ids[i], Shape::Camera(cam));
                rep.cameras += 1;
            } else {
                rep.notes.push("orthographic camera imported as a group".into());
            }
        }
    }

    // ---- meshes
    let gmeshes = j["meshes"].as_array().cloned().unwrap_or_default();
    for (i, n) in gnodes.iter().enumerate() {
        let Some(mi) = n["mesh"].as_u64() else { continue };
        let Some(gm) = gmeshes.get(mi as usize) else { continue };
        let prims = gm["primitives"].as_array().cloned().unwrap_or_default();
        let skin_i = n["skin"].as_u64();
        let target_names: Vec<String> = gm["extras"]["targetNames"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
        let weights: Vec<f32> = n["weights"].as_array().or(gm["weights"].as_array()).map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default();
        for (pi, prim) in prims.iter().enumerate() {
            let mode = prim["mode"].as_u64().unwrap_or(4);
            if !matches!(mode, 4..=6) {
                rep.notes.push(format!("{}: points/lines primitive skipped", gm["name"].as_str().unwrap_or("mesh")));
                continue;
            }
            let pa = prim["attributes"]["POSITION"].as_u64().ok_or("primitive without POSITION")? as usize;
            let (pos, _) = doc.floats(pa)?;
            let nv = pos.len() / 3;
            let raw_idx: Vec<u32> = match prim["indices"].as_u64() {
                Some(a) => doc.indices(a as usize)?,
                None => (0..nv as u32).collect(),
            };
            let tris: Vec<[u32; 3]> = match mode {
                5 => (0..raw_idx.len().saturating_sub(2)).map(|k| if k % 2 == 0 { [raw_idx[k], raw_idx[k + 1], raw_idx[k + 2]] } else { [raw_idx[k + 1], raw_idx[k], raw_idx[k + 2]] }).collect(),
                6 => (1..raw_idx.len().saturating_sub(1)).map(|k| [raw_idx[0], raw_idx[k], raw_idx[k + 1]]).collect(),
                _ => raw_idx.chunks(3).filter(|c| c.len() == 3).map(|c| [c[0], c[1], c[2]]).collect(),
            };
            let uv: Option<Vec<f32>> = prim["attributes"]["TEXCOORD_0"].as_u64().map(|a| doc.floats(a as usize)).transpose()?.map(|x| x.0);
            let joints: Option<Vec<f32>> = prim["attributes"]["JOINTS_0"].as_u64().map(|a| doc.floats(a as usize)).transpose()?.map(|x| x.0);
            let wts: Option<Vec<f32>> = prim["attributes"]["WEIGHTS_0"].as_u64().map(|a| doc.floats(a as usize)).transpose()?.map(|x| x.0);
            // Weld vertices with identical position (and skin data) so the mesh is connected.
            let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
            let mut map = vec![0u32; nv];
            let mut mesh = Mesh::default();
            let mut first_of: Vec<usize> = Vec::new();
            for v in 0..nv {
                let p = Vec3::new(pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]);
                let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
                map[v] = *weld.entry(key).or_insert_with(|| {
                    mesh.positions.push(p);
                    first_of.push(v);
                    mesh.positions.len() as u32 - 1
                });
            }
            let mut corners: Vec<Vec<Vec2>> = Vec::new();
            for t in &tris {
                if t.iter().any(|x| *x as usize >= nv) {
                    continue;
                }
                let f: Vec<u32> = t.iter().map(|x| map[*x as usize]).collect();
                if f[0] == f[1] || f[1] == f[2] || f[0] == f[2] {
                    continue;
                }
                mesh.faces.push(f);
                corners.push(t.iter().map(|x| uv.as_ref().map(|u| Vec2::new(u[*x as usize * 2], 1.0 - u[*x as usize * 2 + 1])).unwrap_or(Vec2::ZERO)).collect());
            }
            if uv.is_some() {
                mesh.set_corner_uvs(&corners);
            }
            // Node: the glTF node itself for a single primitive, else a child per primitive.
            let target = if prims.len() == 1 && !is_joint[i] {
                ids[i]
            } else {
                let nm = format!("{}_prim{pi}", app.scene.node(ids[i]).map(|n| n.name.clone()).unwrap_or_default());
                let c = app.scene.add_node(&nm, Shape::Group, Some(ids[i]));
                app.apply_name(c, Some(&nm));
                c
            };
            let nverts = mesh.positions.len();
            set_shape(app, target, Shape::Poly { history: None, mesh, rev: crate::scene::next_rev() });
            if let Some(mat) = prim["material"].as_u64().and_then(|m| mats.get(m as usize)) {
                app.scene.node_mut(target).unwrap().material = Some(*mat);
            }
            // Morph targets.
            if let Some(tg) = prim["targets"].as_array().filter(|t| !t.is_empty()) {
                let mut bs = BlendShape::default();
                for (ti, t) in tg.iter().enumerate() {
                    let Some(a) = t["POSITION"].as_u64() else { continue };
                    let (d, _) = doc.floats(a as usize)?;
                    let deltas: Vec<Vec3> = first_of.iter().map(|&v| Vec3::new(d[v * 3], d[v * 3 + 1], d[v * 3 + 2])).collect();
                    let name = target_names.get(ti).filter(|s| !s.is_empty()).cloned().unwrap_or(format!("target{ti}"));
                    bs.add_target(name, deltas);
                }
                for (k, w) in weights.iter().enumerate() {
                    if let Some(x) = bs.weights.get_mut(k) {
                        *x = *w;
                    }
                }
                rep.morphs += bs.targets.len();
                app.scene.node_mut(target).unwrap().blend = Some(bs);
            }
            // Skin.
            if let (Some(si), Some(jd), Some(wd)) = (skin_i, &joints, &wts) {
                let sk = &j["skins"][si as usize];
                let jl: Vec<NodeId> = sk["joints"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_u64()).map(|x| ids[x as usize]).collect();
                let ibm: Vec<Mat4> = match sk["inverseBindMatrices"].as_u64() {
                    Some(a) => doc.floats(a as usize)?.0.chunks(16).map(Mat4::from_cols_slice).collect(),
                    None => vec![Mat4::IDENTITY; jl.len()],
                };
                // Our skin: world = joint_world · inv_bind · bind_shape · p.  glTF: world = joint_world · IBM · p.
                let bind_shape = app.scene.world_matrix(target);
                let inv_bind: Vec<Mat4> = ibm.iter().map(|m| *m * bind_shape.inverse()).collect();
                let weights: Vec<Vec<(u32, f32)>> = first_of
                    .iter()
                    .map(|&v| {
                        let mut w: Vec<(u32, f32)> = (0..4).map(|k| (jd[v * 4 + k] as u32, wd[v * 4 + k])).filter(|x| x.1 > 1e-5 && (x.0 as usize) < jl.len()).collect();
                        // Merge duplicate influences, normalise.
                        w.sort_by_key(|x| x.0);
                        w.dedup_by(|a, b| {
                            if a.0 == b.0 {
                                b.1 += a.1;
                                true
                            } else {
                                false
                            }
                        });
                        let s: f32 = w.iter().map(|x| x.1).sum::<f32>().max(1e-6);
                        w.iter_mut().for_each(|x| x.1 /= s);
                        w.sort_by(|a, b| b.1.total_cmp(&a.1));
                        w
                    })
                    .collect();
                let bind_pose = jl.iter().filter_map(|j| app.scene.node(*j).map(|n| (*j, n.translate, n.rotate, n.joint_orient))).collect();
                if weights.len() == nverts {
                    app.scene.node_mut(target).unwrap().skin = Some(Skin { joints: jl, inv_bind, bind_shape, weights, max_influences: 4, bind_pose });
                    rep.skins += 1;
                }
            }
            rep.meshes += 1;
        }
    }

    // ---- animation (first animation becomes keyframes; others are listed)
    let anims = j["animations"].as_array().cloned().unwrap_or_default();
    let names: Vec<String> = anims.iter().enumerate().map(|(k, a)| a["name"].as_str().map(String::from).unwrap_or(format!("anim{k}"))).collect();
    let pick = match take {
        Some(t) => names.iter().position(|n| n.eq_ignore_ascii_case(t)).ok_or(format!("no animation named '{t}' (file has: {})", names.join(", ")))?,
        None => 0,
    };
    if anims.len() > 1 {
        let others: Vec<&str> = names.iter().enumerate().filter(|(k, _)| *k != pick).map(|(_, n)| n.as_str()).collect();
        rep.notes.push(format!("{} animations in file, imported '{}' (others: {}; choose with file -import path -take name)", anims.len(), names[pick], others.join(", ")));
    }
    let fps = app.scene.fps.max(1.0);
    let start = app.scene.play_start;
    let mut last_frame = start;
    if let Some(an) = anims.get(pick) {
        let samplers = an["samplers"].as_array().cloned().unwrap_or_default();
        for ch in an["channels"].as_array().cloned().unwrap_or_default() {
            let Some(ni) = ch["target"]["node"].as_u64().map(|x| x as usize).filter(|x| *x < ids.len()) else { continue };
            let Some(s) = samplers.get(ch["sampler"].as_u64().unwrap_or(0) as usize) else { continue };
            let (times, _) = doc.floats(s["input"].as_u64().ok_or("sampler input")? as usize)?;
            let (vals, comps) = doc.floats(s["output"].as_u64().ok_or("sampler output")? as usize)?;
            let interp = s["interpolation"].as_str().unwrap_or("LINEAR");
            let tangent = if interp == "STEP" { Tangent::Stepped } else { Tangent::Linear };
            let cubic = interp == "CUBICSPLINE";
            let path = ch["target"]["path"].as_str().unwrap_or("");
            let frames: Vec<f32> = times.iter().map(|t| start + t * fps).collect();
            if let Some(f) = frames.last() {
                last_frame = last_frame.max(*f);
            }
            let n_el = if path == "weights" { vals.len() / comps / times.len().max(1) / if cubic { 3 } else { 1 } } else { 1 };
            let value = |k: usize, c: usize, width: usize| -> f32 {
                // CUBICSPLINE stores (in-tangent, value, out-tangent) per key.
                if cubic { vals[(k * 3 + 1) * width + c] } else { vals[k * width + c] }
            };
            let id = ids[ni];
            match path {
                "translation" | "scale" => {
                    let chans = if path == "translation" { Channel::translate() } else { Channel::scale() };
                    for (c, chn) in chans.iter().enumerate() {
                        let mut curve = Curve { tangent, ..Default::default() };
                        for (k, f) in frames.iter().enumerate() {
                            curve.set_key(*f, value(k, c, 3));
                        }
                        app.scene.node_mut(id).unwrap().anim.insert(*chn, curve);
                        rep.anim_curves += 1;
                    }
                }
                "rotation" => {
                    let mut curves = [Curve { tangent, ..Default::default() }, Curve { tangent, ..Default::default() }, Curve { tangent, ..Default::default() }];
                    let mut prev = app.scene.node(id).unwrap().rotate;
                    for (k, f) in frames.iter().enumerate() {
                        let q = Quat::from_xyzw(value(k, 0, 4), value(k, 1, 4), value(k, 2, 4), value(k, 3, 4)).normalize();
                        let e = unwrap(prev, crate::rig::quat_euler(q));
                        prev = e;
                        for c in 0..3 {
                            curves[c].set_key(*f, e[c]);
                        }
                    }
                    let n = app.scene.node_mut(id).unwrap();
                    for (c, curve) in curves.into_iter().enumerate() {
                        n.anim.insert(Channel::rotate()[c], curve);
                        rep.anim_curves += 1;
                    }
                }
                "weights" => {
                    // Morph weights drive the blend shapes of the node (or its primitive children).
                    let mut targets = vec![id];
                    targets.extend(app.scene.children(Some(id)));
                    for t in targets {
                        if let Some(bs) = app.scene.node_mut(t).and_then(|n| n.blend.as_mut()) {
                            for w in 0..n_el.min(bs.targets.len()) {
                                let mut curve = Curve { tangent, ..Default::default() };
                                for (k, f) in frames.iter().enumerate() {
                                    curve.set_key(*f, value(k, w, n_el));
                                }
                                bs.curves[w] = Some(curve);
                                rep.anim_curves += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if last_frame > start {
            app.scene.play_end = app.scene.play_end.max(last_frame.ceil());
            app.scene.anim_end = app.scene.anim_end.max(app.scene.play_end);
        }
        // Rest pose for the bind pose stays as authored; show frame 1 of the animation.
        let t = app.scene.time;
        app.set_time(t);
    }

    // Scene roots.
    let scene_i = j["scene"].as_u64().unwrap_or(0) as usize;
    let roots: Vec<usize> = j["scenes"][scene_i]["nodes"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect()).unwrap_or_else(|| (0..ids.len()).filter(|i| !gnodes.iter().any(|n| n["children"].as_array().is_some_and(|c| c.iter().any(|x| x.as_u64() == Some(*i as u64))))).collect());
    rep.roots = roots.iter().filter_map(|r| ids.get(*r).copied()).collect();
    app.sel = rep.roots.clone();
    app.end_batch();
    Ok(rep)
}

fn set_shape(app: &mut App, id: NodeId, shape: Shape) {
    if let Some(n) = app.scene.node_mut(id) {
        n.shape = shape;
    }
}
