//! Undoable rigging actions on the App (used by menus, shelves, tools, MEL and the AI).

use crate::app::App;
use crate::rig::{self, Constraint, ConstraintKind, DrivenKey};
use crate::scene::{Channel, Curve, NodeId, Shape, Tangent};
use glam::Vec3;

impl App {
    /// Give a freshly created node an explicit name (kept exactly if free, else made unique).
    pub fn apply_name(&mut self, id: NodeId, name: Option<&str>) {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else { return };
        // Taken only if another node has the name (a node keeps its own name, e.g. "Geo2").
        let taken = self.scene.nodes.iter().any(|n| n.id != id && n.name == name);
        let unique = if taken { self.scene.unique_name(name) } else { name.to_string() };
        if let Some(n) = self.scene.node_mut(id) {
            n.name = unique;
        }
    }

    /// End the joint-tool chain: orient it, select its root and switch to the Move tool.
    pub fn finish_joint_chain(&mut self) {
        if let Some(root) = self.joint_chain.take() {
            if self.scene.node(root).is_some() {
                self.orient_joints(&[root]);
                self.sel = vec![root];
                let n = rig::joint_hierarchy(&self.scene, &[root]).len();
                self.info(format!("Joint chain finished ({n} joints, oriented xyz)"));
            }
        }
        if self.tool == crate::app::Tool::Joint {
            self.tool = crate::app::Tool::Move;
        }
    }

    pub fn is_joint(&self, id: NodeId) -> bool {
        rig::is_joint(&self.scene, id)
    }

    /// Create a joint at a world position. If a joint is selected the new joint becomes its
    /// child (Maya's joint tool / `joint` command behaviour). Selects the new joint.
    pub fn create_joint(&mut self, world: Vec3, name: Option<&str>, radius: f32) -> NodeId {
        self.checkpoint();
        let parent = self.lead().filter(|id| self.is_joint(*id));
        let id = self.scene.add_node("joint", Shape::Joint { radius }, parent);
        self.apply_name(id, name);
        rig::set_world_position(&mut self.scene, id, world);
        self.sel = vec![id];
        id
    }

    /// Orient the given joints (and everything below them): X down the bone, Y up.
    pub fn orient_joints(&mut self, roots: &[NodeId]) {
        let joints = rig::joint_hierarchy(&self.scene, roots);
        if joints.is_empty() {
            self.warn("Orient Joint: select joints.");
            return;
        }
        self.checkpoint();
        rig::orient_joints(&mut self.scene, &joints);
    }

    /// Bind meshes to joints (whole hierarchies below the given joints unless `selected_only`).
    pub fn bind_skin(&mut self, joints: &[NodeId], meshes: &[NodeId], selected_only: bool, max_influences: usize) -> Result<String, String> {
        let joints = if selected_only { joints.to_vec() } else { rig::joint_hierarchy(&self.scene, joints) };
        if joints.is_empty() {
            return Err("Bind Skin: select at least one joint and a mesh.".into());
        }
        let meshes: Vec<NodeId> = meshes.iter().copied().filter(|m| self.scene.node(*m).is_some_and(|n| n.mesh().is_some())).collect();
        if meshes.is_empty() {
            return Err("Bind Skin: select a polygon mesh as well as the joints.".into());
        }
        self.checkpoint();
        let mut names = Vec::new();
        for m in meshes {
            // Rebinding: start from the current deformed shape's bind mesh.
            if let Some(skin) = rig::bind_skin(&self.scene, m, &joints, max_influences, 4.0) {
                if let Some(n) = self.scene.node_mut(m) {
                    n.skin = Some(skin);
                    names.push(format!("skinCluster_{}", n.name));
                }
            }
        }
        Ok(names.join(" "))
    }

    pub fn unbind_skin(&mut self, meshes: &[NodeId]) {
        self.checkpoint();
        for m in meshes {
            let skin = self.scene.node(*m).and_then(|n| n.skin.clone());
            if let Some(sk) = skin {
                rig::goto_bind_pose(&mut self.scene, &sk);
            }
            if let Some(n) = self.scene.node_mut(*m) {
                n.skin = None;
            }
        }
    }

    pub fn goto_bind_pose(&mut self, ids: &[NodeId]) {
        let skins: Vec<rig::Skin> = self
            .scene
            .nodes
            .iter()
            .filter(|n| ids.is_empty() || ids.contains(&n.id) || n.skin.as_ref().is_some_and(|s| s.joints.iter().any(|j| ids.contains(j))))
            .filter_map(|n| n.skin.clone())
            .collect();
        if skins.is_empty() {
            self.warn("Go to Bind Pose: no skinned meshes found.");
            return;
        }
        self.checkpoint();
        for s in &skins {
            rig::goto_bind_pose(&mut self.scene, s);
        }
    }

    pub fn create_ik(&mut self, start: NodeId, end: NodeId, name: Option<&str>) -> Result<NodeId, String> {
        if !self.is_joint(start) || !self.is_joint(end) {
            return Err("IK Handle: the start and end must be joints.".into());
        }
        let chain = rig::ik_chain(&self.scene, start, end).ok_or("IK Handle: the end joint must be below the start joint.")?;
        if chain.len() < 3 {
            return Err("IK Handle: the chain needs at least 3 joints (e.g. shoulder → elbow → wrist).".into());
        }
        self.checkpoint();
        let id = self.scene.add_node("ikHandle", Shape::IkHandle { start, end, pole: None, stretch: false, twist: 0.0, rest: Vec::new() }, None);
        self.apply_name(id, name);
        let p = rig::world_pos(&self.scene, end);
        rig::set_world_position(&mut self.scene, id, p);
        if let Some(n) = self.scene.node_mut(id) {
            n.color = [0.95, 0.75, 0.2];
        }
        self.sel = vec![id];
        Ok(id)
    }

