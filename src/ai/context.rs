//! Scene context for prompts and the automatic scene health report.

use crate::app::{App, CompMode};
use crate::scene::Shape;
use glam::Vec3;

fn v3(v: Vec3) -> String {
    format!("[{:.2}, {:.2}, {:.2}]", v.x, v.y, v.z)
}

/// Compact, model-friendly description of the scene (capped for small context windows).
pub fn scene_summary(app: &App) -> String {
    let sc = &app.scene;
    let mut s = String::new();
    let sel: Vec<String> = app.sel.iter().filter_map(|id| sc.node(*id)).map(|n| n.name.clone()).collect();
    s.push_str(&format!(
        "time {} (playback {}-{} @ {} fps); mode {}; tool {:?}; selected: [{}]\n",
        sc.time,
        sc.play_start,
        sc.play_end,
        sc.fps,
        match app.comp_mode {
            CompMode::Object => "object",
            CompMode::Vertex => "vertex",
            CompMode::Face => "face",
            CompMode::Edge => "edge",
        },
        app.tool,
        sel.join(", ")
    ));
    if !sc.materials.is_empty() {
        let m: Vec<String> = sc.materials.iter().map(|m| format!("{} ({}, color [{:.2}, {:.2}, {:.2}])", m.name, m.kind.name(), m.color[0], m.color[1], m.color[2])).collect();
        s.push_str(&format!("Materials: {}\n", m.join("; ")));
    }
    if sc.nodes.is_empty() {
        s.push_str("(empty scene)\n");
    }
    for (i, n) in sc.nodes.iter().enumerate() {
        if i >= 60 {
            s.push_str(&format!("... and {} more objects\n", sc.nodes.len() - 60));
            break;
        }
        let kind = match &n.shape {
            Shape::Particles(ps) => {
                let st = &ps.settings;
                let src = st.source.and_then(|i| sc.node(i)).map(|x| format!(" from {}", x.name)).unwrap_or_default();
                format!("particle system ({} emitter{src}, rate {:.0}/s, speed {:.2}, lifespan {:.1}s, {} live particles)", st.kind.name(), st.rate, st.speed, st.lifespan, ps.current().len())
            }
            Shape::Field(f) => format!(
                "{} field (magnitude {:.2}, attenuation {:.1}{}, acts on {})",
                f.kind.name(),
                f.magnitude,
                f.attenuation,
                if f.max_distance > 0.0 { format!(", max distance {:.2}", f.max_distance) } else { String::new() },
                if f.targets.is_empty() { "all simulations".to_string() } else { f.targets.iter().filter_map(|i| sc.node(*i)).map(|n| n.name.clone()).collect::<Vec<_>>().join(", ") }
            ),
            Shape::Hair(h) => {
                let st = &h.settings;
                let src = st.source.and_then(|i| sc.node(i)).map(|x| x.name.clone()).unwrap_or_else(|| "?".into());
                if st.output.is_some() {
                    format!("dynamic curve (from {src}{}, stiffness {:.2})", if st.pin_tip { ", both ends pinned" } else { "" }, st.stiffness)
                } else {
                    format!("hair system ({} strands on {src}, length {:.2}, {} segments, stiffness {:.2})", st.count, st.length, st.segments, st.stiffness)
                }
            }
            Shape::Group => "group".to_string(),
            Shape::Locator => "locator".to_string(),
            Shape::Joint { .. } => format!("joint (orient {})", v3(n.joint_orient)),
            Shape::IkHandle { start, end, pole, stretch, .. } => format!(
                "ikHandle ({} -> {}{}{})",
                sc.node(*start).map(|x| x.name.as_str()).unwrap_or("?"),
                sc.node(*end).map(|x| x.name.as_str()).unwrap_or("?"),
                pole.and_then(|p| sc.node(p)).map(|p| format!(", pole {}", p.name)).unwrap_or_default(),
                if *stretch { ", stretchy" } else { "" }
            ),
            Shape::ClusterHandle => "cluster handle".to_string(),
            Shape::Light(l) => format!("{} (intensity {:.2}, color [{:.2}, {:.2}, {:.2}]{}{}, aims {})", l.kind.name(), l.intensity, l.color[0], l.color[1], l.color[2],
                if l.kind == crate::look::LightKind::Spot { format!(", cone {:.0}", l.cone) } else { String::new() },
                if l.shadows { ", shadows" } else { "" }, v3(crate::rig::world_rotation(sc, n.id) * Vec3::NEG_Z)),
            Shape::Lattice { div, .. } => format!("lattice ({}x{}x{} points)", div[0], div[1], div[2]),
            Shape::Camera(c) => format!("camera (focal {:.0} mm, looks {})", c.focal, v3(crate::rig::world_rotation(sc, n.id) * Vec3::NEG_Z)),
            Shape::Curve { closed, degree, points, .. } => {
                let len = crate::nurbs::world_curve(sc, n.id).map(|c| c.length()).unwrap_or(0.0);
                format!("nurbsCurve ({}, degree {degree}, {} CVs, length {len:.2})", if *closed { "closed" } else { "open" }, points.len())
            }
            Shape::Poly { history, mesh, .. } => match (history, &n.surf) {
                (Some(p), _) => format!("poly{} ({} faces)", p.base_name(), mesh.face_count()),
                (None, Some(h)) => {
                    let ins: Vec<String> = h.inputs.iter().filter_map(|i| sc.node(*i)).map(|x| x.name.clone()).collect();
                    format!("mesh from {} of {} ({} faces, live history)", h.op_name(), ins.join(", "), mesh.face_count())
                }
                (None, None) => format!("mesh ({} faces)", mesh.face_count()),
            },
        };
        let world = sc.world_matrix(n.id).transform_point3(Vec3::ZERO);
        let size = app.world_bounds(&[n.id]).map(|(lo, hi)| hi - lo).unwrap_or(Vec3::ZERO);
        s.push_str(&format!("- {} : {kind}", n.name));
        if let Some(p) = n.parent.and_then(|p| sc.node(p)) {
            s.push_str(&format!(", parent {}", p.name));
        }
        s.push_str(&format!(", t {} r {} s {}, world pos {}, size {}", v3(n.translate), v3(n.rotate), v3(n.scale), v3(world), v3(size)));
        if n.mesh().is_some() {
            match n.material.and_then(|m| sc.materials.iter().find(|x| x.id == m)) {
                Some(m) => s.push_str(&format!(", material {} ({})", m.name, m.kind.name())),
                None => s.push_str(&format!(", color [{:.2}, {:.2}, {:.2}]", n.color[0], n.color[1], n.color[2])),
            }
        }
        if !n.visible {
            s.push_str(", hidden");
        }
        if let Some(sk) = &n.skin {
            let joints: Vec<&str> = sk.joints.iter().filter_map(|j| sc.node(*j)).map(|j| j.name.as_str()).collect();
            s.push_str(&format!(", skinned to [{}]", joints.join(", ")));
        }
        if let Some(b) = &n.blend {
            let t: Vec<String> = b.targets.iter().zip(&b.weights).map(|(t, w)| format!("{}={w:.2}", t.name)).collect();
            s.push_str(&format!(", blendShape [{}]", t.join(", ")));
        }
        if !n.clusters.is_empty() {
            let h: Vec<&str> = n.clusters.iter().filter_map(|c| sc.node(c.handle)).map(|x| x.name.as_str()).collect();
            s.push_str(&format!(", clusters [{}]", h.join(", ")));
        }
        if !n.lattices.is_empty() {
            let l: Vec<&str> = n.lattices.iter().filter_map(|b| sc.node(b.lattice)).map(|x| x.name.as_str()).collect();
            s.push_str(&format!(", lattices [{}]", l.join(", ")));
        }
        if let Some(w) = n.wrap.as_ref().and_then(|w| sc.node(w.driver)) {
            s.push_str(&format!(", wrapped to {}", w.name));
        }
        if let Some(c) = &n.cloth {
            let col: Vec<&str> = c.settings.colliders.iter().filter_map(|i| sc.node(*i)).map(|x| x.name.as_str()).collect();
            s.push_str(&format!(", nCloth (stretch {:.2}, bend {:.2}, {} pinned vertices, colliders [{}])", c.settings.stretch, c.settings.bend, c.settings.pins.len(), col.join(", ")));
        }
        if !n.wires.is_empty() {
            let w: Vec<String> = n.wires.iter().filter_map(|b| sc.node(b.curve).map(|c| format!("{} (dropoff {:.2})", c.name, b.dropoff))).collect();
            s.push_str(&format!(", wires [{}]", w.join(", ")));
        }
        for c in &n.constraints {
            let d: Vec<&str> = c.drivers.iter().filter_map(|j| sc.node(*j)).map(|j| j.name.as_str()).collect();
            s.push_str(&format!(", {} by [{}]", c.kind.name(), d.join(", ")));
        }
        for dk in &n.driven {
            let what = match dk.blend_target {
                Some(t) => n.blend.as_ref().and_then(|b| b.targets.get(t)).map(|x| x.name.clone()).unwrap_or_default(),
                None => dk.channel.long().to_string(),
            };
            s.push_str(&format!(", {what} driven by {}.{}", sc.node(dk.driver).map(|x| x.name.as_str()).unwrap_or("?"), dk.driver_channel.long()));
        }
        if n.is_animated() {
            let keys = sc.key_frames(&[n.id]);
            s.push_str(&format!(", keyed at {:?}", keys.iter().map(|k| *k as i32).collect::<Vec<_>>()));
        }
        s.push('\n');
    }
    s
}

