//! Animation tools: key selection / tangents / copy-paste (Graph Editor, Dope Sheet), ghosting,
//! motion trails, the pose library and skeleton retargeting.

use crate::app::App;
use crate::render::GpuVertex;
use crate::rig;
use crate::scene::{Channel, Curve, NodeId, Scene, Shape, TanKind};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A key identified by its curve and frame (frames are unique within a curve).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyRef {
    pub node: NodeId,
    pub ch: Channel,
    pub frame: f32,
}

#[derive(Clone, Debug)]
pub struct GhostSettings {
    pub before: u32,
    pub after: u32,
    /// Frames between ghosts.
    pub step: u32,
}

impl Default for GhostSettings {
    fn default() -> Self {
        GhostSettings { before: 3, after: 3, step: 3 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Pose {
    pub name: String,
    /// (node name, channel, value)
    pub values: Vec<(String, Channel, f32)>,
}

/// Copied keys: (node, channel, frame offset from the first copied key, value, tangent).
pub type KeyClipboard = Vec<(NodeId, Channel, f32, f32, Option<crate::scene::KeyTan>)>;

fn idx(c: &Curve, frame: f32) -> Option<usize> {
    c.keys.iter().position(|k| (k.frame - frame).abs() < 1e-4)
}

/// Swap L/R naming (L_, _L, Left, left...) for mirroring poses and retarget sides.
pub fn mirror_name(n: &str) -> String {
    for (a, b) in [("L_", "R_"), ("R_", "L_"), ("_L", "_R"), ("_R", "_L"), ("Left", "Right"), ("Right", "Left"), ("left", "right"), ("right", "left"), (".L", ".R"), (".R", ".L")] {
        if n.starts_with(a) && (a.ends_with('_') || a.chars().next().unwrap().is_lowercase() || a == "Left" || a == "Right") {
            return format!("{b}{}", &n[a.len()..]);
        }
        if n.ends_with(a) && (a.starts_with('_') || a.starts_with('.')) {
            return format!("{}{b}", &n[..n.len() - a.len()]);
        }
    }
    for (a, b) in [("Left", "Right"), ("Right", "Left"), ("left", "right"), ("right", "left")] {
        if n.contains(a) {
            return n.replacen(a, b, 1);
        }
    }
    n.to_string()
}

impl App {
    // ---------------------------------------------------------------- keys

    /// Animated curves of the selected objects.
    pub fn selected_curves(&self) -> Vec<(NodeId, Channel)> {
        self.sel.iter().filter_map(|id| self.scene.node(*id)).flat_map(|n| n.anim.keys().map(move |c| (n.id, *c))).collect()
    }

    pub fn key_value(&self, k: &KeyRef) -> Option<f32> {
        let c = self.scene.node(k.node)?.anim.get(&k.ch)?;
        idx(c, k.frame).map(|i| c.keys[i].value)
    }

    /// Set the tangent type of the selected keys (all keys of the selected objects if none).
    pub fn set_key_tangents(&mut self, kind: TanKind) -> usize {
        let keys = self.effective_key_sel();
        if keys.is_empty() {
            return 0;
        }
        self.checkpoint();
        let mut n = 0;
        for k in keys {
            if let Some(c) = self.scene.node_mut(k.node).and_then(|nd| nd.anim.get_mut(&k.ch))
                && let Some(i) = idx(c, k.frame)
            {
                c.set_key_kind(i, kind);
                n += 1;
            }
        }
        self.scene.evaluate();
        n
    }

    /// Selected keys, or every key of the selected objects' curves when nothing is selected.
    pub fn effective_key_sel(&self) -> Vec<KeyRef> {
        if !self.key_sel.is_empty() {
            return self.key_sel.clone();
        }
        self.selected_curves()
            .into_iter()
            .flat_map(|(n, ch)| self.scene.node(n).and_then(|nd| nd.anim.get(&ch)).map(|c| c.keys.iter().map(|k| KeyRef { node: n, ch, frame: k.frame }).collect::<Vec<_>>()).unwrap_or_default())
            .collect()
    }

    pub fn delete_selected_keys(&mut self) -> usize {
        if self.key_sel.is_empty() {
            return 0;
        }
        self.checkpoint();
        let n = self.key_sel.len();
        for k in std::mem::take(&mut self.key_sel) {
            if let Some(nd) = self.scene.node_mut(k.node) {
                if let Some(c) = nd.anim.get_mut(&k.ch) {
                    c.remove_key(k.frame);
                }
                nd.anim.retain(|_, c| !c.keys.is_empty());
            }
        }
        self.scene.evaluate();
        n
    }

    /// Move keys from their `orig` positions by (dt frames, dv value). Frames are rounded to
    /// whole frames; keys landing on an existing key replace it. Updates the selection.
    pub fn move_keys(&mut self, orig: &[(KeyRef, f32)], dt: f32, dv: f32) {
        let dt = dt.round();
        // Remove all moved keys first, then re-insert (so keys can pass each other).
        let mut saved = Vec::new();
        for (k, _) in orig {
            if let Some(c) = self.scene.node_mut(k.node).and_then(|n| n.anim.get_mut(&k.ch))
                && let Some(i) = idx(c, k.frame)
            {
                saved.push((*k, c.keys.remove(i)));
            }
        }
        let mut new_sel = Vec::new();
        for ((k, v0), (_, key)) in orig.iter().zip(saved) {
            let frame = k.frame + dt;
            if let Some(c) = self.scene.node_mut(k.node).and_then(|n| n.anim.get_mut(&k.ch)) {
                c.keys.retain(|x| (x.frame - frame).abs() >= 1e-4);
                c.keys.push(crate::scene::Key { frame, value: v0 + dv, tan: key.tan });
                c.keys.sort_by(|a, b| a.frame.total_cmp(&b.frame));
            }
            new_sel.push(KeyRef { node: k.node, ch: k.ch, frame });
        }
        self.key_sel = new_sel;
        self.scene.evaluate();
    }

    /// Copy the selected keys (or all keys of the selected objects).
    pub fn copy_keys(&mut self) -> usize {
        let keys = self.effective_key_sel();
        let t0 = keys.iter().map(|k| k.frame).fold(f32::MAX, f32::min);
        self.key_clipboard = keys
            .iter()
            .filter_map(|k| {
                let c = self.scene.node(k.node)?.anim.get(&k.ch)?;
                let key = c.keys[idx(c, k.frame)?];
                Some((k.node, k.ch, k.frame - t0, key.value, key.tan))
            })
            .collect();
        self.key_clipboard.len()
    }

    /// Paste copied keys starting at `frame` (onto the same curves, or onto the selected
    /// objects when exactly one object was copied).
    pub fn paste_keys(&mut self, frame: f32) -> usize {
        if self.key_clipboard.is_empty() {
            return 0;
        }
        self.checkpoint();
        let src_nodes: HashSet<NodeId> = self.key_clipboard.iter().map(|k| k.0).collect();
        let targets: Vec<NodeId> = if src_nodes.len() == 1 && !self.sel.is_empty() { self.sel.clone() } else { Vec::new() };
        let mut n = 0;
        for (node, ch, off, v, tan) in self.key_clipboard.clone() {
            let dests = if targets.is_empty() { vec![node] } else { targets.clone() };
            for d in dests {
                if let Some(nd) = self.scene.node_mut(d) {
                    let c = nd.anim.entry(ch).or_default();
                    c.set_key(frame + off, v);
                    if let Some(i) = idx(c, frame + off) {
                        c.keys[i].tan = tan;
                    }
                    n += 1;
                }
            }
        }
        self.scene.evaluate();
        n
    }

    // ------------------------------------------------------------ ghosts / trails

    /// Toggle ghosting on the selected objects.
    pub fn toggle_ghost(&mut self) -> bool {
        let sel: Vec<NodeId> = self.sel.clone();
        let on = sel.iter().any(|s| !self.ghosts.contains(s));
        for s in sel {
            if on { self.ghosts.insert(s) } else { self.ghosts.remove(&s) };
        }
        on
    }

    pub fn toggle_trail(&mut self) -> bool {
        let sel: Vec<NodeId> = self.sel.clone();
        let on = sel.iter().any(|s| !self.trails.contains(s));
        for s in sel {
            if on { self.trails.insert(s) } else { self.trails.remove(&s) };
        }
        on
    }

    /// Hash of all animation inputs (for caching ghosts / trails).
    pub fn anim_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for n in &self.scene.nodes {
            n.id.hash(&mut h);
            format!("{:?}{:?}{:?}{:?}{:?}{:?}", n.anim, n.translate, n.rotate, n.scale, n.constraints.len(), n.driven.len()).hash(&mut h);
            if let Some(b) = &n.blend {
                format!("{:?}{:?}", b.weights, b.curves).hash(&mut h);
            }
        }
        (self.scene.time.to_bits(), self.scene.play_start.to_bits(), self.scene.play_end.to_bits()).hash(&mut h);
        format!("{:?}", self.ghost_settings).hash(&mut h);
        let mut g: Vec<_> = self.ghosts.iter().collect();
        g.sort();
        let mut t: Vec<_> = self.trails.iter().collect();
        t.sort();
        (g, t).hash(&mut h);
        h.finish()
    }

    /// Meshes drawn as ghosts for a ghosted node: its mesh subtree plus meshes skinned to
    /// joints in its subtree (so ghosting a rig control or root ghosts the character).
    fn ghost_meshes(&self, id: NodeId) -> Vec<NodeId> {
        let sub: HashSet<NodeId> = self.scene.subtree(id).into_iter().collect();
        let mut out: Vec<NodeId> = sub.iter().copied().filter(|n| self.scene.node(*n).is_some_and(|x| x.mesh().is_some())).collect();
        // Joints driven by this control (constraints / IK handles constrained to it) are hard to
        // trace; include skinned meshes whose joints are in the subtree.
        for n in &self.scene.nodes {
            if let Some(sk) = &n.skin
                && sk.joints.iter().any(|j| sub.contains(j))
                && !out.contains(&n.id)
            {
                out.push(n.id);
            }
        }
        if out.is_empty() {
            // A control that drives a rig: ghost every skinned mesh.
            out = self.scene.nodes.iter().filter(|n| n.skin.is_some()).map(|n| n.id).collect();
        }
        out
    }

    /// Ghost wire lines and motion-trail overlay lines for the current frame.
    pub fn anim_overlays(&mut self) -> (Vec<GpuVertex>, Vec<GpuVertex>) {
        if self.ghosts.is_empty() && self.trails.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let key = self.anim_fingerprint();
        if let Some((k, w, o)) = &self.anim_overlay_cache
            && *k == key
        {
            return (w.clone(), o.clone());
        }
        self.ghosts.retain(|g| self.scene.node(*g).is_some());
        self.trails.retain(|g| self.scene.node(*g).is_some());
        let mut wire = Vec::new();
        let mut over = Vec::new();
        let now = self.scene.time;
        let mut eval = self.scene.clone();
        let sample = |sc: &mut Scene, t: f32| {
            sc.time = t;
            sc.evaluate();
            rig::evaluate(sc);
        };
        // Ghosts.
        if !self.ghosts.is_empty() {
            let gs = self.ghost_settings.clone();
            let meshes: Vec<NodeId> = {
                let mut v: Vec<NodeId> = self.ghosts.iter().flat_map(|g| self.ghost_meshes(*g)).collect();
                v.sort();
                v.dedup();
                v
            };
            let mut frames: Vec<(f32, [f32; 3])> = Vec::new();
            for k in 1..=gs.before {
                let a = 1.0 - k as f32 / (gs.before + 1) as f32;
                frames.push((now - (k * gs.step) as f32, [0.25 + 0.2 * a, 0.45 + 0.3 * a, 0.95]));
            }
            for k in 1..=gs.after {
                let a = 1.0 - k as f32 / (gs.after + 1) as f32;
                frames.push((now + (k * gs.step) as f32, [0.95, 0.55 + 0.25 * a, 0.2 + 0.2 * a]));
            }
            for (t, col) in frames {
                sample(&mut eval, t);
                for &m in &meshes {
                    let Some(mesh) = eval.node(m).and_then(|n| n.mesh()) else { continue };
                    let pos: Vec<Vec3> = rig::deformed_positions(&eval, m).unwrap_or_else(|| {
                        let w = eval.world_matrix(m);
                        mesh.positions.iter().map(|p| w.transform_point3(*p)).collect()
                    });
                    for (a, b) in mesh.edges() {
                        wire.push(GpuVertex::line(pos[a as usize], col));
                        wire.push(GpuVertex::line(pos[b as usize], col));
                    }
                }
            }
        }
        // Motion trails: world position over the playback range, ticks every frame, key markers.
        if !self.trails.is_empty() {
            let (s, e) = (self.scene.play_start, self.scene.play_end);
            let ids: Vec<NodeId> = self.trails.iter().copied().collect();
            let mut paths: HashMap<NodeId, Vec<Vec3>> = HashMap::new();
            let mut f = s;
            while f <= e + 1e-4 {
                sample(&mut eval, f);
                for &id in &ids {
                    paths.entry(id).or_default().push(rig::world_pos(&eval, id));
                }
                f += 1.0;
            }
            for (id, pts) in &paths {
                let keyframes: HashSet<i64> = self.scene.node(*id).map(|n| n.anim.values().flat_map(|c| c.keys.iter().map(|k| k.frame.round() as i64)).collect()).unwrap_or_default();
                let size = pts.windows(2).map(|w| w[0].distance(w[1])).fold(0.0f32, f32::max).max(0.02) * 0.6;
                for (i, w) in pts.windows(2).enumerate() {
                    let fr = s + i as f32;
                    let c = if fr < now { [0.95, 0.75, 0.2] } else { [0.4, 0.8, 1.0] };
                    over.push(GpuVertex::line(w[0], c));
                    over.push(GpuVertex::line(w[1], c));
                }
                for (i, p) in pts.iter().enumerate() {
                    let fr = (s + i as f32).round() as i64;
                    let (c, r) = if keyframes.contains(&fr) { ([1.0, 0.25, 0.25], size) } else { ([0.85, 0.85, 0.85], size * 0.35) };
                    for ax in [Vec3::X, Vec3::Y, Vec3::Z] {
                        over.push(GpuVertex::line(*p - ax * r, c));
                        over.push(GpuVertex::line(*p + ax * r, c));
                    }
                }
            }
        }
        self.anim_overlay_cache = Some((key, wire.clone(), over.clone()));
        (wire, over)
    }

    /// Frames and world positions of a trail node's translate keys (for dragging).
    pub fn trail_keys(&self, id: NodeId) -> Vec<(f32, Vec3)> {
        let Some(n) = self.scene.node(id) else { return Vec::new() };
        let mut frames: Vec<f32> = n.anim.iter().filter(|(c, _)| Channel::translate().contains(c)).flat_map(|(_, c)| c.keys.iter().map(|k| k.frame)).collect();
        frames.sort_by(|a, b| a.total_cmp(b));
        frames.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        let mut eval = self.scene.clone();
        frames
            .into_iter()
            .map(|f| {
                eval.time = f;
                eval.evaluate();
                rig::evaluate(&mut eval);
                (f, rig::world_pos(&eval, id))
            })
            .collect()
    }

    /// Move a trail key: set the node's translate keys at `frame` so it sits at `world`.
    pub fn set_trail_key(&mut self, id: NodeId, frame: f32, world: Vec3, live: bool) {
        if !live {
            self.checkpoint();
        }
        let mut eval = self.scene.clone();
        eval.time = frame;
        eval.evaluate();
        rig::evaluate(&mut eval);
        let local = eval.parent_matrix(id).inverse().transform_point3(world);
        if let Some(n) = self.scene.node_mut(id) {
            for (i, ch) in Channel::translate().into_iter().enumerate() {
                n.anim.entry(ch).or_default().set_key(frame, local[i]);
            }
        }
        self.scene.evaluate();
    }

    // ---------------------------------------------------------------- poses

    /// Nodes a pose covers: the selection plus, with `hierarchy`, everything below it.
    fn pose_nodes(&self, hierarchy: bool) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = if hierarchy { self.sel.iter().flat_map(|s| self.scene.subtree(*s)).collect() } else { self.sel.clone() };
        v.retain(|id| self.scene.node(*id).is_some_and(|n| !matches!(n.shape, Shape::Poly { .. }) || n.anim.is_empty()));
        v.dedup();
        v
    }

    pub fn save_pose(&mut self, name: &str, hierarchy: bool) -> Result<usize, String> {
        let nodes = self.pose_nodes(hierarchy);
        if nodes.is_empty() {
            return Err("Save Pose: select the controls (or the rig root with 'hierarchy').".into());
        }
        let mut values = Vec::new();
        for id in nodes {
            let n = self.scene.node(id).unwrap();
            for ch in Channel::ALL {
                if ch != Channel::Visibility {
                    values.push((n.name.clone(), ch, n.get(ch)));
                }
            }
        }
        self.checkpoint();
        let name = if name.trim().is_empty() { format!("pose{}", self.scene.poses.len() + 1) } else { name.trim().to_string() };
        let count = values.len() / 9;
        self.scene.poses.retain(|p| p.name != name);
        self.scene.poses.push(Pose { name, values });
        Ok(count)
    }

    /// Apply a pose blended with the current values (`weight` 0..1); optionally key it and/or
    /// mirror it (L/R names swapped, TX / RY / RZ negated).
    pub fn apply_pose(&mut self, name: &str, weight: f32, key: bool, mirror: bool) -> Result<usize, String> {
        let pose = self.scene.poses.iter().find(|p| p.name == name).cloned().ok_or(format!("No pose named '{name}'"))?;
        self.checkpoint();
        let mut touched: HashSet<NodeId> = HashSet::new();
        let mut keyed: Vec<(NodeId, Channel)> = Vec::new();
        for (node, ch, v) in &pose.values {
            let (target, v) = if mirror {
                let m = mirror_name(node);
                let flip = matches!(ch, Channel::TranslateX | Channel::RotateY | Channel::RotateZ);
                (m, if flip { -*v } else { *v })
            } else {
                (node.clone(), *v)
            };
            let Some(id) = self.scene.find_by_name(&target) else { continue };
            let n = self.scene.node_mut(id).unwrap();
            let cur = n.get(*ch);
            n.set(*ch, cur + (v - cur) * weight.clamp(0.0, 1.0));
            touched.insert(id);
            keyed.push((id, *ch));
        }
        if key {
            let t = self.scene.time;
            for (id, ch) in keyed {
                if let Some(n) = self.scene.node_mut(id) {
                    let v = n.get(ch);
                    n.anim.entry(ch).or_default().set_key(t, v);
                }
            }
        }
        Ok(touched.len())
    }

    pub fn delete_pose(&mut self, name: &str) {
        self.checkpoint();
        self.scene.poses.retain(|p| p.name != name);
    }

    // ------------------------------------------------------------- retarget

    /// Retarget the animation of one skeleton onto another (joints matched by name and body
    /// part), baking rotations on every frame of `start..=end` and the hips translation
    /// (scaled by the hip-height ratio). IK handles and constraints that drive the target
    /// joints are removed so the baked keys play back. Returns a summary.
    pub fn retarget(&mut self, src_root: NodeId, dst_root: NodeId, start: f32, end: f32) -> Result<String, String> {
        let src = skeleton(&self.scene, src_root);
        let dst = skeleton(&self.scene, dst_root);
        if src.is_empty() || dst.is_empty() {
            return Err("Retarget: pick the root of a source skeleton and of a target skeleton.".into());
        }
        let map = auto_map(&self.scene, &src, &dst);
        if map.is_empty() {
            return Err("Retarget: no joints could be matched by name.".into());
        }
        if end < start {
            return Err("Retarget: empty frame range.".into());
        }
        self.begin_batch();
        self.checkpoint();
        // Free the target joints from IK / constraints.
        let dst_set: HashSet<NodeId> = dst.iter().copied().collect();
        let mut removed = 0;
        let ik: Vec<NodeId> = self
            .scene
            .nodes
            .iter()
            .filter(|n| match &n.shape {
                Shape::IkHandle { start, end, .. } => dst_set.contains(start) || dst_set.contains(end),
                _ => false,
            })
            .map(|n| n.id)
            .collect();
        removed += ik.len();
        self.scene.delete(&ik);
        for j in &dst {
            if let Some(n) = self.scene.node_mut(*j) {
                removed += n.constraints.len();
                n.constraints.clear();
            }
        }
        // Rest poses (bind pose if skinned, else the current pose without animation).
        let src_rest = rest_scene(&self.scene, &src);
        let dst_rest = rest_scene(&self.scene, &dst);
        let rot = |sc: &Scene, id: NodeId| rig::world_rotation(sc, id);
        let rs_rest: HashMap<NodeId, Quat> = src.iter().map(|j| (*j, rot(&src_rest, *j))).collect();
        let rt_rest: HashMap<NodeId, Quat> = dst.iter().map(|j| (*j, rot(&dst_rest, *j))).collect();
        // Hips = the first mapped pair in hierarchy order.
        let (src_hips, dst_hips) = map[0];
        let src_h0 = rig::world_pos(&src_rest, src_hips);
        let dst_h0 = rig::world_pos(&dst_rest, dst_hips);
        let src_floor = src.iter().map(|j| rig::world_pos(&src_rest, *j).y).fold(f32::MAX, f32::min);
        let dst_floor = dst.iter().map(|j| rig::world_pos(&dst_rest, *j).y).fold(f32::MAX, f32::min);
        let ratio = {
            let (a, b) = (src_h0.y - src_floor, dst_h0.y - dst_floor);
            if a > 1e-4 && b > 1e-4 { b / a } else { 1.0 }
        };
        let mapped: HashMap<NodeId, NodeId> = map.iter().map(|(s, d)| (*d, *s)).collect();
        // Facing correction: rotate source deltas about Y so the source's forward matches the target's.
        let facing = |sc: &Scene, joints: &[NodeId]| -> Option<Vec3> {
            for part in ["upperleg", "upperarm", "foot", "hand", "clavicle"] {
                let find = |side: char| joints.iter().copied().find(|j| sc.node(*j).is_some_and(|n| body_part(&n.name) == (part.to_string(), side)) || chain_part(sc, joints, *j) == Some((part, side)));
                if let (Some(l), Some(r)) = (find('L'), find('R')) {
                    let side = rig::world_pos(sc, l) - rig::world_pos(sc, r);
                    let fwd = Vec3::new(side.x, 0.0, side.z).cross(Vec3::Y);
                    if fwd.length() > 1e-5 {
                        return Some(fwd.normalize());
                    }
                }
            }
            None
        };
        let face_q = match (facing(&src_rest, &src), facing(&dst_rest, &dst)) {
            (Some(a), Some(b)) => Quat::from_rotation_arc(a, b),
            _ => Quat::IDENTITY,
        };
        // Working copies.
        let mut src_sc = self.scene.clone();
        let mut dst_sc = dst_rest.clone();
        let mut prev_euler: HashMap<NodeId, Vec3> = HashMap::new();
        let mut keys: HashMap<(NodeId, Channel), Curve> = HashMap::new();
        let mut f = start;
        while f <= end + 1e-4 {
            src_sc.time = f;
            src_sc.evaluate();
            rig::evaluate(&mut src_sc);
            // Hips translation (world delta scaled), then rotations parent-first.
            let delta = face_q * (rig::world_pos(&src_sc, src_hips) - src_h0) * ratio;
            rig::set_world_position(&mut dst_sc, dst_hips, dst_h0 + delta);
            let t = dst_sc.node(dst_hips).unwrap().translate;
            for (i, ch) in Channel::translate().into_iter().enumerate() {
                keys.entry((dst_hips, ch)).or_default().set_key(f, t[i]);
            }
            for &j in &dst {
                let Some(&s) = mapped.get(&j) else { continue };
                let desired = face_q * (rot(&src_sc, s) * rs_rest[&s].inverse()) * face_q.inverse() * rt_rest[&j];
                rig::set_world_rotation(&mut dst_sc, j, desired);
                let n = dst_sc.node(j).unwrap();
                let e = unwrap_euler(prev_euler.get(&j).copied().unwrap_or(n.rotate), n.rotate);
                prev_euler.insert(j, e);
                for (i, ch) in Channel::rotate().into_iter().enumerate() {
                    keys.entry((j, ch)).or_default().set_key(f, e[i]);
                }
            }
            f += 1.0;
        }
        let n_curves = keys.len();
        for ((id, ch), c) in keys {
            if let Some(n) = self.scene.node_mut(id) {
                n.anim.insert(ch, c);
            }
        }
        self.scene.play_start = self.scene.play_start.min(start);
        self.scene.play_end = self.scene.play_end.max(end);
        let t = self.scene.time;
        self.set_time(t);
        self.end_batch();
        let names: Vec<String> = map.iter().take(6).map(|(s, d)| format!("{}→{}", self.scene.node(*s).map(|n| n.name.as_str()).unwrap_or("?"), self.scene.node(*d).map(|n| n.name.as_str()).unwrap_or("?"))).collect();
        Ok(format!(
            "Retargeted {} joints over frames {start}–{end} ({n_curves} curves, hip scale {ratio:.2}){}: {}{}",
            map.len(),
            if removed > 0 { format!(", removed {removed} IK handle(s)/constraint(s) on the target") } else { String::new() },
            names.join(", "),
            if map.len() > 6 { ", …" } else { "" }
        ))
    }
}

fn unwrap_euler(prev: Vec3, mut e: Vec3) -> Vec3 {
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

/// Joints in the hierarchy under `root` (root included when it's a joint), parents first.
pub fn skeleton(sc: &Scene, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if rig::is_joint(sc, id) {
            out.push(id);
        }
        let mut kids = sc.children(Some(id));
        kids.reverse();
        stack.extend(kids);
    }
    out
}

/// A copy of the scene with the skeleton in its rest pose (bind pose of a skin that uses it,
/// else the static pose with animation curves ignored).
fn rest_scene(sc: &Scene, joints: &[NodeId]) -> Scene {
    let mut s = sc.clone();
    let set: HashSet<NodeId> = joints.iter().copied().collect();
    if let Some(skin) = sc.nodes.iter().filter_map(|n| n.skin.as_ref()).find(|sk| sk.joints.iter().any(|j| set.contains(j))) {
        rig::goto_bind_pose(&mut s, skin);
    }
    s
}

/// Part of a generically named limb chain ("leg_joint_L_2" → upperleg): the n-th joint by depth.
fn chain_part(sc: &Scene, joints: &[NodeId], j: NodeId) -> Option<(&'static str, char)> {
    let n = sc.node(j)?;
    let (p, side) = body_part(&n.name);
    if p != "other" || side == 'C' {
        return None;
    }
    let lname = n.name.to_ascii_lowercase();
    let limb = if lname.contains("leg") { "leg" } else if lname.contains("arm") { "arm" } else { return None };
    let depth = |id: NodeId| {
        let mut d = 0;
        let mut cur = sc.node(id).and_then(|x| x.parent);
        while let Some(p) = cur {
            d += 1;
            cur = sc.node(p).and_then(|x| x.parent);
        }
        d
    };
    let mut chain: Vec<NodeId> = joints.iter().copied().filter(|x| sc.node(*x).is_some_and(|m| m.name.to_ascii_lowercase().contains(limb) && body_part(&m.name) == ("other".to_string(), side))).collect();
    chain.sort_by_key(|x| depth(*x));
    let k = chain.iter().position(|x| *x == j)?;
    let parts: &[&'static str] = match (limb, chain.len() >= 4) {
        ("leg", true) => &["upperleg", "lowerleg", "foot", "toe"],
        ("leg", false) => &["upperleg", "lowerleg", "foot"],
        (_, true) => &["clavicle", "upperarm", "forearm", "hand"],
        _ => &["upperarm", "forearm", "hand"],
    };
    parts.get(k).map(|p| (*p, side))
}

/// Body part + side of a joint name, e.g. ("upperarm", 'L').
pub fn body_part(name: &str) -> (String, char) {
    let raw = name.to_ascii_lowercase();
    let raw = raw.rsplit([':', '|']).next().unwrap_or(&raw).to_string();
    let raw = raw.trim_start_matches("mixamorig").trim_start_matches('_').to_string();
    let tokens: Vec<String> = raw.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).map(String::from).collect();
    let mut side = 'C';
    let mut words = Vec::new();
    for t in &tokens {
        match t.as_str() {
            "l" | "left" | "lf" | "lft" => side = 'L',
            "r" | "right" | "rt" | "rgt" => side = 'R',
            _ => words.push(t.clone()),
        }
    }
    let mut joined = words.join("");
    for (p, sd) in [("left", 'L'), ("right", 'R')] {
        if joined.contains(p) {
            side = sd;
            joined = joined.replacen(p, "", 1);
        }
    }
    let j = joined.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    let part = if j.contains("clavicle") || j.contains("collar") {
        "clavicle"
    } else if j.contains("forearm") || j.contains("lowerarm") || j.contains("elbow") {
        "forearm"
    } else if j.contains("upperarm") || j == "arm" || j.ends_with("arm") {
        "upperarm"
    } else if j.contains("shoulder") {
        "shoulder"
    } else if j.contains("hand") || j.contains("wrist") {
        if j.contains("end") || j.contains("top") { "handend" } else { "hand" }
    } else if j.contains("thigh") || j.contains("upleg") || j.contains("upperleg") || (j.contains("hip") && side != 'C') {
        "upperleg"
    } else if j.contains("calf") || j.contains("shin") || j.contains("knee") || j.contains("lowerleg") || j == "leg" || j.ends_with("leg") {
        "lowerleg"
    } else if j.contains("foot") || j.contains("ankle") {
        "foot"
    } else if j.contains("toe") || j.contains("ball") {
        "toe"
    } else if j.contains("hips") || j.contains("pelvis") || j == "hip" {
        "hips"
    } else if j.contains("neck") {
        "neck"
    } else if j.contains("head") {
        if j.contains("end") || j.contains("top") { "headend" } else { "head" }
    } else if j.contains("spine") || j.contains("chest") || j.contains("torso") || j.contains("back") {
        "spine"
    } else {
        "other"
    };
    (part.to_string(), side)
}