    pub fn pole_vector(&mut self, pole: NodeId, handle: NodeId) -> Result<(), String> {
        if !matches!(self.scene.node(handle).map(|n| &n.shape), Some(Shape::IkHandle { .. })) {
            return Err("Pole Vector: select the pole object, then the IK handle.".into());
        }
        self.checkpoint();
        if let Some(Shape::IkHandle { pole: p, .. }) = self.scene.node_mut(handle).map(|n| &mut n.shape) {
            *p = Some(pole);
        }
        Ok(())
    }

    /// Maya convention: drivers first, the constrained object last.
    pub fn constrain(&mut self, kind: ConstraintKind, ids: &[NodeId], maintain_offset: bool, aim: Option<Vec3>, up: Option<Vec3>) -> Result<String, String> {
        if ids.len() < 2 {
            return Err(format!("{}: select the driver object(s), then the object to constrain.", kind.name()));
        }
        let target = *ids.last().unwrap();
        let drivers = ids[..ids.len() - 1].to_vec();
        if drivers.iter().any(|d| *d == target || self.scene.is_ancestor(target, *d)) {
            return Err(format!("{}: a driver can't be the target or one of its children.", kind.name()));
        }
        self.checkpoint();
        let mut c = Constraint::new(&self.scene, kind, drivers, target, maintain_offset);
        if let Some(a) = aim {
            c.aim = a;
        }
        if let Some(u) = up {
            c.up = u;
        }
        let tname = self.scene.node(target).map(|n| n.name.clone()).unwrap_or_default();
        if let Some(n) = self.scene.node_mut(target) {
            // Replace an existing constraint of the same kind (Maya would add targets).
            n.constraints.retain(|x| x.kind != kind);
            n.constraints.push(c);
        }
        rig::evaluate(&mut self.scene);
        Ok(format!("{tname}_{}1", kind.name()))
    }

    pub fn remove_constraints(&mut self, ids: &[NodeId]) {
        self.checkpoint();
        for id in ids {
            if let Some(n) = self.scene.node_mut(*id) {
                n.constraints.clear();
            }
        }
    }

    pub fn create_control(&mut self, shape: &str, size: f32, normal: Option<Vec3>, name: Option<&str>) -> Result<NodeId, String> {
        let (mut points, closed) = rig::control_shape(shape, size).ok_or(format!("Unknown control shape '{shape}' (circle, square, cross, arrow, cube, sphere)"))?;
        if let Some(n) = normal.filter(|n| n.length() > 1e-6 && shape == "circle") {
            points = rig::circle_points(size, n, 16);
        }
        self.checkpoint();
        let id = self.scene.add_node(if shape == "circle" { "nurbsCircle" } else { "ctrl" }, Shape::Curve { points, closed }, None);
        self.apply_name(id, name);
        if let Some(n) = self.scene.node_mut(id) {
            n.color = [0.2, 0.55, 1.0];
        }
        self.sel = vec![id];
        Ok(id)
    }

    /// Key the driven attribute at the driver's current value (or `driver_value`).
    pub fn set_driven_key(&mut self, driver: NodeId, driver_ch: Channel, driven: &[NodeId], channels: &[Channel], driver_value: Option<f32>, value: Option<f32>) -> Result<String, String> {
        if driven.is_empty() || channels.is_empty() {
            return Err("Set Driven Key: choose a driven object and attribute.".into());
        }
        self.checkpoint();
        let dv = driver_value.unwrap_or_else(|| self.scene.node(driver).map(|n| n.get(driver_ch)).unwrap_or(0.0));
        let mut count = 0;
        for &id in driven {
            let Some(n) = self.scene.node_mut(id) else { continue };
            for &ch in channels {
                if let Some(v) = value {
                    n.set(ch, v);
                }
                let v = n.get(ch);
                let idx = match n.driven.iter().position(|d| d.channel == ch && d.driver == driver && d.driver_channel == driver_ch) {
                    Some(i) => i,
                    None => {
                        n.driven.retain(|d| d.channel != ch);
                        n.driven.push(DrivenKey { channel: ch, blend_target: None, driver, driver_channel: driver_ch, curve: Curve { tangent: Tangent::Auto, ..Default::default() } });
                        n.driven.len() - 1
                    }
                };
                n.driven[idx].curve.set_key(dv, v);
                count += 1;
            }
        }
        Ok(format!("{count} driven key(s) at driver value {dv}"))
    }
}

// ------------------------------------------------------------------ Phase 3 rigging

/// Mirror a world position across the plane through the origin normal to `axis` (0=x,1=y,2=z).
pub fn mirror_point(p: Vec3, axis: usize) -> Vec3 {
    let mut q = p;
    q[axis] = -q[axis];
    q
}

/// Uniform-grid nearest-point lookup (used by weight mirroring / blend-shape checks).
struct Grid {
    cell: f32,
    map: std::collections::HashMap<(i32, i32, i32), Vec<usize>>,
    pts: Vec<Vec3>,
}

impl Grid {
    fn new(pts: Vec<Vec3>) -> Self {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let ext = (hi - lo).max_element().max(1e-3);
        let cell = (ext / (pts.len() as f32).cbrt().max(1.0)).max(1e-4);
        let mut map: std::collections::HashMap<(i32, i32, i32), Vec<usize>> = Default::default();
        for (i, p) in pts.iter().enumerate() {
            map.entry(Self::key_of(*p, cell)).or_default().push(i);
        }
        Grid { cell, map, pts }
    }
    fn key_of(p: Vec3, cell: f32) -> (i32, i32, i32) {
        ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32, (p.z / cell).floor() as i32)
    }
    fn nearest(&self, p: Vec3) -> Option<(usize, f32)> {
        let k = Self::key_of(p, self.cell);
        let mut best: Option<(usize, f32)> = None;
        for r in 1..=4i32 {
            for dx in -r..=r {
                for dy in -r..=r {
                    for dz in -r..=r {
                        if let Some(v) = self.map.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                            for &i in v {
                                let d = self.pts[i].distance(p);
                                if best.is_none_or(|b| d < b.1) {
                                    best = Some((i, d));
                                }
                            }
                        }
                    }
                }
            }
            if best.is_some_and(|b| b.1 <= self.cell * (r as f32)) {
                return best;
            }
        }
        best.or_else(|| self.pts.iter().enumerate().map(|(i, q)| (i, q.distance(p))).min_by(|a, b| a.1.total_cmp(&b.1)))
    }
}