pub struct Issue {
    pub severity: u8,
    pub text: String,
}

/// Rule-based checks; the report is shown as-is and also sent to the helper model.
pub fn health_issues(app: &App) -> Vec<Issue> {
    let sc = &app.scene;
    let mut out = Vec::new();
    let mut push = |severity: u8, text: String| out.push(Issue { severity, text });
    let mut total_faces = 0;
    for n in &sc.nodes {
        if let Shape::Poly { history, mesh, .. } = &n.shape {
            total_faces += mesh.face_count();
            let ngons = mesh.faces.iter().filter(|f| f.len() > 4).count();
            if ngons > 0 && history.is_none() {
                push(2, format!("{}: {ngons} n-gon face(s) (>4 sides) — may deform badly and smooth unpredictably.", n.name));
            }
            if (n.scale - Vec3::ONE).abs().max_element() > 1e-3 || n.rotate.abs().max_element() > 1e-3 {
                push(1, format!("{}: transforms are not frozen (scale {} rotate {}) — freeze before rigging/export.", n.name, v3(n.scale), v3(n.rotate)));
            }
            if n.scale.min_element() < 0.0 {
                push(3, format!("{}: negative scale — normals will be flipped on export.", n.name));
            }
            if history.is_some() {
                push(0, format!("{}: still has construction history ({}).", n.name, history.as_ref().map(|p| p.base_name()).unwrap_or("")));
            }
            let degenerate = (0..mesh.face_count()).filter(|&f| mesh.faces[f].len() >= 3 && mesh.face_normal(f) == Vec3::ZERO).count();
            if degenerate > 0 {
                push(3, format!("{}: {degenerate} zero-area face(s).", n.name));
            }
            if mesh.face_count() == 0 {
                push(2, format!("{}: empty mesh (no faces).", n.name));
            }
        }
        let base = n.name.trim_end_matches(|c: char| c.is_ascii_digit());
        if matches!(base, "pCube" | "pSphere" | "pCylinder" | "pCone" | "pPlane" | "pTorus" | "polySurface" | "group" | "null" | "locator") {
            push(0, format!("{}: default name — use descriptive names for a clean outliner.", n.name));
        }
        if !n.visible {
            push(0, format!("{}: hidden.", n.name));
        }
    }
    for l in &sc.layers {
        if !sc.nodes.iter().any(|n| n.layer == Some(l.id)) {
            push(0, format!("Display layer {} is empty.", l.name));
        }
    }
    if total_faces > 200_000 {
        push(2, format!("Scene has {total_faces} faces — viewport may be slow."));
    }
    let roots = sc.children(None).len();
    if roots > 15 {
        push(1, format!("{roots} top-level objects — consider grouping."));
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity));
    out
}