/// Match source joints to target joints: exact names first, then body part + side; spine
/// segments are matched proportionally; a "shoulder" is a clavicle when the same side also
/// has a separate upper arm. Returns pairs in target hierarchy order (hips first when found).
pub fn auto_map(sc: &Scene, src: &[NodeId], dst: &[NodeId]) -> Vec<(NodeId, NodeId)> {
    let name = |id: NodeId| sc.node(id).map(|n| n.name.clone()).unwrap_or_default();
    let classify = |list: &[NodeId]| -> Vec<(NodeId, String, char)> {
        let raw: Vec<(NodeId, String, char)> = list.iter().map(|j| {
            let (p, s) = body_part(&name(*j));
            (*j, p, s)
        }).collect();
        raw.iter()
            .map(|(j, p, s)| {
                let p = if p == "shoulder" {
                    if raw.iter().any(|(_, p2, s2)| p2 == "upperarm" && s2 == s) { "clavicle".to_string() } else { "upperarm".to_string() }
                } else {
                    p.clone()
                };
                (*j, p, *s)
            })
            .collect()
    };
    // Generic chains ("leg_joint_L_1..4", "arm_joint_R_2..3"): assign parts by depth order.
    let chains = |mut c: Vec<(NodeId, String, char)>| -> Vec<(NodeId, String, char)> {
        let depth = |id: NodeId| {
            let mut d = 0;
            let mut cur = sc.node(id).and_then(|n| n.parent);
            while let Some(p) = cur {
                d += 1;
                cur = sc.node(p).and_then(|n| n.parent);
            }
            d
        };
        for (limb, parts3, parts4) in [("leg", ["upperleg", "lowerleg", "foot"], ["upperleg", "lowerleg", "foot", "toe"]), ("arm", ["upperarm", "forearm", "hand"], ["clavicle", "upperarm", "forearm", "hand"])] {
            for side in ['L', 'R'] {
                let mut idx: Vec<usize> = (0..c.len()).filter(|i| c[*i].1 == "other" && c[*i].2 == side && name(c[*i].0).to_ascii_lowercase().contains(limb)).collect();
                if idx.len() < 2 {
                    continue;
                }
                idx.sort_by_key(|i| depth(c[*i].0));
                let parts: &[&str] = if idx.len() >= 4 { &parts4 } else { &parts3 };
                for (k, i) in idx.iter().enumerate().take(parts.len()) {
                    c[*i].1 = parts[k].to_string();
                }
            }
        }
        c
    };
    let cs = chains(classify(src));
    let cd = chains(classify(dst));
    let mut pairs: Vec<(NodeId, NodeId)> = Vec::new();
    let mut used_s: HashSet<NodeId> = HashSet::new();
    let mut used_d: HashSet<NodeId> = HashSet::new();
    // Exact names.
    for &d in dst {
        if let Some(&s) = src.iter().find(|s| name(**s).eq_ignore_ascii_case(&name(d))) {
            pairs.push((s, d));
            used_s.insert(s);
            used_d.insert(d);
        }
    }
    // Hips: a joint named like hips/pelvis, else the skeleton roots.
    let hips_of = |c: &[(NodeId, String, char)], list: &[NodeId]| c.iter().find(|(_, p, _)| p == "hips").map(|x| x.0).or(list.first().copied());
    if let (Some(hs), Some(hd)) = (hips_of(&cs, src), hips_of(&cd, dst))
        && !used_s.contains(&hs)
        && !used_d.contains(&hd)
    {
        pairs.push((hs, hd));
        used_s.insert(hs);
        used_d.insert(hd);
    }
    // Spine chains: proportional.
    let spine = |c: &[(NodeId, String, char)], used: &HashSet<NodeId>| -> Vec<NodeId> { c.iter().filter(|(j, p, _)| p == "spine" && !used.contains(j)).map(|x| x.0).collect() };
    let (ss, ds) = (spine(&cs, &used_s), spine(&cd, &used_d));
    if !ss.is_empty() {
        for (k, d) in ds.iter().enumerate() {
            let s = ss[((k as f32 + 0.5) / ds.len() as f32 * ss.len() as f32) as usize % ss.len()];
            pairs.push((s, *d));
            used_d.insert(*d);
        }
        used_s.extend(ss);
    }
    // Body parts.
    for (d, p, sd) in &cd {
        if used_d.contains(d) || p == "other" || p == "spine" {
            continue;
        }
        if let Some((s, _, _)) = cs.iter().find(|(s, ps, ss)| !used_s.contains(s) && ps == p && ss == sd) {
            pairs.push((*s, *d));
            used_s.insert(*s);
            used_d.insert(*d);
        }
    }
    // Hierarchy order of the target; hips first.
    let order: HashMap<NodeId, usize> = dst.iter().enumerate().map(|(i, d)| (*d, i)).collect();
    let hd = hips_of(&cd, dst);
    pairs.sort_by_key(|(_, d)| order[d]);
    if let Some(i) = pairs.iter().position(|(_, d)| Some(*d) == hd) {
        let h = pairs.remove(i);
        pairs.insert(0, h);
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn body_parts() {
        assert_eq!(body_part("mixamorig:LeftForeArm"), ("forearm".into(), 'L'));
        assert_eq!(body_part("L_shoulder"), ("shoulder".into(), 'L'));
        assert_eq!(body_part("R_hip"), ("upperleg".into(), 'R'));
        assert_eq!(body_part("hips"), ("hips".into(), 'C'));
        assert_eq!(body_part("thigh.R"), ("upperleg".into(), 'R'));
        assert_eq!(body_part("Spine2"), ("spine".into(), 'C'));
        assert_eq!(body_part("RightToeBase"), ("toe".into(), 'R'));
        assert_eq!(mirror_name("L_hand_ctrl"), "R_hand_ctrl");
        assert_eq!(mirror_name("arm_L"), "arm_R");
        assert_eq!(mirror_name("LeftArm"), "RightArm");
    }

    #[test]
    fn tangents_keys_copy_paste() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "spaceLocator -n a; setKeyframe -t 1 -at ty -v 0 a; setKeyframe -t 10 -at ty -v 10 a; setKeyframe -t 20 -at ty -v 0 a").unwrap();
        let a = app.scene.find_by_name("a").unwrap();
        let c = app.scene.node(a).unwrap().anim[&Channel::TranslateY].clone();
        assert_eq!(c.out_slope(1), 0.0, "auto is flat at a peak");
        execute(&mut app, "select a; keyTangent -itt linear -ott linear").unwrap();
        let c = app.scene.node(a).unwrap().anim[&Channel::TranslateY].clone();
        assert!((c.evaluate(5.5).unwrap() - 5.0).abs() < 1e-4, "linear");
        execute(&mut app, "keyTangent -t 1 -ott step a").unwrap();
        assert_eq!(app.scene.node(a).unwrap().anim[&Channel::TranslateY].evaluate(5.0), Some(0.0));
        // Move a key with the Graph Editor API.
        app.key_sel = vec![KeyRef { node: a, ch: Channel::TranslateY, frame: 10.0 }];
        app.move_keys(&[(app.key_sel[0], 10.0)], 2.4, 1.0);
        let c = &app.scene.node(a).unwrap().anim[&Channel::TranslateY];
        assert!(c.has_key_at(12.0) && (c.keys[1].value - 11.0).abs() < 1e-5);
        // Copy all keys, paste at 100 onto another locator.
        app.key_sel.clear();
        assert_eq!(app.copy_keys(), 3);
        execute(&mut app, "spaceLocator -n b; select b").unwrap();
        assert_eq!(app.paste_keys(100.0), 3);
        let b = app.scene.find_by_name("b").unwrap();
        assert!(app.scene.node(b).unwrap().anim[&Channel::TranslateY].has_key_at(111.0));
        // Delete selected key.
        app.key_sel = vec![KeyRef { node: b, ch: Channel::TranslateY, frame: 100.0 }];
        assert_eq!(app.delete_selected_keys(), 1);
        assert_eq!(app.scene.node(b).unwrap().anim[&Channel::TranslateY].keys.len(), 2);
    }

    #[test]
    fn ghosts_trails_and_poses() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -n box; setKeyframe -t 1 -at tx -v 0 box; setKeyframe -t 30 -at tx -v 10 box; currentTime 15; select box").unwrap();
        app.toggle_ghost();
        app.toggle_trail();
        let (w, o) = app.anim_overlays();
        assert_eq!(w.len(), 6 * 12 * 2, "6 ghosts × 12 edges");
        assert!(!o.is_empty());
        // Drag a trail key.
        let box_ = app.scene.find_by_name("box").unwrap();
        let tk = app.trail_keys(box_);
        assert_eq!(tk.len(), 2);
        app.set_trail_key(box_, 30.0, Vec3::new(10.0, 5.0, 0.0), false);
        assert_eq!(app.scene.node(box_).unwrap().anim[&Channel::TranslateY].evaluate(30.0), Some(5.0));
        // Poses with mirroring.
        execute(&mut app, "spaceLocator -n L_hand_ctrl; spaceLocator -n R_hand_ctrl; move 1 2 3 L_hand_ctrl; rotate 0 30 0 L_hand_ctrl; select L_hand_ctrl").unwrap();
        app.save_pose("wave", false).unwrap();
        execute(&mut app, "move 0 0 0 L_hand_ctrl").unwrap();
        app.apply_pose("wave", 0.5, false, false).unwrap();
        let l = app.scene.find_by_name("L_hand_ctrl").unwrap();
        assert!((app.scene.node(l).unwrap().translate - Vec3::new(0.5, 1.0, 1.5)).length() < 1e-5);
        app.apply_pose("wave", 1.0, true, true).unwrap();
        let r = app.scene.find_by_name("R_hand_ctrl").unwrap();
        let n = app.scene.node(r).unwrap();
        assert!((n.translate - Vec3::new(-1.0, 2.0, 3.0)).length() < 1e-5 && (n.rotate.y + 30.0).abs() < 1e-4);
        assert!(n.anim.contains_key(&Channel::TranslateX), "keyed");
        let json = serde_json::to_string(&app.scene).unwrap();
        assert!(json.contains("wave"));
    }

    #[test]
    fn retarget_reproduces_motion() {
        let mut app = App::new();
        app.scene.nodes.clear();
        // Source: a little leg chain with animation; target: same shape, different names, offset and 2× size.
        for m in [
            "select -cl; joint -p 0 2 0 -n Hips; joint -p 0.3 1.8 0 -n LeftUpLeg; joint -p 0.3 1 0.05 -n LeftLeg; joint -p 0.3 0.1 0 -n LeftFoot",
            "select -cl; joint -p 5 4 0 -n pelvis; joint -p 5.6 3.6 0 -n thigh_l; joint -p 5.6 2 0.1 -n calf_l; joint -p 5.6 0.2 0 -n foot_l",
            "setKeyframe -t 1 -at rx -v 0 LeftUpLeg; setKeyframe -t 10 -at rx -v -40 LeftUpLeg; setKeyframe -t 1 -at ty -v 2 Hips; setKeyframe -t 10 -at ty -v 2.5 Hips",
        ] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let msg = execute(&mut app, "retarget -src Hips -dst pelvis -s 1 -e 10").unwrap();
        assert!(msg.contains("4 joints"), "{msg}");
        execute(&mut app, "currentTime 10").unwrap();
        crate::rig::evaluate(&mut app.scene);
        let s = &app.scene;
        let dir = |a: &str, b: &str| (rig::world_pos(s, s.find_by_name(b).unwrap()) - rig::world_pos(s, s.find_by_name(a).unwrap())).normalize();
        assert!(dir("LeftUpLeg", "LeftLeg").distance(dir("thigh_l", "calf_l")) < 0.02, "thigh direction matches");
        let hips_dy = rig::world_pos(s, s.find_by_name("pelvis").unwrap()).y - 4.0;
        assert!((hips_dy - 1.0).abs() < 0.02, "hip lift scaled by 2: {hips_dy}");
    }
}

