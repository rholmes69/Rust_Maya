//! "Explain my rig": a structured description of skeletons, IK, controls, constraints, driven
//! keys, skins, deformers and simulations, plus checks for common rigging problems with
//! suggested fixes. The AI copilot turns the report into a plain-language explanation; the
//! report alone works offline (`rigReport`).

use crate::app::App;
use crate::scene::{NodeId, Shape};
use glam::Vec3;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn tag(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN",
            Severity::Info => "info",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RigReport {
    /// Description lines (what the rig is and how it's driven).
    pub lines: Vec<String>,
    pub issues: Vec<(Severity, String)>,
    /// MEL fixes for the issues that have one.
    pub fixes: Vec<String>,
    pub joints: usize,
    pub skins: usize,
}

impl RigReport {
    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        if self.issues.is_empty() {
            s.push_str("\nNo problems found.");
        } else {
            s.push_str("\nIssues:");
            for (sev, m) in &self.issues {
                s.push_str(&format!("\n  [{}] {m}", sev.tag()));
            }
        }
        s
    }
}

fn name(app: &App, id: NodeId) -> String {
    app.scene.node(id).map(|n| n.name.clone()).unwrap_or_else(|| "(deleted)".into())
}

/// The mirrored name (L_x ↔ R_x, x_L ↔ x_R, left ↔ right), if the name is sided.
pub fn mirror_name(n: &str) -> Option<String> {
    for (a, b) in [("L_", "R_"), ("R_", "L_"), ("l_", "r_"), ("r_", "l_"), ("Left", "Right"), ("Right", "Left"), ("left", "right"), ("right", "left")] {
        if let Some(rest) = n.strip_prefix(a) {
            return Some(format!("{b}{rest}"));
        }
    }
    for (a, b) in [("_L", "_R"), ("_R", "_L"), ("_l", "_r"), ("_r", "_l")] {
        if let Some(rest) = n.strip_suffix(a) {
            return Some(format!("{rest}{b}"));
        }
    }
    None
}

