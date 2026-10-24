//! Character rigging: smooth skinning, IK (analytic two-bone + CCD), constraints, set driven
//! keys, joint orientation and NURBS-style control curves.
//!
//! Evaluation order every frame (after animation curves): driven keys → constraints → IK.
//! Skinned meshes are deformed at draw/pick time from the current joint matrices.

use crate::scene::{Channel, Curve, NodeId, Scene, Shape};
use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

const DEG: f32 = 180.0 / std::f32::consts::PI;
const RAD: f32 = std::f32::consts::PI / 180.0;

pub fn euler_quat(deg: Vec3) -> Quat {
    let r = deg * RAD;
    Quat::from_euler(EulerRot::ZYX, r.z, r.y, r.x)
}

pub fn quat_euler(q: Quat) -> Vec3 {
    let (z, y, x) = q.to_euler(EulerRot::ZYX);
    Vec3::new(x, y, z) * DEG
}

// --------------------------------------------------------------------------------- skin

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Skin {
    pub joints: Vec<NodeId>,
    /// Inverse world matrix of each influence at bind time.
    pub inv_bind: Vec<Mat4>,
    /// Mesh world matrix at bind time (Maya's bindPreMatrix / geomMatrix).
    pub bind_shape: Mat4,
    /// Per vertex: (influence index, weight), normalised.
    pub weights: Vec<Vec<(u32, f32)>>,
    pub max_influences: usize,
    /// (joint, translate, rotate, joint orient) at bind — for Go To Bind Pose.
    pub bind_pose: Vec<(NodeId, Vec3, Vec3, Vec3)>,
}

impl Skin {
    pub fn weight(&self, v: usize, inf: usize) -> f32 {
        self.weights.get(v).and_then(|w| w.iter().find(|(i, _)| *i as usize == inf)).map(|x| x.1).unwrap_or(0.0)
    }

    /// Set one influence weight and renormalise the others proportionally (Maya's
    /// "normalize weights: interactive").
    pub fn set_weight(&mut self, v: usize, inf: usize, w: f32) {
        let w = w.clamp(0.0, 1.0);
        let Some(list) = self.weights.get_mut(v) else { return };
        let others: f32 = list.iter().filter(|(i, _)| *i as usize != inf).map(|x| x.1).sum();
        let remaining = 1.0 - w;
        let mut out: Vec<(u32, f32)> = Vec::new();
        if others > 1e-6 {
            for (i, x) in list.iter() {
                if *i as usize != inf {
                    out.push((*i, x * remaining / others));
                }
            }
        } else if remaining > 1e-6 {
            // Nothing else influences this vertex: give the remainder to the next-closest existing weight
            // (there is none) — keep it on this influence.
            out.clear();
            out.push((inf as u32, 1.0));
            *list = out;
            return;
        }
        if w > 1e-5 {
            out.push((inf as u32, w));
        }
        out.retain(|x| x.1 > 1e-5);
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        // Respect the max influence count, renormalising what is kept.
        out.truncate(self.max_influences.max(1));
        let s: f32 = out.iter().map(|x| x.1).sum();
        if s > 1e-6 {
            for x in &mut out {
                x.1 /= s;
            }
        }
        *list = out;
    }
}

/// Distance from p to segment ab.
fn seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

