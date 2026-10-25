//! Scene importers.

pub mod gltf;
pub mod usd;

use crate::app::App;
use std::path::Path;

/// Import a file by extension (.glb / .gltf / .obj). Returns a summary.
pub fn import_path(app: &mut App, path: &Path) -> Result<String, String> {
    import_take(app, path, None)
}

/// Import, choosing a named glTF animation ("take").
pub fn import_take(app: &mut App, path: &Path, take: Option<&str>) -> Result<String, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "glb" | "gltf" => gltf::import(app, path, take).map(|r| r.summary()),
        "usda" | "usd" | "usdc" | "usdz" => usd::import(app, path).map(|r| r.summary()),
        "obj" => {
            app.import_obj(path.to_path_buf());
            Ok(String::new())
        }
        _ => Err(format!("Unknown import format '.{ext}' (use .glb, .gltf, .usd/.usda/.usdc/.usdz or .obj)")),
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;
    use crate::scene::Shape;

    /// Export a rigged, animated, textured scene to glTF and import it back.
    #[test]
    fn gltf_round_trip() {
        let mut src = App::new();
        src.scene.nodes.clear();
        let dir = std::env::temp_dir().join("rm_import_rt");
        let _ = std::fs::create_dir_all(&dir);
        let png = dir.join("art.png");
        std::fs::write(&png, crate::texture::TexImage::checker().to_png()).unwrap();
        for m in [
            "select -cl".to_string(),
            "joint -p 0 0 0 -n root".into(),
            "joint -p 0 2 0 -n mid".into(),
            "joint -p 0 4 0 -n tip".into(),
            "polyCylinder -r 0.3 -h 4 -sh 6 -n tube".into(),
            "move 0 2 0".into(),
            "skinCluster root tube".into(),
            "setKeyframe -t 1 -at rz -v 0 mid".into(),
            "setKeyframe -t 10 -at rz -v 60 mid".into(),
            "polyCube -n box".into(),
            "duplicate -n box_tall".into(),
            "scale 1 2 1 box_tall".into(),
            "makeIdentity box_tall".into(),
            "hide box_tall".into(),
            "blendShape box_tall box".into(),
            "setKeyframe -t 1 -at box_tall -v 0 box".into(),
            "setKeyframe -t 10 -at box_tall -v 1 box".into(),
            "shadingNode -asShader standardSurface -n art -rgb 0.9 0.2 0.2".into(),
            format!("setAttr art.colorMap \"{}\"", png.display()),
            "hyperShade -assign art box".into(),
            "spotLight -n key -ca 40 -p 0 5 5 -aim 0 0 0".into(),
            "camera -n shot -fl 50 -p 0 2 10 -aim 0 2 0".into(),
            "playbackOptions -min 1 -max 10".into(),
        ] {
            execute(&mut src, &m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let glb = dir.join("rt.glb");
        execute(&mut src, &format!("file -export {}", glb.display())).unwrap();

        let mut app = App::new();
        app.scene.nodes.clear();
        let msg = execute(&mut app, &format!("file -import {}", glb.display())).unwrap();
        eprintln!("{msg}");
        let s = &app.scene;
        let find = |n: &str| s.find_by_name(n).unwrap_or_else(|| panic!("missing {n}"));
        // Hierarchy + joints.
        assert!(matches!(s.node(find("mid")).unwrap().shape, Shape::Joint { .. }));
        assert_eq!(s.node(find("mid")).unwrap().parent, Some(find("root")));
        // Skin and deformation match the source at the bind pose.
        let tube = find("tube");
        assert!(s.node(tube).unwrap().skin.is_some());
        let a = crate::rig::deformed_positions(&src.scene, src.scene.find_by_name("tube").unwrap()).unwrap();
        let b = crate::rig::deformed_positions(s, tube).unwrap();
        let (sa, sb) = (a.iter().sum::<glam::Vec3>() / a.len() as f32, b.iter().sum::<glam::Vec3>() / b.len() as f32);
        assert!((sa - sb).length() < 1e-3, "skinned centroid {sa} vs {sb}");
        // Animation: mid rotates 60° at frame 10.
        let mut app2 = app;
        app2.set_time(10.0);
        crate::rig::evaluate(&mut app2.scene);
        let tip = crate::rig::world_pos(&app2.scene, app2.scene.find_by_name("tip").unwrap());
        let mut src2 = src;
        src2.set_time(10.0);
        let tip_src = crate::rig::world_pos(&src2.scene, src2.scene.find_by_name("tip").unwrap());
        assert!((tip - tip_src).length() < 1e-3, "animated tip {tip} vs {tip_src}");
        // Blend shape with its target name, animated weight.
        let bx = app2.scene.find_by_name("box").unwrap();
        let bs = app2.scene.node(bx).unwrap().blend.clone().unwrap();
        assert_eq!(bs.targets[0].name, "box_tall");
        assert!((bs.weights[0] - 1.0).abs() < 1e-4);
        // Material + texture + UVs.
        let m = app2.scene.node(bx).unwrap().material.and_then(|m| app2.material(m)).unwrap();
        assert_eq!(m.name, "art");
        assert!(m.color_map.as_ref().is_some_and(|p| std::path::Path::new(p).exists()));
        assert!(app2.scene.node(bx).unwrap().mesh().unwrap().has_uvs());
        // Light + camera.
        assert!(matches!(&app2.scene.node(app2.scene.find_by_name("key").unwrap()).unwrap().shape, Shape::Light(l) if (l.cone - 40.0).abs() < 0.5));
        assert!(matches!(&app2.scene.node(app2.scene.find_by_name("shot").unwrap()).unwrap().shape, Shape::Camera(c) if (c.focal - 50.0).abs() < 0.1));
        // One undo step removes the import.
        execute(&mut app2, "undo").unwrap();
        assert!(app2.scene.find_by_name("tube").is_none());
    }
}

/// SAMPLES=/path/to/dir cargo test samples -- --ignored --nocapture
#[cfg(test)]
#[test]
#[ignore]
fn import_samples() {
    let Ok(dir) = std::env::var("SAMPLES") else { return };
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "glb" || e == "gltf")).collect();
    files.sort();
    for f in files {
        let mut app = App::new();
        app.scene.nodes.clear();
        match import_path(&mut app, &f) {
            Ok(s) => {
                let mid = (app.scene.play_start + app.scene.play_end) * 0.5;
                app.set_time(mid);
                crate::rig::evaluate(&mut app.scene);
                let g = app.build_geometry();
                let bad = g.tris.iter().flat_map(|c| c.data.iter()).any(|v| v.pos.iter().any(|x| !x.is_finite()));
                let tris: usize = g.tris.iter().map(|c| c.data.len() / 3).sum();
                println!("{}: {s} | {tris} tris, finite={}, range {}-{}", f.file_name().unwrap().to_string_lossy(), !bad, app.scene.play_start, app.scene.play_end);
                assert!(!bad);
            }
            Err(e) => println!("{}: ERROR {e}", f.display()),
        }
    }
}
