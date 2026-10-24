//! Scene exporters: glTF 2.0 binary (.glb) and USD ASCII (.usda).

pub mod gltf;
pub mod usd;

use crate::app::App;
use crate::scene::{NodeId, Scene};
use glam::{Mat4, Quat, Vec3};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub selected_only: bool,
    pub animation: bool,
    /// Frame range for baked animation (inclusive).
    pub start: f32,
    pub end: f32,
    /// USD: bake deformed (skinned / blend-shape / cluster) points per frame.
    pub bake_deformation: bool,
    /// Destination file (USD writes the UV-checker texture next to it when used).
    pub out_path: Option<std::path::PathBuf>,
}

impl ExportOptions {
    pub fn from_app(app: &App, selected_only: bool) -> Self {
        ExportOptions { selected_only, animation: true, start: app.scene.play_start, end: app.scene.play_end, bake_deformation: true, out_path: None }
    }
}

/// Nodes to export, parents before children, plus each node's export parent.
pub struct ExportSet {
    pub order: Vec<NodeId>,
    pub parent: HashMap<NodeId, Option<NodeId>>,
}

impl ExportSet {
    pub fn new(scene: &Scene, sel: &[NodeId], selected_only: bool) -> Self {
        let mut include: HashSet<NodeId> = HashSet::new();
        if selected_only {
            for &s in sel {
                include.extend(scene.subtree(s));
            }
        } else {
            include.extend(scene.nodes.iter().map(|n| n.id));
        }
        let mut order = Vec::new();
        let mut parent = HashMap::new();
        // DFS from scene roots so parents precede children.
        let mut stack: Vec<(NodeId, Option<NodeId>)> = scene.children(None).into_iter().rev().map(|r| (r, None)).collect();
        while let Some((id, exp_parent)) = stack.pop() {
            let inc = include.contains(&id);
            if inc {
                order.push(id);
                parent.insert(id, exp_parent);
            }
            let next_parent = if inc { Some(id) } else { exp_parent };
            for c in scene.children(Some(id)).into_iter().rev() {
                stack.push((c, next_parent));
            }
        }
        ExportSet { order, parent }
    }

    /// Local transform relative to the export parent.
    pub fn local(&self, scene: &Scene, id: NodeId) -> (Vec3, Quat, Vec3) {
        let n = scene.node(id).unwrap();
        let ep = self.parent.get(&id).copied().flatten();
        if ep == n.parent && (ep.is_some() || n.parent.is_none()) {
            let q = if n.joint_orient == Vec3::ZERO { n.rotation_quat() } else { crate::rig::euler_quat(n.joint_orient) * n.rotation_quat() };
            return (n.translate, q.normalize(), n.scale);
        }
        let pw = ep.map(|p| scene.world_matrix(p)).unwrap_or(Mat4::IDENTITY);
        let m = pw.inverse() * scene.world_matrix(id);
        let (s, r, t) = m.to_scale_rotation_translation();
        (t, r.normalize(), s)
    }
}

/// Per-frame samples of local transforms, blend weights and (optionally) deformed points.
pub struct Baked {
    pub frames: Vec<f32>,
    pub trs: HashMap<NodeId, Vec<(Vec3, Quat, Vec3)>>,
    pub weights: HashMap<NodeId, Vec<Vec<f32>>>,
    /// Object-space deformed points per frame (meshes with deformers only).
    pub points: HashMap<NodeId, Vec<Vec<Vec3>>>,
}

impl Baked {
    pub fn sample(app: &App, set: &ExportSet, opts: &ExportOptions, with_points: bool) -> Baked {
        let mut frames = Vec::new();
        if opts.animation && opts.end >= opts.start {
            let mut f = opts.start;
            while f <= opts.end + 1e-4 {
                frames.push(f);
                f += 1.0;
            }
        }
        let mut b = Baked { frames, trs: HashMap::new(), weights: HashMap::new(), points: HashMap::new() };
        if b.frames.len() < 2 {
            b.frames.clear();
            return b;
        }
        let mut sc = app.scene.clone();
        for &f in &b.frames.clone() {
            sc.time = f;
            sc.evaluate();
            crate::rig::evaluate(&mut sc);
            for &id in &set.order {
                b.trs.entry(id).or_default().push(set.local(&sc, id));
                let n = sc.node(id).unwrap();
                if let Some(bs) = &n.blend {
                    b.weights.entry(id).or_default().push(bs.weights.clone());
                }
                if with_points && crate::rig::has_deformers(&sc, id) {
                    let world = crate::rig::deformed_positions(&sc, id).unwrap_or_default();
                    let inv = sc.world_matrix(id).inverse();
                    b.points.entry(id).or_default().push(world.iter().map(|p| inv.transform_point3(*p)).collect());
                }
            }
        }
        // Keep only what actually changes.
        b.trs.retain(|_, v| v.windows(2).any(|w| w[0].0.distance(w[1].0) > 1e-5 || w[0].1.angle_between(w[1].1) > 1e-5 || w[0].2.distance(w[1].2) > 1e-5));
        b.weights.retain(|_, v| v.windows(2).any(|w| w[0].iter().zip(&w[1]).any(|(a, c)| (a - c).abs() > 1e-6)));
        b.points.retain(|_, v| v.windows(2).any(|w| w[0].iter().zip(&w[1]).any(|(a, c)| a.distance(*c) > 1e-6)));
        b
    }
}