impl App {
    pub fn rig_report(&self) -> RigReport {
        let sc = &self.scene;
        let mut r = RigReport::default();
        let joints: Vec<NodeId> = sc.nodes.iter().filter(|n| matches!(n.shape, Shape::Joint { .. })).map(|n| n.id).collect();
        r.joints = joints.len();
        let jset: HashSet<NodeId> = joints.iter().copied().collect();
        let kids = |id: NodeId| -> Vec<NodeId> { sc.nodes.iter().filter(|n| n.parent == Some(id) && jset.contains(&n.id)).map(|n| n.id).collect() };
        // ---- skeletons
        let roots: Vec<NodeId> = joints.iter().copied().filter(|j| !sc.node(*j).and_then(|n| n.parent).is_some_and(|p| jset.contains(&p))).collect();
        if roots.is_empty() {
            r.lines.push("No skeleton in the scene.".into());
        }
        for root in &roots {
            let mut count = 0;
            let mut leaves = Vec::new();
            let mut stack = vec![(*root, vec![*root])];
            while let Some((j, path)) = stack.pop() {
                count += 1;
                let k = kids(j);
                if k.is_empty() {
                    leaves.push(path);
                } else {
                    for c in k {
                        let mut p = path.clone();
                        p.push(c);
                        stack.push((c, p));
                    }
                }
            }
            r.lines.push(format!("Skeleton '{}': {count} joints, {} chains:", name(self, *root), leaves.len()));
            leaves.sort_by_key(|p| p.last().map(|l| name(self, *l)).unwrap_or_default());
            for p in leaves.iter().take(12) {
                // Skip the shared trunk: show from the branch point.
                let names: Vec<String> = p.iter().map(|j| name(self, *j)).collect();
                let shown = if names.len() > 7 { format!("{} → … → {}", names[..3].join(" → "), names[names.len() - 3..].join(" → ")) } else { names.join(" → ") };
                r.lines.push(format!("  • {shown}"));
            }
            if leaves.len() > 12 {
                r.lines.push(format!("  • … and {} more chains", leaves.len() - 12));
            }
        }
        // Joint checks: orientation along the bone, scale, L/R symmetry.
        let mut unaimed = Vec::new();
        let mut scaled = Vec::new();
        for &j in &joints {
            let n = sc.node(j).unwrap();
            if (n.scale - Vec3::ONE).abs().max_element() > 1e-3 {
                scaled.push(n.name.clone());
            }
            let k = kids(j);
            if k.len() == 1 {
                let t = sc.node(k[0]).unwrap().translate;
                if t.length() > 1e-4 {
                    let d = t.normalize();
                    // Oriented joints point an axis (X by default) down the bone.
                    let best = d.x.abs().max(d.y.abs()).max(d.z.abs());
                    if d.x.abs() < 0.966 && best < 0.966 {
                        unaimed.push(n.name.clone());
                    }
                }
            }
        }
        if !unaimed.is_empty() {
            r.issues.push((Severity::Warning, format!("{} joint(s) aren't oriented along their bone ({}): rotations will twist oddly and mirroring may flip.", unaimed.len(), unaimed.iter().take(6).cloned().collect::<Vec<_>>().join(", "))));
            for rt in &roots {
                r.fixes.push(format!("orientJoint {}", name(self, *rt)));
            }
        }
        if !scaled.is_empty() {
            r.issues.push((Severity::Warning, format!("Scaled joints ({}): scale on joints skews children and exports poorly; scale the rig root or controls instead.", scaled.iter().take(6).cloned().collect::<Vec<_>>().join(", "))));
        }
        let names: HashSet<String> = sc.nodes.iter().map(|n| n.name.clone()).collect();
        let lonely: Vec<String> = sc
            .nodes
            .iter()
            .filter(|n| matches!(n.shape, Shape::Joint { .. } | Shape::Curve { .. }))
            .filter_map(|n| mirror_name(&n.name).filter(|m| !names.contains(m)).map(|_| n.name.clone()))
            .collect();
        if !lonely.is_empty() {
            r.issues.push((Severity::Info, format!("Sided names without a mirror partner: {} (mirror tools pair L_/R_ names).", lonely.iter().take(8).cloned().collect::<Vec<_>>().join(", "))));
        }
        // ---- IK
        let iks: Vec<NodeId> = sc.nodes.iter().filter(|n| matches!(n.shape, Shape::IkHandle { .. })).map(|n| n.id).collect();
        if !iks.is_empty() {
            r.lines.push(format!("IK handles ({}):", iks.len()));
        }
        for ik in &iks {
            let n = sc.node(*ik).unwrap();
            let Shape::IkHandle { start, end, pole, stretch, twist, .. } = &n.shape else { continue };
            // Chain from end up to start.
            let mut chain = vec![*end];
            let mut cur = *end;
            while cur != *start {
                match sc.node(cur).and_then(|x| x.parent) {
                    Some(p) => {
                        chain.push(p);
                        cur = p;
                    }
                    None => break,
                }
            }
            chain.reverse();
            let drivers: Vec<String> = n.constraints.iter().flat_map(|c| c.drivers.iter().map(move |d| format!("{} ({})", name(self, *d), c.kind.name()))).collect();
            r.lines.push(format!(
                "  • {}: {} → {} ({} bones){}{}{}; goal {}",
                n.name,
                name(self, *start),
                name(self, *end),
                chain.len().saturating_sub(1),
                pole.map(|p| format!(", pole {}", name(self, p))).unwrap_or_default(),
                if *stretch { ", stretchy" } else { "" },
                if twist.abs() > 1e-3 { format!(", twist {twist:.0}°") } else { String::new() },
                if drivers.is_empty() { "moved directly".to_string() } else { format!("driven by {}", drivers.join(", ")) }
            ));
            if pole.is_none() && chain.len() >= 3 {
                r.issues.push((Severity::Warning, format!("{} has no pole vector: the {} can flip when the goal crosses the chain's plane.", n.name, name(self, chain[1]))));
                r.fixes.push(format!("// add a locator in front of {} then: poleVectorConstraint <locator> {}", name(self, chain[1]), n.name));
            }
            if chain.len() == 3 && pole.is_none() {
                let p: Vec<Vec3> = chain.iter().map(|j| crate::rig::world_pos(sc, *j)).collect();
                let (a, b) = ((p[0] - p[1]).normalize_or_zero(), (p[2] - p[1]).normalize_or_zero());
                if a.dot(b) < -0.9986 {
                    r.issues.push((Severity::Warning, format!("{}: the chain is perfectly straight (no pre-bend at {}), so the solver can't tell which way to bend.", n.name, name(self, chain[1]))));
                }
            }
        }
        // ---- controls and constraints
        let mut ctrl_lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for n in &sc.nodes {
            for c in &n.constraints {
                for d in &c.drivers {
                    match sc.node(*d) {
                        Some(dn) => ctrl_lines.entry(dn.name.clone()).or_default().push(format!("{} → {}", c.kind.name(), n.name)),
                        None => r.issues.push((Severity::Error, format!("{} on {} has a deleted driver.", c.kind.name(), n.name))),
                    }
                }
                // Keys fighting a constraint.
                let chans: &[crate::scene::Channel] = match c.kind {
                    crate::rig::ConstraintKind::Point => &[crate::scene::Channel::TranslateX, crate::scene::Channel::TranslateY, crate::scene::Channel::TranslateZ],
                    crate::rig::ConstraintKind::Orient | crate::rig::ConstraintKind::Aim => &[crate::scene::Channel::RotateX, crate::scene::Channel::RotateY, crate::scene::Channel::RotateZ],
                    crate::rig::ConstraintKind::Parent => &[crate::scene::Channel::TranslateX, crate::scene::Channel::RotateX],
                    crate::rig::ConstraintKind::Scale => &[crate::scene::Channel::ScaleX],
                };
                if chans.iter().any(|ch| n.anim.contains_key(ch)) {
                    r.issues.push((Severity::Warning, format!("{} is keyed and also constrained ({}): the constraint wins, so those keys do nothing.", n.name, c.kind.name())));
                }
            }
            for dk in &n.driven {
                match sc.node(dk.driver) {
                    Some(d) => ctrl_lines.entry(d.name.clone()).or_default().push(format!("set driven key {}.{:?} → {}", d.name, dk.driver_channel, n.name)),
                    None => r.issues.push((Severity::Error, format!("A driven key on {} has a deleted driver.", n.name))),
                }
            }
        }
        if !ctrl_lines.is_empty() {
            r.lines.push(format!("Controls and drivers ({}):", ctrl_lines.len()));
            for (c, v) in &ctrl_lines {
                let keyed = sc.find_by_name(c).and_then(|i| sc.node(i)).map(|n| n.anim.len()).unwrap_or(0);
                r.lines.push(format!("  • {c}: {}{}", v.join("; "), if keyed > 0 { format!(" [{keyed} animated channels]") } else { String::new() }));
            }
        }
        // ---- skins and deformers
        for n in &sc.nodes {
            let Some(mesh) = n.mesh() else { continue };
            let nv = mesh.positions.len();
            let mut def = Vec::new();
            if let Some(sk) = &n.skin {
                r.skins += 1;
                let unweighted = sk.weights.iter().filter(|w| w.iter().map(|x| x.1).sum::<f32>() < 0.99).count();
                let maxinf = sk.weights.iter().map(|w| w.iter().filter(|x| x.1 > 1e-4).count()).max().unwrap_or(0);
                let over4 = sk.weights.iter().filter(|w| w.iter().filter(|x| x.1 > 1e-4).count() > 4).count();
                let used: HashSet<u32> = sk.weights.iter().flatten().filter(|x| x.1 > 1e-4).map(|x| x.0).collect();
                let unused: Vec<String> = sk.joints.iter().enumerate().filter(|(i, _)| !used.contains(&(*i as u32))).map(|(_, j)| name(self, *j)).collect();
                def.push(format!("skinCluster ({} influences, up to {maxinf} per vertex)", sk.joints.len()));
                if sk.weights.len() != nv {
                    r.issues.push((Severity::Error, format!("{}: the skin has {} weight rows for {nv} vertices (topology changed after binding) — the mesh won't deform.", n.name, sk.weights.len())));
                }
                if unweighted > 0 {
                    r.issues.push((Severity::Error, format!("{}: {unweighted} vertices aren't fully weighted (weights sum below 1): they'll shrink toward the origin when the rig moves.", n.name)));
                    r.fixes.push(format!("skinPercent -normalize {}", n.name));
                }
                if over4 > 0 {
                    r.issues.push((Severity::Info, format!("{}: {over4} vertices have more than 4 influences (game engines and glTF keep 4).", n.name)));
                }
                if !unused.is_empty() {
                    r.issues.push((Severity::Info, format!("{}: {} influence(s) carry no weight ({}).", n.name, unused.len(), unused.iter().take(5).cloned().collect::<Vec<_>>().join(", "))));
                }
                let moved = n.translate.length() > 1e-4 || n.rotate.length() > 1e-3 || (n.scale - Vec3::ONE).abs().max_element() > 1e-4;
                if moved {
                    r.issues.push((Severity::Warning, format!("{}: the skinned mesh's own transform isn't zero — moving the rig AND the mesh double-transforms it. Move the rig controls instead.", n.name)));
                }
            }
            if let Some(b) = &n.blend {
                let keyed = b.curves.iter().filter(|c| c.is_some()).count();
                def.push(format!("blendShape ({} targets{})", b.targets.len(), if keyed > 0 { format!(", {keyed} animated") } else { String::new() }));
            }
            if !n.clusters.is_empty() {
                def.push(format!("{} cluster(s)", n.clusters.len()));
            }
            for l in &n.lattices {
                def.push(format!("lattice {}", name(self, l.lattice)));
            }
            if let Some(w) = &n.wrap {
                def.push(format!("wrap → {}", name(self, w.driver)));
            }
            for w in &n.wires {
                def.push(format!("wire {} (dropoff {:.2})", name(self, w.curve), w.dropoff));
            }
            if let Some(c) = &n.cloth {
                let pin = match c.settings.pin_to {
                    Some(p) if sc.node(p).is_none() => {
                        r.issues.push((Severity::Error, format!("{}: the cloth's pins follow a deleted object.", n.name)));
                        "a deleted object".to_string()
                    }
                    Some(p) => name(self, p),
                    None => "its own transform".into(),
                };
                let cols: Vec<String> = c.settings.colliders.iter().map(|i| name(self, *i)).collect();
                def.push(format!("nCloth ({} pins following {pin}; colliders: {})", c.settings.pins.len(), if cols.is_empty() { "none".into() } else { cols.join(", ") }));
                if c.settings.pins.is_empty() && cols.is_empty() {
                    r.issues.push((Severity::Info, format!("{}: the cloth has no pins and no colliders, so it just falls to the ground.", n.name)));
                }
            }
            if !def.is_empty() {
                r.lines.push(format!("Mesh '{}' ({nv} vertices): {}", n.name, def.join(", ")));
            }
        }
        for n in &sc.nodes {
            if let Shape::Particles(p) = &n.shape {
                r.lines.push(format!("Particles '{}': {} emitter, {:.0}/s, {} colliders", n.name, p.settings.kind.name(), p.settings.rate, p.settings.colliders.len()));
            }
            if let Shape::Hair(h) = &n.shape {
                let src = h.settings.source.and_then(|i| sc.node(i)).map(|x| x.name.clone()).unwrap_or_else(|| "(missing)".into());
                r.lines.push(format!("Hair '{}': {} strands on {src}, {} colliders", n.name, h.settings.count, h.settings.colliders.len()));
                if h.settings.source.and_then(|i| sc.node(i)).is_none() {
                    r.issues.push((Severity::Warning, format!("Hair '{}' has lost its growth mesh / curve.", n.name)));
                }
            }
            if let Shape::Field(f) = &n.shape {
                r.lines.push(format!("Field '{}': {} (magnitude {:.2}), acts on {}", n.name, f.kind.name(), f.magnitude, if f.targets.is_empty() { "all simulations".to_string() } else { format!("{} target(s)", f.targets.len()) }));
            }
        }
        if r.joints > 0 && r.skins == 0 {
            r.issues.push((Severity::Info, "There's a skeleton but no skinned mesh: bind a mesh (Skin › Bind Skin, or rigAnimate / quickRig with the mesh selected).".into()));
        }
        r.issues.sort_by_key(|i| i.0);
        r
    }