impl App {
    /// Create (or extend) a blend shape on `base` from target meshes with the same vertex count.
    pub fn create_blend_shape(&mut self, targets: &[NodeId], base: NodeId) -> Result<String, String> {
        let base_pos = self.scene.node(base).and_then(|n| n.mesh()).map(|m| m.positions.clone()).ok_or("blendShape: the last object must be a polygon mesh (the base).")?;
        if targets.is_empty() {
            return Err("blendShape: select the target mesh(es), then the base mesh.".into());
        }
        let mut new = Vec::new();
        for &t in targets {
            let n = self.scene.node(t).ok_or("blendShape: missing target")?;
            let m = n.mesh().ok_or(format!("blendShape: '{}' is not a polygon mesh", n.name))?;
            if m.positions.len() != base_pos.len() {
                return Err(format!("blendShape: '{}' has {} vertices but the base has {} — targets must share topology.", n.name, m.positions.len(), base_pos.len()));
            }
            let deltas: Vec<Vec3> = m.positions.iter().zip(&base_pos).map(|(a, b)| *a - *b).collect();
            new.push((n.name.clone(), deltas));
        }
        self.checkpoint();
        let n = self.scene.node_mut(base).unwrap();
        let b = n.blend.get_or_insert_with(Default::default);
        let mut names = Vec::new();
        for (name, d) in new {
            match b.index_of(&name) {
                Some(i) => b.targets[i].deltas = d,
                None => b.add_target(name.clone(), d),
            }
            names.push(name);
        }
        Ok(format!("blendShape_{}: {}", n.name, names.join(" ")))
    }

    /// Index of a blend target on a node by name (or numeric index).
    pub fn blend_index(&self, id: NodeId, target: &str) -> Option<usize> {
        self.scene.node(id)?.blend.as_ref()?.index_of(target)
    }

    /// Set a blend-target weight (undoable unless `live` — used while dragging sliders).
    pub fn set_blend_weight(&mut self, id: NodeId, idx: usize, w: f32, live: bool) {
        if !live {
            self.checkpoint();
        }
        if let Some(x) = self.scene.node_mut(id).and_then(|n| n.blend.as_mut()).and_then(|b| b.weights.get_mut(idx)) {
            *x = w;
        }
        if self.auto_key && !live {
            self.key_blend_weight(id, idx, false);
        }
    }

    /// Key a blend-target weight at the current time.
    pub fn key_blend_weight(&mut self, id: NodeId, idx: usize, checkpoint: bool) {
        if checkpoint {
            self.checkpoint();
        }
        let t = self.scene.time;
        if let Some(b) = self.scene.node_mut(id).and_then(|n| n.blend.as_mut()) {
            if let (Some(w), Some(c)) = (b.weights.get(idx).copied(), b.curves.get_mut(idx)) {
                c.get_or_insert_with(Default::default).set_key(t, w);
            }
        }
    }

    pub fn remove_blend_shape(&mut self, ids: &[NodeId]) {
        self.checkpoint();
        for id in ids {
            if let Some(n) = self.scene.node_mut(*id) {
                n.blend = None;
                n.driven.retain(|d| d.blend_target.is_none());
            }
        }
    }

    /// Driven key on a blend-target weight.
    pub fn set_driven_blend_key(&mut self, driver: NodeId, driver_ch: Channel, driven: NodeId, idx: usize, driver_value: Option<f32>, value: Option<f32>) -> Result<String, String> {
        if self.scene.node(driven).and_then(|n| n.blend.as_ref()).is_none_or(|b| idx >= b.targets.len()) {
            return Err("Set Driven Key: unknown blend-shape target.".into());
        }
        self.checkpoint();
        let dv = driver_value.unwrap_or_else(|| self.scene.node(driver).map(|n| n.get(driver_ch)).unwrap_or(0.0));
        let n = self.scene.node_mut(driven).unwrap();
        let b = n.blend.as_mut().unwrap();
        if let Some(v) = value {
            b.weights[idx] = v;
        }
        let v = b.weights[idx];
        let i = match n.driven.iter().position(|d| d.blend_target == Some(idx)) {
            Some(i) => i,
            None => {
                n.driven.push(DrivenKey { channel: Channel::Visibility, blend_target: Some(idx), driver, driver_channel: driver_ch, curve: Curve { tangent: Tangent::Auto, ..Default::default() } });
                n.driven.len() - 1
            }
        };
        n.driven[i].driver = driver;
        n.driven[i].driver_channel = driver_ch;
        n.driven[i].curve.set_key(dv, v);
        Ok(format!("driven key on blend target {idx} at driver value {dv}"))
    }