/// SAMPLES=dir cargo test retarget_sample -- --ignored --nocapture
#[cfg(test)]
#[test]
#[ignore]
fn retarget_sample() {
    use crate::commands::execute;
    let Ok(dir) = std::env::var("SAMPLES") else { return };
    for file in ["CesiumMan.glb", "Fox.glb", "RiggedFigure.glb"] {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, &format!("file -import {dir}/{file}")).unwrap();
        let roots = app.sel.clone();
        let joints: Vec<NodeId> = roots.iter().flat_map(|r| skeleton(&app.scene, *r)).collect();
        let names: Vec<String> = joints.iter().map(|j| format!("{}={:?}", app.scene.node(*j).unwrap().name, body_part(&app.scene.node(*j).unwrap().name))).collect();
        println!("{file}: {}", names.join(" "));
        for m in [
            "polyCube -w 0.5 -h 0.8 -d 0.3 -sy 4 -n torso; move 6 1.2 0; polyCube -w 1.6 -h 0.12 -d 0.12 -sx 12 -n arms; move 6 1.47 0",
            "polyCube -w 0.4 -h 0.9 -d 0.15 -sy 6 -sx 2 -n legs; move 6 0.45 0; polyUnite torso arms legs; rename polySurface1 body",
            "quickRig body",
        ] {
            execute(&mut app, m).unwrap();
        }
        let src_root = joints[0];
        let src_name = app.scene.node(src_root).unwrap().name.clone();
        let r = execute(&mut app, &format!("retarget -src {src_name} -dst hips -s 1 -e 30"));
        println!("  → {r:?}");
        execute(&mut app, "currentTime 15").unwrap();
        crate::rig::evaluate(&mut app.scene);
        let ok = app.scene.nodes.iter().all(|n| n.translate.is_finite() && n.rotate.is_finite());
        assert!(ok);
    }
}