pub fn health_report(app: &App) -> String {
    let issues = health_issues(app);
    if issues.is_empty() {
        return "No issues found.".into();
    }
    issues
        .iter()
        .map(|i| format!("[{}] {}", ["info", "low", "medium", "high"][i.severity.min(3) as usize], i.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Description of the selected objects for Smart Rename.
pub fn selection_details(app: &App) -> String {
    app.sel
        .iter()
        .filter_map(|id| app.scene.node(*id))
        .map(|n| {
            let size = app.world_bounds(&[n.id]).map(|(lo, hi)| hi - lo).unwrap_or(Vec3::ZERO);
            let pos = app.scene.world_matrix(n.id).transform_point3(Vec3::ZERO);
            let parent = n.parent.and_then(|p| app.scene.node(p)).map(|p| p.name.clone()).unwrap_or("-".into());
            let kids: Vec<String> = app.scene.children(Some(n.id)).iter().filter_map(|c| app.scene.node(*c)).map(|c| c.name.clone()).collect();
            format!("{}: type {:?}, world pos {}, size {}, color {:?}, parent {parent}, children [{}]", n.name, shape_kind(&n.shape), v3(pos), v3(size), n.color, kids.join(", "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn shape_kind(s: &Shape) -> &'static str {
    match s {
        Shape::Group => "group",
        Shape::Locator => "locator",
        Shape::Joint { .. } => "joint",
        Shape::IkHandle { .. } => "ikHandle",
        Shape::ClusterHandle => "clusterHandle",
        Shape::Light(l) => l.kind.name(),
        Shape::Camera(_) => "camera",
        Shape::Lattice { .. } => "lattice",
        Shape::Particles(_) => "particles",
        Shape::Field(_) => "field",
        Shape::Hair(_) => "hair",
        Shape::Curve { .. } => "curve",
        Shape::Poly { history: Some(p), .. } => p.base_name(),
        Shape::Poly { .. } => "mesh",
    }
}