    /// Create a cluster deformer from the selected vertices (component mode) or whole selected
    /// meshes. `falloff` > 0 adds a smooth falloff to unselected vertices within that distance.
    pub fn create_cluster(&mut self, name: Option<&str>, falloff: f32) -> Result<NodeId, String> {
        let mut sets: Vec<(NodeId, Vec<u32>)> = if self.comp_mode != crate::app::CompMode::Object && !self.comp_sel.is_empty() {
            self.comp_vertices().into_iter().collect()
        } else {
            self.sel.iter().filter_map(|&id| self.scene.node(id).and_then(|n| n.mesh()).map(|m| (id, (0..m.positions.len() as u32).collect()))).collect()
        };
        sets.retain(|(_, v)| !v.is_empty());
        if sets.is_empty() {
            return Err("Cluster: select vertices (or meshes) to deform.".into());
        }
        // Centroid in world space (deformed positions if any).
        let world_of = |app: &App, id: NodeId| -> Vec<Vec3> {
            rig::deformed_positions(&app.scene, id).unwrap_or_else(|| {
                let m = app.scene.world_matrix(id);
                app.scene.node(id).and_then(|n| n.mesh()).map(|me| me.positions.iter().map(|p| m.transform_point3(*p)).collect()).unwrap_or_default()
            })
        };
        let mut sum = Vec3::ZERO;
        let mut cnt = 0.0f32;
        let worlds: Vec<Vec<Vec3>> = sets.iter().map(|(id, _)| world_of(self, *id)).collect();
        for ((_, vs), w) in sets.iter().zip(&worlds) {
            for v in vs {
                if let Some(p) = w.get(*v as usize) {
                    sum += *p;
                    cnt += 1.0;
                }
            }
        }
        let centroid = sum / cnt.max(1.0);
        self.checkpoint();
        let h = self.scene.add_node("cluster1Handle", Shape::ClusterHandle, None);
        self.apply_name(h, name);
        rig::set_world_position(&mut self.scene, h, centroid);
        if let Some(n) = self.scene.node_mut(h) {
            n.color = [0.9, 0.9, 0.45];
        }
        let bind_inv = self.scene.world_matrix(h).inverse();
        for ((id, vs), w) in sets.into_iter().zip(worlds) {
            let mut weights: Vec<(u32, f32)> = vs.iter().map(|v| (*v, 1.0)).collect();
            if falloff > 1e-5 {
                let chosen: std::collections::HashSet<u32> = vs.iter().copied().collect();
                let sel_pts: Vec<Vec3> = vs.iter().filter_map(|v| w.get(*v as usize).copied()).collect();
                let grid = Grid::new(sel_pts);
                for (i, p) in w.iter().enumerate() {
                    if chosen.contains(&(i as u32)) {
                        continue;
                    }
                    if let Some((_, d)) = grid.nearest(*p) {
                        if d < falloff {
                            let t = 1.0 - d / falloff;
                            weights.push((i as u32, t * t * (3.0 - 2.0 * t)));
                        }
                    }
                }
            }
            if let Some(n) = self.scene.node_mut(id) {
                n.clusters.push(rig::Cluster { handle: h, bind_inv, weights });
            }
        }
        self.comp_mode = crate::app::CompMode::Object;
        self.comp_sel.clear();
        self.sel = vec![h];
        Ok(h)
    }

    /// Duplicate a joint hierarchy mirrored across `axis` (0=YZ plane for x …), renaming with
    /// search → replace. Returns the new root.
    pub fn mirror_joints(&mut self, root: NodeId, axis: usize, search: &str, replace: &str) -> Result<NodeId, String> {
        if !self.is_joint(root) {
            return Err("Mirror Joint: select a joint.".into());
        }
        let joints = rig::joint_hierarchy(&self.scene, &[root]);
        self.checkpoint();
        let parent = self.scene.node(root).and_then(|n| n.parent);
        let mut map: std::collections::HashMap<NodeId, NodeId> = Default::default();
        for &j in &joints {
            let (name, radius, color, p_old) = {
                let n = self.scene.node(j).unwrap();
                let r = match n.shape { Shape::Joint { radius } => radius, _ => 0.5 };
                (n.name.clone(), r, n.color, n.parent)
            };
            let par = if j == root { parent } else { p_old.and_then(|p| map.get(&p).copied()) };
            let new_name = if !search.is_empty() && name.contains(search) { name.replace(search, replace) } else { format!("{name}_mirror") };
            let id = self.scene.add_node("joint", Shape::Joint { radius }, par);
            self.apply_name(id, Some(&new_name));
            let wp = mirror_point(rig::world_pos(&self.scene, j), axis);
            rig::set_world_position(&mut self.scene, id, wp);
            if let Some(n) = self.scene.node_mut(id) {
                n.color = color;
            }
            map.insert(j, id);
        }
        let new_root = map[&root];
        let mut all = rig::joint_hierarchy(&self.scene, &[new_root]);
        all.retain(|j| map.values().any(|v| v == j));
        rig::orient_joints(&mut self.scene, &all);
        self.sel = vec![new_root];
        Ok(new_root)
    }

    /// Mirror skin weights across `axis`: vertices on the destination side copy the weights
    /// of their mirror vertex, with influences swapped to their mirror joints.
    /// `positive_to_negative`: copy from +axis to −axis (Maya's default direction).
    pub fn mirror_skin_weights(&mut self, mesh: NodeId, axis: usize, positive_to_negative: bool) -> Result<usize, String> {
        let n = self.scene.node(mesh).ok_or("Mirror Skin Weights: select a skinned mesh.")?;
        let skin = n.skin.clone().ok_or("Mirror Skin Weights: the mesh has no skinCluster.")?;
        let m = n.mesh().ok_or("Mirror Skin Weights: not a mesh")?;
        let bind_pts: Vec<Vec3> = m.positions.iter().map(|p| skin.bind_shape.transform_point3(*p)).collect();
        let joint_bind: Vec<Vec3> = skin.inv_bind.iter().map(|m| m.inverse().transform_point3(Vec3::ZERO)).collect();
        let jgrid = Grid::new(joint_bind.clone());
        let jmap: Vec<u32> = joint_bind.iter().map(|p| jgrid.nearest(mirror_point(*p, axis)).map(|x| x.0 as u32).unwrap_or(0)).collect();
        let ext = bind_pts.iter().fold(0.0f32, |a, p| a.max(p.abs().max_element()));
        let tol = (ext * 1e-3).max(1e-4);
        let grid = Grid::new(bind_pts.clone());
        let mut out = skin.weights.clone();
        let mut count = 0;
        for (v, p) in bind_pts.iter().enumerate() {
            let side = p[axis];
            let dest = if positive_to_negative { side < -tol } else { side > tol };
            if !dest {
                continue;
            }
            if let Some((src, _)) = grid.nearest(mirror_point(*p, axis)) {
                let mut w: Vec<(u32, f32)> = Vec::new();
                for &(i, x) in &skin.weights[src] {
                    let mi = jmap[i as usize];
                    match w.iter_mut().find(|e| e.0 == mi) {
                        Some(e) => e.1 += x,
                        None => w.push((mi, x)),
                    }
                }
                out[v] = w;
                count += 1;
            }
        }
        self.checkpoint();
        if let Some(s) = self.scene.node_mut(mesh).and_then(|n| n.skin.as_mut()) {
            s.weights = out;
        }
        Ok(count)
    }