    /// Normalise skin weights so every vertex sums to 1 (zero-weight vertices go to their
    /// nearest joint).
    pub fn normalize_skin(&mut self, ids: &[NodeId]) -> Result<usize, String> {
        let targets: Vec<NodeId> = ids.iter().copied().filter(|i| self.scene.node(*i).is_some_and(|n| n.skin.is_some())).collect();
        if targets.is_empty() {
            return Err("skinPercent: select skinned meshes.".into());
        }
        self.checkpoint();
        let mut fixed = 0;
        for id in targets {
            let pts: Vec<Vec3> = {
                let w = self.scene.world_matrix(id);
                self.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
            };
            let jpos: Vec<Vec3> = self.scene.node(id).unwrap().skin.as_ref().unwrap().joints.iter().map(|j| crate::rig::world_pos(&self.scene, *j)).collect();
            let sk = self.scene.node_mut(id).unwrap().skin.as_mut().unwrap();
            for (v, w) in sk.weights.iter_mut().enumerate() {
                let sum: f32 = w.iter().map(|x| x.1).sum();
                if (sum - 1.0).abs() < 1e-4 {
                    continue;
                }
                fixed += 1;
                if sum > 1e-6 {
                    for x in w.iter_mut() {
                        x.1 /= sum;
                    }
                } else if let Some(p) = pts.get(v) {
                    let best = jpos.iter().enumerate().min_by(|a, b| a.1.distance(*p).total_cmp(&b.1.distance(*p))).map(|x| x.0).unwrap_or(0);
                    *w = vec![(best as u32, 1.0)];
                }
            }
        }
        Ok(fixed)
    }
}

pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let r = match cmd.as_str() {
        "rigReport" | "explainRig" => Ok(app.rig_report().text()),
        "skinPercent" => (|| {
            if !rest.iter().any(|t| t == "-normalize" || t == "-nrm") {
                return Err("skinPercent -normalize meshes".to_string());
            }
            let objs: Vec<String> = rest.iter().filter(|t| !t.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            app.normalize_skin(&ids).map(|n| format!("{n} vertices normalised"))
        })(),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;

    #[test]
    fn report_describes_quick_rig_and_flags_problems() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "character -style human -n hero; rigAnimate -anim idle -s 1 -e 24 hero").unwrap();
        let r = app.rig_report();
        let t = r.text();
        assert!(t.contains("Skeleton 'hips'") && t.contains("L_arm_ik: L_shoulder → L_wrist (2 bones), pole L_elbow_pole"), "{t}");
        assert!(t.contains("L_hand_ctrl: ") && t.contains("pointConstraint → L_arm_ik"), "{t}");
        assert!(t.contains("Mesh 'hero'") && t.contains("skinCluster"), "{t}");
        assert!(!r.issues.iter().any(|i| i.0 == super::Severity::Error), "a fresh Quick Rig is clean: {t}");
        // Break things: unnormalised weights, a missing pole, a keyed constrained node.
        let hero = app.scene.find_by_name("hero").unwrap();
        if let Some(sk) = app.scene.node_mut(hero).unwrap().skin.as_mut() {
            sk.weights[0] = vec![(0, 0.3)];
            sk.weights[1] = vec![];
        }
        let lik = app.scene.find_by_name("L_leg_ik").unwrap();
        if let Some(crate::scene::Shape::IkHandle { pole, .. }) = app.scene.node_mut(lik).map(|n| &mut n.shape) {
            *pole = None;
        }
        execute(&mut app, "setKeyframe -at translateX -v 0 L_leg_ik").unwrap();
        let r = app.rig_report();
        let t = r.text();
        assert!(t.contains("2 vertices aren't fully weighted"), "{t}");
        assert!(t.contains("L_leg_ik has no pole vector"), "{t}");
        assert!(t.contains("L_leg_ik is keyed and also constrained"), "{t}");
        assert!(r.fixes.iter().any(|f| f == "skinPercent -normalize hero"));
        // The fix works.
        execute(&mut app, "skinPercent -normalize hero").unwrap();
        assert!(!app.rig_report().text().contains("fully weighted"));
        assert!(execute(&mut app, "rigReport").unwrap().contains("Issues:"));
    }

    #[test]
    fn unoriented_joints_and_asymmetry() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "joint -p 0 0 0 -n L_a; joint -p 1 1 0 -n L_b; joint -p 2 1.5 0 -n L_c").unwrap();
        let t = app.rig_report().text();
        assert!(t.contains("aren't oriented along their bone"), "{t}");
        assert!(t.contains("Sided names without a mirror partner"), "{t}");
        assert!(t.contains("no skinned mesh"));
        execute(&mut app, "orientJoint L_a").unwrap();
        assert!(!app.rig_report().text().contains("aren't oriented"));
    }
}