/// sRGB → linear (exact curve).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Export the scene by file extension (.glb / .gltf → glTF binary, .usda / .usd → USD ASCII).
pub fn export_path(app: &App, path: &std::path::Path, opts: &ExportOptions) -> Result<String, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let opts = &ExportOptions { out_path: Some(path.to_path_buf()), ..opts.clone() };
    let (bytes, summary) = match ext.as_str() {
        "glb" => gltf::export(app, opts)?,
        "gltf" => gltf::export_embedded(app, opts)?,
        "usda" | "usd" => usd::export(app, opts)?,
        _ => return Err(format!("Unknown export format '.{ext}' (use .glb, .gltf or .usda)")),
    };
    std::fs::write(path, &bytes).map_err(|e| e.to_string())?;
    Ok(format!("{summary} → {} ({:.1} KB)", path.display(), bytes.len() as f64 / 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn export_quick_rigged_character() {
        let mut app = App::new();
        app.scene.nodes.clear();
        for m in [
            "polyCube -w 0.5 -h 0.8 -d 0.3 -sy 4 -n torso; move 0 1.2 0; polyCube -w 1.6 -h 0.12 -d 0.12 -sx 12 -n arms; move 0 1.47 0",
            "polyCube -w 0.4 -h 0.9 -d 0.15 -sy 6 -sx 2 -n legs; move 0 0.45 0; polyUnite torso arms legs; rename polySurface1 body",
            "quickRig body",
            "setKeyframe -t 1 -at ty -v 0 L_hand_ctrl; setKeyframe -t 12 -at ty -v 0.3 L_hand_ctrl; playbackOptions -min 1 -max 12",
            "shadingNode -asShader standardSurface -n skin_mat -rgb 0.9 0.7 0.6; hyperShade -assign skin_mat body",
            "directionalLight -n sun -sh on; camera -n cam -p 0 1.5 6 -aim 0 1 0",
        ] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let dir = std::env::temp_dir();
        let g = execute(&mut app, &format!("file -export {}", dir.join("rm_char.glb").display())).unwrap();
        let u = execute(&mut app, &format!("file -export {}", dir.join("rm_char.usda").display())).unwrap();
        eprintln!("{g}\n{u}");
        assert!(g.contains("1 skins") && g.contains("animation channels"));
        assert!(u.contains("baked as point samples"));
        // The wrist moves in the baked animation (driven through IK), so it gets a rotation channel chain.
        let (doc, _) = gltf::parse_glb(&std::fs::read(dir.join("rm_char.glb")).unwrap());
        let n_ch = doc["animations"][0]["channels"].as_array().unwrap().len();
        assert!(n_ch >= 3, "{n_ch}");
    }

    #[test]
    fn export_textured() {
        let mut app = App::new();
        app.scene.nodes.clear();
        let dir = std::env::temp_dir();
        let png = dir.join("rm_tex_export.png");
        std::fs::write(&png, crate::texture::TexImage::checker().to_png()).unwrap();
        for m in [
            "polyCube -n crate".to_string(),
            "polySphere -n ball".to_string(),
            "shadingNode -asShader blinn -n art".to_string(),
            format!("setAttr art.colorMap \"{}\"", png.display()),
            "setAttr art.repeatUV 2 2".to_string(),
            "hyperShade -assign art crate".to_string(),
            "shadingNode -asShader lambert -n chk; setAttr chk.colorMap checker; hyperShade -assign chk ball".to_string(),
        ] {
            execute(&mut app, &m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let g = execute(&mut app, &format!("file -export {}", dir.join("rm_tex.glb").display())).unwrap();
        let u = execute(&mut app, &format!("file -export {}", dir.join("rm_tex.usda").display())).unwrap();
        eprintln!("{g}\n{u}");
        let (doc, _) = gltf::parse_glb(&std::fs::read(dir.join("rm_tex.glb")).unwrap());
        assert_eq!(doc["images"].as_array().unwrap().len(), 2);
        assert!(doc["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"].is_number());
        assert!(doc["materials"].as_array().unwrap().iter().any(|m| m["pbrMetallicRoughness"]["baseColorTexture"]["index"].is_number()));
        let usda = std::fs::read_to_string(dir.join("rm_tex.usda")).unwrap();
        assert!(usda.contains("texCoord2f[] primvars:st"));
        assert!(usda.contains("UsdUVTexture"));
        assert!(dir.join("rm_tex_uvchecker.png").exists());
        // OBJ keeps UVs.
        execute(&mut app, &format!("file -export {}", dir.join("rm_tex.obj").display())).unwrap();
        let obj = std::fs::read_to_string(dir.join("rm_tex.obj")).unwrap();
        assert!(obj.contains("\nvt "));
        // ...and imports back with its material and texture.
        let mut back = App::new();
        back.scene.nodes.clear();
        back.import_obj(dir.join("rm_tex.obj"));
        let crate_id = back.scene.find_by_name("crate").unwrap();
        let m = back.scene.node(crate_id).unwrap().material.and_then(|m| back.material(m)).unwrap();
        assert_eq!(m.name, "art");
        assert!(m.color_map.is_some());
        assert!(back.scene.node(crate_id).unwrap().mesh().unwrap().has_uvs());
    }
}