    /// Toggle IK stretch / set twist on a handle. Turning stretch on captures rest lengths.
    pub fn set_ik_options(&mut self, handle: NodeId, stretch: Option<bool>, twist: Option<f32>) -> Result<(), String> {
        let Some(Shape::IkHandle { start, end, rest, .. }) = self.scene.node(handle).map(|n| n.shape.clone()) else {
            return Err("IK: select an IK handle.".into());
        };
        self.checkpoint();
        let chain = rig::ik_chain(&self.scene, start, end).unwrap_or_default();
        let new_rest: Vec<(NodeId, Vec3)> = match stretch {
            Some(true) if rest.is_empty() => chain.iter().skip(1).filter_map(|j| self.scene.node(*j).map(|n| (*j, n.translate))).collect(),
            Some(false) => {
                for (j, t) in &rest {
                    if let Some(n) = self.scene.node_mut(*j) {
                        n.translate = *t;
                    }
                }
                Vec::new()
            }
            _ => rest,
        };
        if let Some(Shape::IkHandle { stretch: s, twist: tw, rest: r, .. }) = self.scene.node_mut(handle).map(|n| &mut n.shape) {
            if let Some(v) = stretch {
                *s = v;
            }
            if let Some(v) = twist {
                *tw = v;
            }
            *r = new_rest;
        }
        Ok(())
    }

    fn qr_joint(&mut self, name: &str, p: Vec3, parent: Option<NodeId>, radius: f32) -> NodeId {
        let id = self.scene.add_node("joint", Shape::Joint { radius }, parent);
        self.apply_name(id, Some(name));
        rig::set_world_position(&mut self.scene, id, p);
        id
    }

    fn qr_ctrl(&mut self, name: &str, shape: &str, size: f32, normal: Option<Vec3>, at: Vec3, parent: Option<NodeId>, color: [f32; 3]) -> NodeId {
        let (mut pts, closed) = rig::control_shape(shape, size).unwrap_or((vec![], true));
        if let Some(nr) = normal.filter(|_| shape == "circle") {
            pts = rig::circle_points(size, nr, 16);
        }
        let id = self.scene.add_node(name, Shape::Curve { points: pts, closed }, parent);
        self.apply_name(id, Some(name));
        rig::set_world_position(&mut self.scene, id, at);
        if let Some(n) = self.scene.node_mut(id) {
            n.color = color;
        }
        id
    }