pub fn world_pos(scene: &Scene, id: NodeId) -> Vec3 {
    scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

pub fn is_joint(scene: &Scene, id: NodeId) -> bool {
    scene.node(id).is_some_and(|n| matches!(n.shape, Shape::Joint { .. }))
}

pub fn child_joints(scene: &Scene, id: NodeId) -> Vec<NodeId> {
    scene.children(Some(id)).into_iter().filter(|c| is_joint(scene, *c)).collect()
}

/// All joints in the hierarchies below the given ids (inclusive), in hierarchy order.
pub fn joint_hierarchy(scene: &Scene, roots: &[NodeId]) -> Vec<NodeId> {
    let mut out = Vec::new();
    for &r in roots {
        for id in scene.subtree(r) {
            if is_joint(scene, id) && !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// Bind a mesh to joints with closest-distance automatic weights (Maya "Bind Skin",
/// bind method "Closest distance", dropoff rate 4).
pub fn bind_skin(scene: &Scene, mesh_id: NodeId, joints: &[NodeId], max_influences: usize, dropoff: f32) -> Option<Skin> {
    let mesh = scene.node(mesh_id)?.mesh()?;
    if joints.is_empty() {
        return None;
    }
    let bind_shape = scene.world_matrix(mesh_id);
    let inv_bind: Vec<Mat4> = joints.iter().map(|j| scene.world_matrix(*j).inverse()).collect();
    // Bone segments owned by each joint: joint -> each child joint (or the joint point for leaves).
    let segs: Vec<Vec<(Vec3, Vec3)>> = joints
        .iter()
        .map(|&j| {
            let p = world_pos(scene, j);
            let kids = child_joints(scene, j);
            if kids.is_empty() { vec![(p, p)] } else { kids.iter().map(|k| (p, world_pos(scene, *k))).collect() }
        })
        .collect();
    let weights = mesh
        .positions
        .iter()
        .map(|lp| {
            let p = bind_shape.transform_point3(*lp);
            let mut cand: Vec<(u32, f32)> = segs
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let d = s.iter().map(|(a, b)| seg_dist(p, *a, *b)).fold(f32::MAX, f32::min);
                    (i as u32, 1.0 / (d.powf(dropoff) + 1e-6))
                })
                .collect();
            cand.sort_by(|a, b| b.1.total_cmp(&a.1));
            cand.truncate(max_influences.max(1));
            let s: f32 = cand.iter().map(|c| c.1).sum();
            cand.iter_mut().for_each(|c| c.1 /= s);
            cand.retain(|c| c.1 > 1e-4);
            let s: f32 = cand.iter().map(|c| c.1).sum();
            cand.iter_mut().for_each(|c| c.1 /= s);
            cand
        })
        .collect();
    let bind_pose = joints.iter().filter_map(|j| scene.node(*j).map(|n| (*j, n.translate, n.rotate, n.joint_orient))).collect();
    Some(Skin { joints: joints.to_vec(), inv_bind, bind_shape, weights, max_influences, bind_pose })
}

/// Per-influence skinning matrices (joint world × inverse bind).
pub fn skin_matrices(scene: &Scene, skin: &Skin) -> Vec<Mat4> {
    skin.joints
        .iter()
        .zip(&skin.inv_bind)
        .map(|(j, ib)| if scene.node(*j).is_some() { scene.world_matrix(*j) * *ib } else { Mat4::IDENTITY })
        .collect()
}

/// Blended matrix for vertex `v` mapping its *object-space bind position* to world space.
pub fn vertex_matrix(skin: &Skin, mats: &[Mat4], v: usize) -> Mat4 {
    let Some(ws) = skin.weights.get(v) else { return skin.bind_shape };
    if ws.is_empty() {
        return skin.bind_shape;
    }
    let mut m = Mat4::ZERO;
    for &(i, w) in ws {
        m += mats.get(i as usize).copied().unwrap_or(Mat4::IDENTITY) * w;
    }
    m * skin.bind_shape
}

/// True when the node has an active deformer (blend shape, skin or cluster).
pub fn has_deformers(scene: &Scene, id: NodeId) -> bool {
    let Some(n) = scene.node(id) else { return false };
    let Some(mesh) = n.mesh() else { return false };
    let nv = mesh.positions.len();
    n.skin.as_ref().is_some_and(|s| s.weights.len() == nv)
        || n.blend.as_ref().is_some_and(|b| b.targets.iter().any(|t| t.deltas.len() == nv) && b.weights.iter().any(|w| w.abs() > 1e-6))
        || !n.clusters.is_empty()
        || !n.lattices.is_empty()
        || n.wrap.as_ref().is_some_and(|w| w.binds.len() == nv)
}

/// Deformed world positions (deformer order: blend shape → skin → clusters), or None if the
/// node has no active deformers.
pub fn deformed_positions(scene: &Scene, id: NodeId) -> Option<Vec<Vec3>> {
    deformed_positions_depth(scene, id, 0)
}

fn deformed_positions_depth(scene: &Scene, id: NodeId, depth: u32) -> Option<Vec<Vec3>> {
    if !has_deformers(scene, id) || depth > 4 {
        return None;
    }
    let n = scene.node(id)?;
    let mesh = n.mesh()?;
    let nv = mesh.positions.len();
    let mut pts: Vec<Vec3> = mesh.positions.clone();
    if let Some(b) = &n.blend {
        for (t, w) in b.targets.iter().zip(&b.weights) {
            if t.deltas.len() == nv && w.abs() > 1e-6 {
                for (p, d) in pts.iter_mut().zip(&t.deltas) {
                    *p += *d * *w;
                }
            }
        }
    }
    let world: Vec<Vec3> = match n.skin.as_ref().filter(|s| s.weights.len() == nv) {
        Some(skin) => {
            let mats = skin_matrices(scene, skin);
            pts.iter().enumerate().map(|(v, p)| vertex_matrix(skin, &mats, v).transform_point3(*p)).collect()
        }
        None => {
            let w = scene.world_matrix(id);
            pts.iter().map(|p| w.transform_point3(*p)).collect()
        }
    };
    let mut world = world;
    for c in &n.clusters {
        if scene.node(c.handle).is_none() {
            continue;
        }
        let m = scene.world_matrix(c.handle) * c.bind_inv;
        for &(v, w) in &c.weights {
            if let Some(p) = world.get_mut(v as usize) {
                *p = p.lerp(m.transform_point3(*p), w);
            }
        }
    }
    crate::deform::apply_lattices(scene, &n.lattices, &mut world);
    if let Some(wb) = n.wrap.as_ref().filter(|w| w.binds.len() == nv) {
        if let Some(dm) = scene.node(wb.driver).and_then(|d| d.mesh()) {
            let dw = deformed_positions_depth(scene, wb.driver, depth + 1).unwrap_or_else(|| {
                let m = scene.world_matrix(wb.driver);
                dm.positions.iter().map(|p| m.transform_point3(*p)).collect()
            });
            crate::deform::wrap_apply(&dw, &wb.binds, &mut world);
        }
    }
    Some(world)
}

// ------------------------------------------------------------------------ blend shapes

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BlendTarget {
    pub name: String,
    /// Per-vertex offsets from the base mesh (object space).
    pub deltas: Vec<Vec3>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BlendShape {
    pub targets: Vec<BlendTarget>,
    pub weights: Vec<f32>,
    /// Optional animation curve per target weight.
    #[serde(default)]
    pub curves: Vec<Option<Curve>>,
}

impl BlendShape {
    pub fn add_target(&mut self, name: String, deltas: Vec<Vec3>) {
        self.targets.push(BlendTarget { name, deltas });
        self.weights.push(0.0);
        self.curves.push(None);
    }
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.targets.iter().position(|t| t.name == name).or_else(|| name.parse().ok().filter(|i| *i < self.targets.len()))
    }
}

// ----------------------------------------------------------------------------- clusters

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Cluster {
    pub handle: NodeId,
    /// Inverse of the handle's world matrix at creation.
    pub bind_inv: Mat4,
    pub weights: Vec<(u32, f32)>,
}

pub fn goto_bind_pose(scene: &mut Scene, skin: &Skin) {
    for (j, t, r, o) in &skin.bind_pose {
        if let Some(n) = scene.node_mut(*j) {
            n.translate = *t;
            n.rotate = *r;
            n.joint_orient = *o;
        }
    }
}

// ---------------------------------------------------------------------- world helpers

pub fn world_rotation(scene: &Scene, id: NodeId) -> Quat {
    let (_, r, _) = scene.world_matrix(id).to_scale_rotation_translation();
    r.normalize()
}

fn parent_rotation(scene: &Scene, id: NodeId) -> Quat {
    let (_, r, _) = scene.parent_matrix(id).to_scale_rotation_translation();
    r.normalize()
}

/// Set a node's rotate channels so its world rotation becomes `q` (joint orient respected).
pub fn set_world_rotation(scene: &mut Scene, id: NodeId, q: Quat) {
    let local = parent_rotation(scene, id).inverse() * q;
    if let Some(n) = scene.node_mut(id) {
        let r = euler_quat(n.joint_orient).inverse() * local;
        n.rotate = quat_euler(r.normalize());
    }
}

pub fn set_world_position(scene: &mut Scene, id: NodeId, p: Vec3) {
    let local = scene.parent_matrix(id).inverse().transform_point3(p);
    if let Some(n) = scene.node_mut(id) {
        n.translate = local;
    }
}

fn rotate_world_by(scene: &mut Scene, id: NodeId, q: Quat) {
    let r = world_rotation(scene, id);
    set_world_rotation(scene, id, (q * r).normalize());
}

// --------------------------------------------------------------------------------- IK

/// Joints from `start` down to `end` (inclusive), or None if `end` isn't below `start`.
pub fn ik_chain(scene: &Scene, start: NodeId, end: NodeId) -> Option<Vec<NodeId>> {
    let mut chain = vec![end];
    let mut cur = end;
    while cur != start {
        cur = scene.node(cur)?.parent?;
        chain.push(cur);
        if chain.len() > 64 {
            return None;
        }
    }
    chain.reverse();
    (chain.len() >= 2).then_some(chain)
}

/// Rotate-plane IK (Maya ikRPsolver) for 2-bone chains; CCD for longer chains.
#[allow(dead_code)]
pub fn solve_ik(scene: &mut Scene, start: NodeId, end: NodeId, goal: Vec3, pole: Option<Vec3>) {
    solve_ik_twist(scene, start, end, goal, pole, 0.0);
}

/// Rotate-plane IK with a twist (degrees) of the bend plane around the start → goal axis.
pub fn solve_ik_twist(scene: &mut Scene, start: NodeId, end: NodeId, goal: Vec3, pole: Option<Vec3>, twist: f32) {
    let Some(chain) = ik_chain(scene, start, end) else { return };
    if chain.len() == 3 {
        let (a_id, b_id) = (chain[0], chain[1]);
        let a = world_pos(scene, a_id);
        let b = world_pos(scene, b_id);
        let c = world_pos(scene, end);
        let (l1, l2) = ((b - a).length(), (c - b).length());
        if l1 < 1e-5 || l2 < 1e-5 {
            return;
        }
        let dvec = goal - a;
        let d = dvec.length().clamp((l1 - l2).abs() + 1e-4, l1 + l2 - 1e-4);
        let x = if dvec.length() > 1e-6 { dvec.normalize() } else { (c - a).normalize_or(Vec3::X) };
        let reference = pole.unwrap_or(b);
        let mut y = (reference - a) - x * (reference - a).dot(x);
        if y.length() < 1e-5 {
            // Bend direction undefined (straight chain, no pole): pick any perpendicular.
            y = x.any_orthonormal_vector();
        }
        let y = Quat::from_axis_angle(x, twist * RAD) * y.normalize();
        let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
        let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
        let b_new = a + (x * cos_a + y * sin_a) * l1;
        let c_new = a + x * d;
        let q1 = Quat::from_rotation_arc((b - a) / l1, (b_new - a) / l1);
        rotate_world_by(scene, a_id, q1);
        let b_now = world_pos(scene, b_id);
        let c_now = world_pos(scene, end);
        if (c_now - b_now).length() > 1e-6 && (c_new - b_now).length() > 1e-6 {
            let q2 = Quat::from_rotation_arc((c_now - b_now).normalize(), (c_new - b_now).normalize());
            rotate_world_by(scene, b_id, q2);
        }
    } else {
        for _ in 0..12 {
            for &j in chain[..chain.len() - 1].iter().rev() {
                let jp = world_pos(scene, j);
                let e = world_pos(scene, end);
                let (from, to) = (e - jp, goal - jp);
                if from.length() < 1e-6 || to.length() < 1e-6 {
                    continue;
                }
                rotate_world_by(scene, j, Quat::from_rotation_arc(from.normalize(), to.normalize()));
            }
            if (world_pos(scene, end) - goal).length() < 1e-4 {
                break;
            }
        }
    }
}

// ------------------------------------------------------------------------ constraints

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintKind {
    Parent,
    Point,
    Orient,
    Scale,
    Aim,
}

impl ConstraintKind {
    pub fn name(self) -> &'static str {
        match self {
            ConstraintKind::Parent => "parentConstraint",
            ConstraintKind::Point => "pointConstraint",
            ConstraintKind::Orient => "orientConstraint",
            ConstraintKind::Scale => "scaleConstraint",
            ConstraintKind::Aim => "aimConstraint",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub drivers: Vec<NodeId>,
    /// Per-driver offset (driver world → target world at creation; identity without -mo).
    pub offsets: Vec<Mat4>,
    pub weight: f32,
    pub aim: Vec3,
    pub up: Vec3,
}

impl Constraint {
    pub fn new(scene: &Scene, kind: ConstraintKind, drivers: Vec<NodeId>, target: NodeId, maintain_offset: bool) -> Self {
        let tw = scene.world_matrix(target);
        let offsets = drivers
            .iter()
            .map(|d| {
                if !maintain_offset {
                    return Mat4::IDENTITY;
                }
                let dw = scene.world_matrix(*d);
                match kind {
                    ConstraintKind::Parent => dw.inverse() * tw,
                    ConstraintKind::Point => Mat4::from_translation(tw.transform_point3(Vec3::ZERO) - dw.transform_point3(Vec3::ZERO)),
                    ConstraintKind::Orient | ConstraintKind::Aim => {
                        let (_, dr, _) = dw.to_scale_rotation_translation();
                        let (_, tr, _) = tw.to_scale_rotation_translation();
                        Mat4::from_quat(dr.inverse() * tr)
                    }
                    ConstraintKind::Scale => {
                        let (ds, _, _) = dw.to_scale_rotation_translation();
                        let (ts, _, _) = tw.to_scale_rotation_translation();
                        Mat4::from_scale(ts / ds.max(Vec3::splat(1e-6)))
                    }
                }
            })
            .collect();
        Constraint { kind, drivers, offsets, weight: 1.0, aim: Vec3::X, up: Vec3::Y }
    }
}

fn blend_quats(qs: &[Quat]) -> Quat {
    let mut acc = Vec3::ZERO.extend(0.0);
    let first = qs[0];
    for q in qs {
        let q = if q.dot(first) < 0.0 { -*q } else { *q };
        acc += glam::Vec4::from(q);
    }
    Quat::from_vec4(acc).normalize()
}

fn apply_constraint(scene: &mut Scene, target: NodeId, c: &Constraint) {
    let live: Vec<(Mat4, Mat4)> = c.drivers.iter().zip(&c.offsets).filter(|(d, _)| scene.node(**d).is_some() && **d != target).map(|(d, o)| (scene.world_matrix(*d), *o)).collect();
    if live.is_empty() || c.weight <= 0.0 {
        return;
    }
    let n = live.len() as f32;
    match c.kind {
        ConstraintKind::Parent => {
            let mats: Vec<Mat4> = live.iter().map(|(w, o)| *w * *o).collect();
            let pos = mats.iter().map(|m| m.transform_point3(Vec3::ZERO)).sum::<Vec3>() / n;
            let rots: Vec<Quat> = mats.iter().map(|m| m.to_scale_rotation_translation().1).collect();
            set_world_position(scene, target, pos);
            set_world_rotation(scene, target, blend_quats(&rots));
        }
        ConstraintKind::Point => {
            let pos = live.iter().map(|(w, o)| w.transform_point3(Vec3::ZERO) + o.transform_point3(Vec3::ZERO)).sum::<Vec3>() / n;
            set_world_position(scene, target, pos);
        }
        ConstraintKind::Orient => {
            let rots: Vec<Quat> = live.iter().map(|(w, o)| w.to_scale_rotation_translation().1 * o.to_scale_rotation_translation().1).collect();
            set_world_rotation(scene, target, blend_quats(&rots));
        }
        ConstraintKind::Scale => {
            let s = live.iter().map(|(w, o)| w.to_scale_rotation_translation().0 * o.to_scale_rotation_translation().0).sum::<Vec3>() / n;
            let ps = scene.parent_matrix(target).to_scale_rotation_translation().0;
            if let Some(node) = scene.node_mut(target) {
                node.scale = s / ps.max(Vec3::splat(1e-6));
            }
        }
        ConstraintKind::Aim => {
            let goal = live.iter().map(|(w, _)| w.transform_point3(Vec3::ZERO)).sum::<Vec3>() / n;
            let p = world_pos(scene, target);
            let dir = goal - p;
            if dir.length() < 1e-6 {
                return;
            }
            let dir = dir.normalize();
            // Build a frame whose `aim` axis points at the goal and `up` axis is closest to world up.
            let up = if dir.cross(c.up).length() < 1e-4 { Vec3::Z } else { c.up };
            let side = dir.cross(up).normalize();
            let up2 = side.cross(dir).normalize();
            let frame = Quat::from_mat3(&Mat3::from_cols(dir, up2, dir.cross(up2)));
            // Local axes: aim -> dir, up -> up2.
            let aim = c.aim.normalize_or(Vec3::X);
            let local_up = {
                let u = Vec3::Y - aim * aim.dot(Vec3::Y);
                if u.length() < 1e-4 { Vec3::Z } else { u.normalize() }
            };
            let local = Quat::from_mat3(&Mat3::from_cols(aim, local_up, aim.cross(local_up)));
            set_world_rotation(scene, target, (frame * local.inverse()).normalize());
        }
    }
}

// --------------------------------------------------------------------- set driven key

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrivenKey {
    pub channel: Channel,
    /// When set, the key drives this blend-shape target weight instead of `channel`.
    #[serde(default)]
    pub blend_target: Option<usize>,
    pub driver: NodeId,
    pub driver_channel: Channel,
    /// x = driver value, y = driven value.
    pub curve: Curve,
}

// ------------------------------------------------------------------------- evaluation

/// Evaluate driven keys, constraints and IK handles (call after animation evaluation).
pub fn evaluate(scene: &mut Scene) {
    // Driven keys.
    let mut sets = Vec::new();
    for n in &scene.nodes {
        for dk in &n.driven {
            if let Some(dv) = scene.node(dk.driver).map(|d| d.get(dk.driver_channel)) {
                if let Some(v) = dk.curve.evaluate(dv) {
                    sets.push((n.id, dk.channel, dk.blend_target, v));
                }
            }
        }
    }
    for (id, c, bt, v) in sets {
        if let Some(n) = scene.node_mut(id) {
            match bt {
                Some(i) => {
                    if let Some(w) = n.blend.as_mut().and_then(|b| b.weights.get_mut(i)) {
                        *w = v;
                    }
                }
                None => n.set(c, v),
            }
        }
    }
    // Constraints (two passes so chained constraints settle).
    let constrained: Vec<(NodeId, Vec<Constraint>)> = scene.nodes.iter().filter(|n| !n.constraints.is_empty()).map(|n| (n.id, n.constraints.clone())).collect();
    for _ in 0..2 {
        for (id, cs) in &constrained {
            for c in cs {
                apply_constraint(scene, *id, c);
            }
        }
    }
    // IK handles.
    type Handle = (NodeId, NodeId, NodeId, Option<NodeId>, bool, f32, Vec<(NodeId, Vec3)>);
    let handles: Vec<Handle> = scene
        .nodes
        .iter()
        .filter_map(|n| match &n.shape {
            Shape::IkHandle { start, end, pole, stretch, twist, rest } => Some((n.id, *start, *end, *pole, *stretch, *twist, rest.clone())),
            _ => None,
        })
        .collect();
    for (h, s, e, pole, stretch, twist, rest) in handles {
        if scene.node(s).is_none() || scene.node(e).is_none() {
            continue;
        }
        let goal = world_pos(scene, h);
        if stretch && !rest.is_empty() {
            // Restore rest lengths, then scale bones uniformly if the goal is out of reach.
            for (j, t) in &rest {
                if let Some(n) = scene.node_mut(*j) {
                    n.translate = *t;
                }
            }
            if let Some(chain) = ik_chain(scene, s, e) {
                let len: f32 = chain.windows(2).map(|w| (world_pos(scene, w[1]) - world_pos(scene, w[0])).length()).sum();
                let d = (goal - world_pos(scene, s)).length();
                if len > 1e-5 && d > len {
                    let k = d / len;
                    for j in &chain[1..] {
                        if let Some(n) = scene.node_mut(*j) {
                            n.translate *= k;
                        }
                    }
                }
            }
        }
        let pole = pole.filter(|p| scene.node(*p).is_some()).map(|p| world_pos(scene, p));
        solve_ik_twist(scene, s, e, goal, pole, twist);
    }
}

// ------------------------------------------------------------------------ joint orient

/// Orient joints so X points down the bone to the first child joint and Y is closest to
/// world up (Maya Orient Joint: xyz / yup). Keeps every node's world position.
pub fn orient_joints(scene: &mut Scene, joints: &[NodeId]) {
    // Save world matrices of everything below (to restore non-joint children afterwards).
    let mut affected: Vec<NodeId> = Vec::new();
    for &j in joints {
        for id in scene.subtree(j) {
            if !affected.contains(&id) {
                affected.push(id);
            }
        }
    }
    let saved: Vec<(NodeId, Mat4)> = affected.iter().map(|id| (*id, scene.world_matrix(*id))).collect();
    let world_of = |id: NodeId| saved.iter().find(|(i, _)| *i == id).map(|x| x.1).unwrap();
    // Process in hierarchy order (parents first): `affected` is built from subtree() which is BFS.
    for &id in &affected {
        let w = world_of(id);
        let pos = w.transform_point3(Vec3::ZERO);
        if is_joint(scene, id) && joints.contains(&id) {
            let kids = child_joints(scene, id);
            let rot = match kids.first() {
                Some(k) => {
                    let x = (world_of(*k).transform_point3(Vec3::ZERO) - pos).normalize_or(Vec3::X);
                    let mut z = x.cross(Vec3::Y);
                    if z.length() < 1e-4 {
                        z = x.cross(Vec3::Z);
                    }
                    let z = z.normalize();
                    let y = z.cross(x).normalize();
                    Quat::from_mat3(&Mat3::from_cols(x, y, z))
                }
                // Leaf: inherit the parent's orientation.
                None => parent_rotation(scene, id),
            };
            let local = parent_rotation(scene, id).inverse() * rot;
            set_world_position(scene, id, pos);
            if let Some(n) = scene.node_mut(id) {
                n.joint_orient = quat_euler(local.normalize());
                n.rotate = Vec3::ZERO;
            }
        } else {
            // Restore world transform of non-oriented nodes.
            let local = scene.parent_matrix(id).inverse() * w;
            let (s, r, t) = local.to_scale_rotation_translation();
            if let Some(n) = scene.node_mut(id) {
                n.translate = t;
                n.scale = s;
                n.rotate = quat_euler(euler_quat(n.joint_orient).inverse() * r);
            }
        }
    }
}

// --------------------------------------------------------------------------- controls

pub fn circle_points(radius: f32, normal: Vec3, sections: usize) -> Vec<Vec3> {
    let n = normal.normalize_or(Vec3::Y);
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    (0..sections).map(|i| {
        let a = i as f32 / sections as f32 * std::f32::consts::TAU;
        (u * a.cos() + v * a.sin()) * radius
    }).collect()
}

/// Control-curve presets: (points, closed).
pub fn control_shape(kind: &str, size: f32) -> Option<(Vec<Vec3>, bool)> {
    let s = size;
    Some(match kind {
        "circle" => (circle_points(s, Vec3::Y, 16), true),
        "square" => (vec![Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s)], true),
        "cross" | "plus" => {
            let t = s / 3.0;
            (
                vec![
                    Vec3::new(-t, 0.0, -s), Vec3::new(t, 0.0, -s), Vec3::new(t, 0.0, -t), Vec3::new(s, 0.0, -t),
                    Vec3::new(s, 0.0, t), Vec3::new(t, 0.0, t), Vec3::new(t, 0.0, s), Vec3::new(-t, 0.0, s),
                    Vec3::new(-t, 0.0, t), Vec3::new(-s, 0.0, t), Vec3::new(-s, 0.0, -t), Vec3::new(-t, 0.0, -t),
                ],
                true,
            )
        }
        "arrow" => (
            vec![
                Vec3::new(-s * 0.3, 0.0, s), Vec3::new(s * 0.3, 0.0, s), Vec3::new(s * 0.3, 0.0, 0.0), Vec3::new(s * 0.7, 0.0, 0.0),
                Vec3::new(0.0, 0.0, -s), Vec3::new(-s * 0.7, 0.0, 0.0), Vec3::new(-s * 0.3, 0.0, 0.0),
            ],
            true,
        ),
        "cube" | "box" => {
            let c = |x: f32, y: f32, z: f32| Vec3::new(x * s, y * s, z * s);
            (
                vec![
                    c(-1., -1., -1.), c(1., -1., -1.), c(1., -1., 1.), c(-1., -1., 1.), c(-1., -1., -1.),
                    c(-1., 1., -1.), c(1., 1., -1.), c(1., -1., -1.), c(1., 1., -1.), c(1., 1., 1.),
                    c(1., -1., 1.), c(1., 1., 1.), c(-1., 1., 1.), c(-1., -1., 1.), c(-1., 1., 1.), c(-1., 1., -1.),
                ],
                false,
            )
        }
        "sphere" => {
            let mut p = circle_points(s, Vec3::Y, 16);
            p.push(p[0]);
            let side: Vec<Vec3> = circle_points(s, Vec3::X, 16);
            let start = side.iter().position(|q| (*q - p[0]).length() < 1e-3).unwrap_or(0);
            p.extend(side.iter().cycle().skip(start).take(17));
            let front: Vec<Vec3> = circle_points(s, Vec3::Z, 16);
            p.extend(front.iter().copied());
            p.push(front[0]);
            (p, false)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Primitive;

    fn chain(scene: &mut Scene) -> (NodeId, NodeId, NodeId) {
        let a = scene.add_node("joint", Shape::Joint { radius: 0.5 }, None);
        let b = scene.add_node("joint", Shape::Joint { radius: 0.5 }, Some(a));
        let c = scene.add_node("joint", Shape::Joint { radius: 0.5 }, Some(b));
        scene.node_mut(b).unwrap().translate = Vec3::new(2.0, 0.0, 0.0);
        scene.node_mut(c).unwrap().translate = Vec3::new(2.0, 0.0, 0.0);
        (a, b, c)
    }

    #[test]
    fn two_bone_ik_reaches_goal_and_respects_pole() {
        let mut s = Scene::default();
        let (a, b, c) = chain(&mut s);
        let goal = Vec3::new(2.0, 2.0, 0.0);
        solve_ik(&mut s, a, c, goal, Some(Vec3::new(0.0, 0.0, 5.0)));
        assert!((world_pos(&s, c) - goal).length() < 1e-3, "end {:?}", world_pos(&s, c));
        assert!((world_pos(&s, b) - world_pos(&s, a)).length() - 2.0 < 1e-3);
        // Knee bends toward the pole (+Z).
        assert!(world_pos(&s, b).z > 0.1, "mid {:?}", world_pos(&s, b));
        // Unreachable goal: chain straightens toward it.
        solve_ik(&mut s, a, c, Vec3::new(10.0, 0.0, 0.0), None);
        assert!((world_pos(&s, c) - Vec3::new(4.0, 0.0, 0.0)).length() < 0.02);
    }

    #[test]
    fn ccd_for_longer_chains() {
        let mut s = Scene::default();
        let (_, _, c) = chain(&mut s);
        let d = s.add_node("joint", Shape::Joint { radius: 0.5 }, Some(c));
        s.node_mut(d).unwrap().translate = Vec3::new(2.0, 0.0, 0.0);
        let a = s.find_by_name("joint1").unwrap();
        let goal = Vec3::new(3.0, 3.0, 1.0);
        solve_ik(&mut s, a, d, goal, None);
        assert!((world_pos(&s, d) - goal).length() < 1e-2);
    }

    #[test]
    fn orient_joint_points_x_down_the_bone() {
        let mut s = Scene::default();
        let (a, b, c) = chain(&mut s);
        s.node_mut(b).unwrap().translate = Vec3::new(0.0, 2.0, 0.0);
        let before: Vec<Vec3> = [a, b, c].iter().map(|j| world_pos(&s, *j)).collect();
        orient_joints(&mut s, &[a, b, c]);
        let after: Vec<Vec3> = [a, b, c].iter().map(|j| world_pos(&s, *j)).collect();
        for (p, q) in before.iter().zip(&after) {
            assert!((*p - *q).length() < 1e-4);
        }
        let x_axis = world_rotation(&s, a) * Vec3::X;
        assert!((x_axis - Vec3::Y).length() < 1e-4, "{x_axis}");
        assert_eq!(s.node(a).unwrap().rotate, Vec3::ZERO);
        // Children's translations are now along +X of their parent.
        assert!((s.node(b).unwrap().translate - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn skinning_follows_joints() {
        let mut s = Scene::default();
        let (a, b, c) = chain(&mut s);
        let m = s.add_primitive(Primitive::Cylinder { radius: 0.3, height: 4.0, axis_div: 8, height_div: 8, caps: true });
        s.node_mut(m).unwrap().rotate = Vec3::new(0.0, 0.0, -90.0); // lie along +X
        s.node_mut(m).unwrap().translate = Vec3::new(2.0, 0.0, 0.0);
        let skin = bind_skin(&s, m, &[a, b, c], 3, 4.0).unwrap();
        for w in &skin.weights {
            assert!((w.iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-4);
            assert!(w.len() <= 3);
        }
        s.node_mut(m).unwrap().skin = Some(skin.clone());
        // Bind pose: no deformation.
        let rest = deformed_positions(&s, m).unwrap();
        let wm = s.world_matrix(m);
        let mesh = s.node(m).unwrap().mesh().unwrap().clone();
        for (p, q) in mesh.positions.iter().zip(&rest) {
            assert!((wm.transform_point3(*p) - *q).length() < 1e-4, "{} vs {}", wm.transform_point3(*p), q);
        }
        // Bend the elbow 90°: the tip vertices follow the forearm.
        s.node_mut(b).unwrap().rotate = Vec3::new(0.0, 0.0, 90.0);
        let bent = deformed_positions(&s, m).unwrap();
        let tip = bent.iter().cloned().fold(Vec3::splat(f32::MIN), Vec3::max);
        assert!(tip.y > 1.5, "tip should swing up, got {tip}");
        // Painting a weight renormalises.
        let mut sk = skin;
        sk.set_weight(0, 1, 0.75);
        assert!((sk.weights[0].iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-4);
        assert!((sk.weight(0, 1) - 0.75).abs() < 1e-4);
        goto_bind_pose(&mut s, &sk);
        assert_eq!(s.node(b).unwrap().rotate, Vec3::ZERO);
    }

    #[test]
    fn constraints() {
        let mut s = Scene::default();
        let drv = s.add_node("ctrl", Shape::Locator, None);
        let tgt = s.add_node("tgt", Shape::Locator, None);
        s.node_mut(tgt).unwrap().translate = Vec3::new(1.0, 0.0, 0.0);
        let c = Constraint::new(&s, ConstraintKind::Parent, vec![drv], tgt, true);
        s.node_mut(tgt).unwrap().constraints.push(c);
        s.node_mut(drv).unwrap().rotate = Vec3::new(0.0, 90.0, 0.0);
        s.node_mut(drv).unwrap().translate = Vec3::new(0.0, 5.0, 0.0);
        evaluate(&mut s);
        // Offset (1,0,0) rotated 90° about Y -> (0,0,-1), plus driver translation.
        assert!((world_pos(&s, tgt) - Vec3::new(0.0, 5.0, -1.0)).length() < 1e-4, "{}", world_pos(&s, tgt));
        // Aim: X axis of the target points at the driver.
        let aimer = s.add_node("aimer", Shape::Locator, None);
        let c = Constraint::new(&s, ConstraintKind::Aim, vec![drv], aimer, false);
        s.node_mut(aimer).unwrap().constraints.push(c);
        evaluate(&mut s);
        let x = world_rotation(&s, aimer) * Vec3::X;
        assert!((x - Vec3::Y).length() < 1e-3, "{x}");
        // Point with two drivers averages.
        let d2 = s.add_node("d2", Shape::Locator, None);
        s.node_mut(d2).unwrap().translate = Vec3::new(4.0, 5.0, 0.0);
        let pt = s.add_node("pt", Shape::Locator, None);
        let c = Constraint::new(&s, ConstraintKind::Point, vec![drv, d2], pt, false);
        s.node_mut(pt).unwrap().constraints.push(c);
        evaluate(&mut s);
        assert!((world_pos(&s, pt) - Vec3::new(2.0, 5.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn driven_keys() {
        let mut s = Scene::default();
        let d = s.add_node("driver", Shape::Locator, None);
        let t = s.add_node("door", Shape::Locator, None);
        let mut curve = Curve { tangent: crate::scene::Tangent::Linear, ..Default::default() };
        curve.set_key(0.0, 0.0);
        curve.set_key(10.0, 90.0);
        s.node_mut(t).unwrap().driven.push(DrivenKey { channel: Channel::RotateY, blend_target: None, driver: d, driver_channel: Channel::TranslateY, curve });
        s.node_mut(d).unwrap().translate.y = 5.0;
        evaluate(&mut s);
        assert!((s.node(t).unwrap().rotate.y - 45.0).abs() < 1e-4);
    }

    #[test]
    fn control_shapes() {
        for k in ["circle", "square", "cross", "arrow", "cube", "sphere"] {
            let (p, _) = control_shape(k, 1.0).unwrap();
            assert!(p.len() >= 4, "{k}");
        }
        assert!(control_shape("blob", 1.0).is_none());
    }
}