    /// One-click biped auto-rig (Maya Quick Rig "one-click"): builds a proportional skeleton
    /// fitted to the mesh's bounding box (character facing +Z, Y up, centred on X), mirrors the
    /// left limbs, adds arm/leg IK with pole vectors, FK/IK controls and binds the mesh.
    pub fn quick_rig(&mut self, mesh: Option<NodeId>) -> Result<String, String> {
        let (lo, hi) = match mesh {
            Some(m) => {
                let pts = rig::deformed_positions(&self.scene, m).or_else(|| {
                    let w = self.scene.world_matrix(m);
                    self.scene.node(m).and_then(|n| n.mesh()).map(|me| me.positions.iter().map(|p| w.transform_point3(*p)).collect())
                });
                let pts = pts.filter(|p: &Vec<Vec3>| !p.is_empty()).ok_or("Quick Rig: select a polygon mesh (a character standing on Y, facing +Z).")?;
                if self.scene.node(m).is_some_and(|n| n.skin.is_some()) {
                    return Err("Quick Rig: the mesh is already skinned — unbind it first.".into());
                }
                pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)))
            }
            None => (Vec3::new(-0.9, 0.0, -0.15), Vec3::new(0.9, 1.8, 0.15)),
        };
        let h = (hi.y - lo.y).max(1e-3);
        let cx = (lo.x + hi.x) * 0.5;
        let cz = (lo.z + hi.z) * 0.5;
        let y0 = lo.y;
        let half_w = ((hi.x - lo.x) * 0.5).max(0.2 * h);
        // T-pose characters are about as wide as tall; otherwise assume arms out to 0.45 H.
        let reach = if half_w > 0.3 * h { half_w.min(0.55 * h) } else { 0.45 * h };
        let at = |x: f32, y: f32, z: f32| Vec3::new(cx + x * h, y0 + y * h, cz + z * h);
        let r = 0.025 * h;
        if self.scene.name_exists("hips") {
            return Err("Quick Rig: a rig already exists (a joint named 'hips'). Delete it first.".into());
        }
        self.begin_batch();
        self.checkpoint();
        let res: Result<String, String> = (|| {
            let hips = self.qr_joint("hips", at(0.0, 0.53, 0.0), None, r * 1.4);
            let s1 = self.qr_joint("spine1", at(0.0, 0.62, -0.01), Some(hips), r);
            let s2 = self.qr_joint("spine2", at(0.0, 0.71, -0.01), Some(s1), r);
            let chest = self.qr_joint("chest", at(0.0, 0.79, 0.0), Some(s2), r);
            let neck = self.qr_joint("neck", at(0.0, 0.86, 0.0), Some(chest), r);
            let head = self.qr_joint("head", at(0.0, 0.92, 0.01), Some(neck), r);
            let _head_end = self.qr_joint("head_end", at(0.0, 1.0, 0.01), Some(head), r * 0.7);
            let sh_y = 0.815;
            let rx = reach / h;
            let clav = self.qr_joint("L_clavicle", at(0.03, sh_y, 0.01), Some(chest), r);
            let shoulder = self.qr_joint("L_shoulder", at(0.1, sh_y, 0.0), Some(clav), r);
            let elbow = self.qr_joint("L_elbow", at(0.1 + (rx - 0.1) * 0.47, sh_y, -0.015), Some(shoulder), r);
            let wrist = self.qr_joint("L_wrist", at(0.1 + (rx - 0.1) * 0.86, sh_y, 0.0), Some(elbow), r);
            let _hand = self.qr_joint("L_hand_end", at(rx, sh_y, 0.0), Some(wrist), r * 0.7);
            let hip = self.qr_joint("L_hip", at(0.09, 0.5, 0.0), Some(hips), r);
            let knee = self.qr_joint("L_knee", at(0.09, 0.27, 0.02), Some(hip), r);
            let ankle = self.qr_joint("L_ankle", at(0.09, 0.05, -0.01), Some(knee), r);
            let _toe = self.qr_joint("L_toe", at(0.09, 0.01, 0.08), Some(ankle), r * 0.7);
            let hj = rig::joint_hierarchy(&self.scene, &[hips]);
            rig::orient_joints(&mut self.scene, &hj);
            // Mirror limbs to the right side (mirror across YZ through the character centre).
            for root in [clav, hip] {
                self.mirror_joints_about(root, cx, "L_", "R_")?;
            }
            let root_ctrl = self.qr_ctrl("root_ctrl", "circle", 0.3 * h, Some(Vec3::Y), at(0.0, 0.0, 0.0), None, [0.95, 0.85, 0.15]);
            let cog = self.qr_ctrl("cog_ctrl", "circle", 0.2 * h, Some(Vec3::Y), at(0.0, 0.53, 0.0), Some(root_ctrl), [0.95, 0.5, 0.1]);
            let chest_ctrl = self.qr_ctrl("chest_ctrl", "circle", 0.15 * h, Some(Vec3::Y), at(0.0, 0.79, 0.0), Some(cog), [0.95, 0.85, 0.15]);
            let head_ctrl = self.qr_ctrl("head_ctrl", "circle", 0.09 * h, Some(Vec3::Y), at(0.0, 0.92, 0.01), Some(chest_ctrl), [0.95, 0.85, 0.15]);
            let add_c = |app: &mut App, kind: ConstraintKind, d: NodeId, t: NodeId| {
                let c = Constraint::new(&app.scene, kind, vec![d], t, true);
                if let Some(n) = app.scene.node_mut(t) {
                    n.constraints.push(c);
                }
            };
            add_c(self, ConstraintKind::Parent, cog, hips);
            add_c(self, ConstraintKind::Orient, chest_ctrl, chest);
            add_c(self, ConstraintKind::Orient, head_ctrl, head);
            for side in ["L", "R"] {
                let sign = if side == "L" { 1.0 } else { -1.0 };
                let blue_red = if side == "L" { [0.2, 0.45, 1.0] } else { [0.95, 0.2, 0.15] };
                let f = |app: &App, n: &str| app.scene.find_by_name(&format!("{side}_{n}")).unwrap();
                let (sh, wr, hp, an) = (f(self, "shoulder"), f(self, "wrist"), f(self, "hip"), f(self, "ankle"));
                let arm_ik = self.create_ik(sh, wr, Some(&format!("{side}_arm_ik")))?;
                let leg_ik = self.create_ik(hp, an, Some(&format!("{side}_leg_ik")))?;
                let elbow_p = rig::world_pos(&self.scene, f(self, "elbow"));
                let knee_p = rig::world_pos(&self.scene, f(self, "knee"));
                let ep = self.scene.add_node(&format!("{side}_elbow_pole"), Shape::Locator, Some(cog));
                self.apply_name(ep, Some(&format!("{side}_elbow_pole")));
                rig::set_world_position(&mut self.scene, ep, elbow_p - Vec3::Z * 0.25 * h);
                let kp = self.scene.add_node(&format!("{side}_knee_pole"), Shape::Locator, Some(root_ctrl));
                self.apply_name(kp, Some(&format!("{side}_knee_pole")));
                rig::set_world_position(&mut self.scene, kp, knee_p + Vec3::Z * 0.3 * h);
                self.pole_vector(ep, arm_ik)?;
                self.pole_vector(kp, leg_ik)?;
                let wp = rig::world_pos(&self.scene, wr);
                let ap = rig::world_pos(&self.scene, an);
                let hand_c = self.qr_ctrl(&format!("{side}_hand_ctrl"), "circle", 0.05 * h, Some(Vec3::X * sign), wp, Some(cog), blue_red);
                let foot_c = self.qr_ctrl(&format!("{side}_foot_ctrl"), "square", 0.06 * h, None, Vec3::new(ap.x, y0, ap.z + 0.02 * h), Some(root_ctrl), blue_red);
                add_c(self, ConstraintKind::Point, hand_c, arm_ik);
                add_c(self, ConstraintKind::Point, foot_c, leg_ik);
                add_c(self, ConstraintKind::Orient, hand_c, wr);
                let clav_j = f(self, "clavicle");
                let cp = rig::world_pos(&self.scene, clav_j);
                let clav_c = self.qr_ctrl(&format!("{side}_clavicle_ctrl"), "circle", 0.04 * h, Some(Vec3::X * sign), cp + Vec3::X * sign * 0.03 * h, Some(chest_ctrl), blue_red);
                add_c(self, ConstraintKind::Orient, clav_c, clav_j);
                for id in [arm_ik, leg_ik] {
                    if let Some(n) = self.scene.node_mut(id) {
                        n.visible = false;
                    }
                }
            }
            let mut msg = format!("Quick Rig: {} joints, 4 IK limbs, controls under root_ctrl", rig::joint_hierarchy(&self.scene, &[hips]).len());
            if let Some(m) = mesh {
                let joints = rig::joint_hierarchy(&self.scene, &[hips]);
                // End joints don't deform.
                let joints: Vec<NodeId> = joints.into_iter().filter(|j| self.scene.node(*j).is_some_and(|n| !n.name.ends_with("_end"))).collect();
                if let Some(skin) = rig::bind_skin(&self.scene, m, &joints, 4, 4.0) {
                    self.scene.node_mut(m).unwrap().skin = Some(skin);
                    msg.push_str("; mesh bound (4 influences)");
                }
            }
            rig::evaluate(&mut self.scene);
            self.sel = vec![root_ctrl];
            Ok(msg)
        })();
        self.end_batch();
        res
    }

    /// Mirror joints across the plane x = `cx` (helper for off-centre characters).
    fn mirror_joints_about(&mut self, root: NodeId, cx: f32, search: &str, replace: &str) -> Result<NodeId, String> {
        let new_root = self.mirror_joints(root, 0, search, replace)?;
        if cx.abs() > 1e-6 {
            // x' = -x mirrors about 0; mirroring about cx is x' = 2cx - x: shift the new root
            // (its children follow).
            let p = rig::world_pos(&self.scene, new_root) + Vec3::X * 2.0 * cx;
            rig::set_world_position(&mut self.scene, new_root, p);
        }
        Ok(new_root)
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;
    use glam::Vec3;

    #[test]
    fn mel_arm_rig() {
        let mut app = App::new();
        for m in [
            "select -cl",
            "joint -p 0 5 0 -n shoulder",
            "joint -p 2 5 -0.3 -n elbow",
            "joint -p 4 5 0 -n wrist",
            "joint -e -oj xyz shoulder",
            "ikHandle -sj shoulder -ee wrist -n arm_ik",
            "spaceLocator -n elbow_pole",
            "move 2 5 -3",
            "poleVectorConstraint elbow_pole arm_ik",
            "circle -r 0.5 -nr 1 0 0 -n wrist_ctrl",
            "move 4 5 0",
            "pointConstraint wrist_ctrl arm_ik",
            "polyCylinder -r 0.3 -h 4 -sa 8 -n arm_mesh",
            "rotate 0 0 -90",
            "move 2 5 0",
            "skinCluster -mi 3 shoulder arm_mesh",
            "move 3 6.5 0 wrist_ctrl",
        ] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        crate::rig::evaluate(&mut app.scene);
        let s = &app.scene;
        let wrist = crate::rig::world_pos(s, s.find_by_name("wrist").unwrap());
        assert!((wrist - Vec3::new(3.0, 6.5, 0.0)).length() < 0.05, "wrist follows the control via IK: {wrist}");
        let elbow = crate::rig::world_pos(s, s.find_by_name("elbow").unwrap());
        assert!(elbow.z < -0.1, "elbow bends toward the pole: {elbow}");
        let mesh = s.find_by_name("arm_mesh").unwrap();
        assert!(s.node(mesh).unwrap().skin.is_some());
        assert!(crate::rig::deformed_positions(s, mesh).is_some());
        execute(&mut app, "gotoBindPose").unwrap();
        // Set driven key: ctrl.ty drives door.ry
        for m in ["spaceLocator -n ctrl", "spaceLocator -n door", "setDrivenKeyframe -cd ctrl.ty -dv 0 -at ry -v 0 door", "setDrivenKeyframe -cd ctrl.ty -dv 10 -at ry -v 90 door", "move 0 5 0 ctrl"] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        crate::rig::evaluate(&mut app.scene);
        let door = app.scene.find_by_name("door").unwrap();
        assert!((app.scene.node(door).unwrap().rotate.y - 45.0).abs() < 1.0);
        // Unbind, constraint removal, control presets
        execute(&mut app, "skinCluster -e -ub arm_mesh; controlShape -s arrow -r 2 -n move_ctrl").unwrap();
        assert!(app.scene.node(mesh).unwrap().skin.is_none());
        assert!(execute(&mut app, "ikHandle -sj wrist -ee shoulder").is_err());
    }

    #[test]
    fn mel_phase3_rigging() {
        use crate::rig;
        let mut app = App::new();
        app.scene.nodes.clear();
        // Blend shape: base + a target whose vertices are lifted by 1.
        execute(&mut app, "polyCube -n base; polyCube -n smile").unwrap();
        let t = app.scene.find_by_name("smile").unwrap();
        for p in &mut app.scene.node_mut(t).unwrap().mesh_mut(true).unwrap().positions {
            p.y += 1.0;
        }
        execute(&mut app, "blendShape smile base").unwrap();
        let base = app.scene.find_by_name("base").unwrap();
        assert!(rig::deformed_positions(&app.scene, base).is_none(), "zero weight → no deformation");
        execute(&mut app, "setAttr base.smile 0.5").unwrap();
        let d = rig::deformed_positions(&app.scene, base).unwrap();
        let orig = app.scene.node(base).unwrap().mesh().unwrap().positions[0];
        assert!((d[0].y - orig.y - 0.5).abs() < 1e-4);
        assert_eq!(execute(&mut app, "getAttr base.smile").unwrap(), "0.5");
        // Keyed blend weight.
        execute(&mut app, "setKeyframe -t 1 -at smile -v 0 base; setKeyframe -t 11 -at smile -v 1 base; currentTime 6").unwrap();
        app.scene.evaluate();
        let w = app.scene.node(base).unwrap().blend.as_ref().unwrap().weights[0];
        assert!((w - 0.5).abs() < 0.1, "keyed weight mid-way: {w}");
        // Driven blend target.
        execute(&mut app, "spaceLocator -n jaw; setDrivenKeyframe -cd jaw.ty -dv 0 -at smile -v 0 base; setDrivenKeyframe -cd jaw.ty -dv 2 -at smile -v 1 base; move 0 1 0 jaw").unwrap();
        rig::evaluate(&mut app.scene);
        let w = app.scene.node(base).unwrap().blend.as_ref().unwrap().weights[0];
        assert!((w - 0.5).abs() < 0.05, "driven weight: {w}");
        assert!(execute(&mut app, "polySphere -n ball; blendShape ball base").is_err(), "topology mismatch is rejected");

        // Cluster on a vertex subset.
        let mut app = App::new();
        execute(&mut app, "polyPlane -sx 2 -sy 2 -n sheet").unwrap();
        let sheet = app.scene.find_by_name("sheet").unwrap();
        app.set_comp_mode(crate::app::CompMode::Vertex);
        app.comp_sel.insert((sheet, 4));
        let h = execute(&mut app, "cluster -n bump").unwrap();
        assert_eq!(h, "bump");
        execute(&mut app, "move 0 2 0 bump").unwrap();
        let d = rig::deformed_positions(&app.scene, sheet).unwrap();
        assert!((d[4].y - 2.0).abs() < 1e-4 && d[0].y.abs() < 1e-4);

        // Mirror joints + IK stretch.
        let mut app = App::new();
        for m in ["select -cl", "joint -p 1 5 0 -n L_shoulder", "joint -p 3 5 -0.3 -n L_elbow", "joint -p 5 5 0 -n L_wrist", "joint -e -oj xyz L_shoulder", "mirrorJoint -myz -sr L_ R_ L_shoulder"] {
            execute(&mut app, m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        let rw = app.scene.find_by_name("R_wrist").expect("mirrored wrist");
        assert!((rig::world_pos(&app.scene, rw) - glam::Vec3::new(-5.0, 5.0, 0.0)).length() < 1e-3);
        execute(&mut app, "ikHandle -sj R_shoulder -ee R_wrist -n r_ik; ikHandle -e -stretch on r_ik; move -9 5 0 r_ik").unwrap();
        rig::evaluate(&mut app.scene);
        let p = rig::world_pos(&app.scene, rw);
        assert!((p - glam::Vec3::new(-9.0, 5.0, 0.0)).length() < 0.05, "stretchy IK reaches: {p}");
        execute(&mut app, "ikHandle -e -stretch off r_ik").unwrap();
        rig::evaluate(&mut app.scene);
        let p = rig::world_pos(&app.scene, rw);
        assert!((p - rig::world_pos(&app.scene, app.scene.find_by_name("R_shoulder").unwrap())).length() < 4.1);
    }

    #[test]
    fn quick_rig_and_weight_mirror() {
        use crate::rig;
        let mut app = App::new();
        app.scene.nodes.clear();
        // A rough T-pose body: torso + arms + legs as one mesh.
        execute(&mut app, "polyCube -w 0.5 -h 0.8 -d 0.3 -sy 4 -n torso; move 0 1.2 0; polyCube -w 1.6 -h 0.12 -d 0.12 -sx 12 -n arms; move 0 1.47 0; polyCube -w 0.4 -h 0.9 -d 0.15 -sy 6 -sx 2 -n legs; move 0 0.45 0; polyUnite torso arms legs").unwrap();
        let body = app.sel[0];
        app.scene.node_mut(body).unwrap().name = "body".into();
        let msg = execute(&mut app, "quickRig body").unwrap();
        assert!(msg.contains("bound"), "{msg}");
        for n in ["hips", "L_shoulder", "R_shoulder", "R_knee", "L_arm_ik", "R_leg_ik", "root_ctrl", "R_hand_ctrl"] {
            assert!(app.scene.find_by_name(n).is_some(), "missing {n}");
        }
        let lw = rig::world_pos(&app.scene, app.scene.find_by_name("L_wrist").unwrap());
        let rw = rig::world_pos(&app.scene, app.scene.find_by_name("R_wrist").unwrap());
        assert!((lw.x + rw.x).abs() < 1e-3 && lw.x > 0.3, "symmetric wrists {lw} {rw}");
        // Moving a hand control drives the arm through IK.
        execute(&mut app, "move -r 0 0.2 0 L_hand_ctrl").unwrap();
        rig::evaluate(&mut app.scene);
        let lw2 = rig::world_pos(&app.scene, app.scene.find_by_name("L_wrist").unwrap());
        assert!((lw2.y - lw.y - 0.2).abs() < 0.05, "wrist follows hand ctrl {lw} → {lw2}");
        // Undo removes the whole rig in one step.
        execute(&mut app, "undo; undo").unwrap();
        assert!(app.scene.find_by_name("hips").is_none());
        // Mirror skin weights: right side copies left with swapped influences.
        execute(&mut app, "quickRig body").unwrap();
        let skin0 = app.scene.node(body).unwrap().skin.clone().unwrap();
        let li = skin0.joints.iter().position(|j| app.scene.node(*j).unwrap().name == "L_elbow").unwrap();
        let ri = skin0.joints.iter().position(|j| app.scene.node(*j).unwrap().name == "R_elbow").unwrap();
        let n = execute(&mut app, "copySkinWeights -mm YZ body").unwrap();
        assert!(n.contains("vertices"));
        let skin = app.scene.node(body).unwrap().skin.clone().unwrap();
        let mesh = app.scene.node(body).unwrap().mesh().unwrap();
        let wpos: Vec<glam::Vec3> = mesh.positions.iter().map(|p| skin.bind_shape.transform_point3(*p)).collect();
        for (v, p) in wpos.iter().enumerate() {
            if p.x > 0.5 {
                let m = wpos.iter().position(|q| (*q - glam::Vec3::new(-p.x, p.y, p.z)).length() < 1e-3);
                if let Some(m) = m {
                    assert!((skin.weight(v, li) - skin.weight(m, ri)).abs() < 1e-4);
                }
            }
        }
    }
}
