//! Hair: dynamic strands grown from a mesh (they follow its deformation, e.g. a skinned head),
//! and dynamic curves (one strand that follows a NURBS curve and writes its motion to an output
//! curve, like Maya's "Make Curves Dynamic").
//!
//! Each strand is a chain of points simulated with position-based dynamics: gravity, drag,
//! wind, fields, an attraction toward the strand's rest shape (stiff at the root, loose at the
//! tip), inextensible segments (follow-the-leader from the root), and collisions with the
//! growth mesh, colliders and the ground. Like particles and cloth, the simulation is cached
//! per frame and rebuilt when its inputs change.

use crate::fx::{FieldSet, Rng, SimCache, Tracks, TriGrid, MAX_FRAMES};
use crate::scene::{NodeId, Scene, Shape};
use glam::{Mat3, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HairSettings {
    /// Growth mesh, or the curve made dynamic.
    pub source: Option<NodeId>,
    /// Output curve (dynamic curves): receives the simulated points.
    pub output: Option<NodeId>,
    pub count: u32,
    pub length: f32,
    /// ± fraction of length.
    pub length_random: f32,
    pub segments: u32,
    /// Random tilt of the root direction away from the surface normal (degrees).
    pub tilt: f32,
    /// Random bend along each strand (degrees, root to tip).
    pub curl: f32,
    /// World direction the rest shape is combed toward, and how much (0..1).
    pub comb: Vec3,
    pub comb_amount: f32,
    /// Attraction to the rest shape at the root and at the tip (0..1 per frame).
    pub stiffness: f32,
    pub tip_stiffness: f32,
    pub damping: f32,
    pub drag: f32,
    pub gravity: Vec3,
    pub wind: Vec3,
    pub colliders: Vec<NodeId>,
    /// Collide with the growth mesh too.
    pub collide_source: bool,
    /// Collision distance.
    pub thickness: f32,
    pub ground: bool,
    pub ground_height: f32,
    pub width: f32,
    pub tip_width: f32,
    pub color: [f32; 3],
    pub tip_color: [f32; 3],
    /// Dynamic curves: hold the tip at the input curve's end as well as the root.
    pub pin_tip: bool,
    pub seed: u32,
    pub start: f32,
    pub substeps: u32,
    pub iterations: u32,
}

impl Default for HairSettings {
    fn default() -> Self {
        HairSettings {
            source: None,
            output: None,
            count: 400,
            length: 0.3,
            length_random: 0.2,
            segments: 8,
            tilt: 15.0,
            curl: 20.0,
            comb: Vec3::ZERO,
            comb_amount: 0.0,
            stiffness: 0.25,
            tip_stiffness: 0.02,
            damping: 0.05,
            drag: 0.5,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            wind: Vec3::ZERO,
            colliders: Vec::new(),
            collide_source: true,
            thickness: 0.01,
            ground: true,
            ground_height: 0.0,
            width: 0.012,
            tip_width: 0.003,
            color: [0.35, 0.22, 0.12],
            tip_color: [0.6, 0.45, 0.28],
            pin_tip: false,
            seed: 1,
            start: 1.0,
            substeps: 4,
            iterations: 4,
        }
    }
}

impl HairSettings {
    pub fn set_attr(&mut self, attr: &str, vals: &[f32]) -> Result<(), String> {
        use crate::fx::{one, v3};
        match attr {
            "count" => self.count = (one(vals)? as u32).clamp(1, 200_000),
            "length" => self.length = one(vals)?.max(1e-4),
            "lengthRandom" => self.length_random = one(vals)?.clamp(0.0, 1.0),
            "segments" => self.segments = (one(vals)? as u32).clamp(1, 64),
            "tilt" => self.tilt = one(vals)?.clamp(0.0, 90.0),
            "curl" => self.curl = one(vals)?.clamp(0.0, 720.0),
            "comb" => self.comb = v3(vals)?,
            "combAmount" => self.comb_amount = one(vals)?.clamp(0.0, 1.0),
            "stiffness" => self.stiffness = one(vals)?.clamp(0.0, 1.0),
            "tipStiffness" => self.tip_stiffness = one(vals)?.clamp(0.0, 1.0),
            "damping" | "damp" => self.damping = one(vals)?.clamp(0.0, 1.0),
            "drag" => self.drag = one(vals)?.max(0.0),
            "gravity" => self.gravity = if vals.len() >= 3 { v3(vals)? } else { Vec3::new(0.0, -one(vals)?, 0.0) },
            "wind" => self.wind = v3(vals)?,
            "collideSource" => self.collide_source = one(vals)? > 0.5,
            "thickness" => self.thickness = one(vals)?.max(0.0),
            "ground" => self.ground = one(vals)? > 0.5,
            "groundHeight" => self.ground_height = one(vals)?,
            "width" => self.width = one(vals)?.max(1e-5),
            "tipWidth" => self.tip_width = one(vals)?.max(0.0),
            "color" => self.color = v3(vals)?.to_array(),
            "tipColor" => self.tip_color = v3(vals)?.to_array(),
            "pinTip" => self.pin_tip = one(vals)? > 0.5,
            "seed" => self.seed = one(vals)?.max(0.0) as u32,
            "startFrame" => self.start = one(vals)?,
            "substeps" => self.substeps = (one(vals)? as u32).clamp(1, 32),
            "iterations" => self.iterations = (one(vals)? as u32).clamp(1, 32),
            _ => {
                return Err(format!(
                    "Unknown hair attribute '{attr}' (count length lengthRandom segments tilt curl comb combAmount stiffness tipStiffness damping drag gravity wind collideSource thickness ground groundHeight width tipWidth color tipColor pinTip seed startFrame substeps iterations)"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct HairFrame {
    /// All strand points, strand after strand (`segments + 1` each), world space.
    pub pos: Vec<Vec3>,
    pub prev: Vec<Vec3>,
    /// Rest-shape goals at this frame (interpolated over the next frame's substeps).
    pub goal: Vec<Vec3>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Hair {
    pub settings: HairSettings,
    #[serde(skip)]
    pub cache: Arc<SimCache<HairFrame>>,
}

impl Hair {
    pub fn current(&self) -> Option<&[Vec3]> {
        if let Some(b) = &self.cache.blend {
            return Some(&b.pos);
        }
        self.cache.current.and_then(|i| self.cache.frames.get(i)).map(|f| f.pos.as_slice())
    }
    pub fn points_per_strand(&self) -> usize {
        self.settings.segments.max(1) as usize + 1
    }
    /// Current strands (world space).
    pub fn strands(&self) -> Vec<&[Vec3]> {
        let n = self.points_per_strand();
        self.current().map(|p| p.chunks(n).collect()).unwrap_or_default()
    }
}

// ------------------------------------------------------------------------------ setup

/// Where a strand grows and its rest shape.
struct Root {
    tri: [u32; 3],
    bary: Vec3,
    /// Unit segment directions in the root frame (x = tangent, y = normal, z = bitangent).
    dirs: Vec<Vec3>,
    seg: f32,
}

enum Setup {
    Mesh { roots: Vec<Root> },
    Curve { seg: f32 },
}

fn tris_of(faces: &[Vec<u32>]) -> Vec<[u32; 3]> {
    let mut t = Vec::new();
    for f in faces {
        for i in 1..f.len().saturating_sub(1) {
            t.push([f[0], f[i], f[i + 1]]);
        }
    }
    t
}

/// Rotate `v` about `axis` by `deg`.
fn rot(v: Vec3, axis: Vec3, deg: f32) -> Vec3 {
    glam::Quat::from_axis_angle(axis.normalize_or(Vec3::X), deg.to_radians()) * v
}

fn build_setup(tr: &Tracks, s: &HairSettings) -> Option<Setup> {
    let src = s.source?;
    let segs = s.segments.max(1) as usize;
    match &tr.sc.node(src)?.shape {
        Shape::Poly { .. } => {
            let (pos, faces) = tr.mesh_world(src)?;
            let tris = tris_of(&faces);
            let mut cum = Vec::with_capacity(tris.len());
            let mut acc = 0.0;
            for t in &tris {
                let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
                acc += (b - a).cross(c - a).length() * 0.5;
                cum.push(acc);
            }
            if acc <= 0.0 {
                return None;
            }
            let mut rng = Rng::new(s.seed as u64 ^ 0x4841_4952);
            let mut roots = Vec::with_capacity(s.count as usize);
            for _ in 0..s.count {
                let x = rng.f() * acc;
                let ti = cum.partition_point(|c| *c < x).min(tris.len() - 1);
                let (mut u, mut v) = (rng.f(), rng.f());
                if u + v > 1.0 {
                    u = 1.0 - u;
                    v = 1.0 - v;
                }
                let bary = Vec3::new(1.0 - u - v, u, v);
                let len = s.length * (1.0 + s.length_random * rng.s());
                // Root direction: the normal (local +Y) tilted at random; then a gentle random curl.
                let az = rng.f() * std::f32::consts::TAU;
                let tilt_axis = Vec3::new(az.cos(), 0.0, az.sin());
                let mut d = rot(Vec3::Y, tilt_axis, s.tilt * rng.f());
                let curl_axis = {
                    let a = rng.f() * std::f32::consts::TAU;
                    let c = Vec3::new(a.cos(), 0.0, a.sin());
                    d.cross(c).normalize_or(Vec3::X)
                };
                let step = s.curl * (0.5 + rng.f()) / segs as f32;
                let mut dirs = Vec::with_capacity(segs);
                for _ in 0..segs {
                    dirs.push(d);
                    d = rot(d, curl_axis, step);
                }
                roots.push(Root { tri: tris[ti], bary, dirs, seg: len / segs as f32 });
            }
            Some(Setup::Mesh { roots })
        }
        Shape::Curve { .. } => {
            let c = crate::nurbs::world_curve(&tr.sc, src).filter(|c| c.is_valid())?;
            Some(Setup::Curve { seg: c.length() / segs as f32 })
        }
        _ => None,
    }
}

/// Rest-shape goal points at the tracks' current frame.
fn goals(tr: &Tracks, s: &HairSettings, setup: &Setup) -> Option<Vec<Vec3>> {
    let src = s.source?;
    let segs = s.segments.max(1) as usize;
    match setup {
        Setup::Mesh { roots } => {
            let (pos, _) = tr.mesh_world(src)?;
            let comb = s.comb.normalize_or_zero();
            let mut out = Vec::with_capacity(roots.len() * (segs + 1));
            for r in roots {
                let get = |i: u32| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO);
                let (a, b, c) = (get(r.tri[0]), get(r.tri[1]), get(r.tri[2]));
                let n = (b - a).cross(c - a).normalize_or(Vec3::Y);
                let t = (b - a - n * n.dot(b - a)).normalize_or(n.any_orthonormal_vector());
                let m = Mat3::from_cols(t, n, t.cross(n));
                let mut p = a * r.bary.x + b * r.bary.y + c * r.bary.z;
                out.push(p);
                for d in &r.dirs {
                    let mut w = m * *d;
                    if comb != Vec3::ZERO && s.comb_amount > 0.0 {
                        w = w.lerp(comb, s.comb_amount).normalize_or(w);
                    }
                    p += w * r.seg;
                    out.push(p);
                }
            }
            Some(out)
        }
        Setup::Curve { .. } => {
            let c = crate::nurbs::world_curve(&tr.sc, src).filter(|c| c.is_valid())?;
            Some(c.resample(segs + 1))
        }
    }
}

fn seg_lengths(setup: &Setup, strands: usize) -> Vec<f32> {
    match setup {
        Setup::Mesh { roots } => roots.iter().map(|r| r.seg).collect(),
        Setup::Curve { seg } => vec![*seg; strands],
    }
}

// ------------------------------------------------------------------------------ simulation

#[allow(clippy::too_many_arguments)]
fn step_hair(prev: &HairFrame, goal_now: &[Vec3], s: &HairSettings, seg: &[f32], fps: f32, grid: &TriGrid, fields: &FieldSet, t0: f32) -> HairFrame {
    let per = s.segments.max(1) as usize + 1;
    let sub = s.substeps.clamp(1, 32) as usize;
    let iters = s.iterations.clamp(1, 32) as usize;
    let dt = 1.0 / fps.max(1.0) / sub as f32;
    let mut pos = prev.pos.clone();
    let mut old = prev.prev.clone();
    let goal_prev = &prev.goal;
    let keep = (1.0 - s.damping.clamp(0.0, 1.0)).powf(dt);
    // Per-iteration attraction so the per-frame strength doesn't depend on substeps/iterations.
    let n_apply = (sub * iters) as f32;
    let attract: Vec<f32> = (0..per)
        .map(|i| {
            let k = s.stiffness + (s.tip_stiffness - s.stiffness) * i as f32 / (per - 1).max(1) as f32;
            1.0 - (1.0 - k.clamp(0.0, 0.999)).powf(1.0 / n_apply)
        })
        .collect();
    let r = s.thickness.max(1e-4);
    let mut scratch = Vec::new();
    let strands = pos.len() / per;
    let pinned = |i: usize| i == 0 || (s.pin_tip && i == per - 1);
    for step in 0..sub {
        let f = (step + 1) as f32 / sub as f32;
        let t = t0 + step as f32 * dt;
        let goal = |k: usize| goal_prev[k].lerp(goal_now[k], f);
        // Integrate.
        for j in 0..strands {
            for i in 0..per {
                let k = j * per + i;
                if pinned(i) {
                    old[k] = pos[k];
                    pos[k] = goal(k);
                    continue;
                }
                let v = (pos[k] - old[k]) / dt * keep;
                let mut a = s.gravity + (s.wind - v) * s.drag;
                if !fields.is_empty() {
                    a += fields.accel(pos[k], v, t);
                }
                old[k] = pos[k];
                pos[k] += v * dt + a * dt * dt;
            }
        }
        // Follow-the-leader corrections this substep (for the DFTL velocity correction).
        let mut corr = vec![Vec3::ZERO; if s.pin_tip { 0 } else { pos.len() }];
        for _ in 0..iters {
            for j in 0..strands {
                let base = j * per;
                // Shape attraction.
                for (i, k_att) in attract.iter().enumerate().skip(1) {
                    if *k_att > 0.0 && !pinned(i) {
                        let k = base + i;
                        let d = (goal(k) - pos[k]) * *k_att;
                        pos[k] += d;
                    }
                }
                let l = seg[j];
                if s.pin_tip {
                    // Both ends held: symmetric distance constraints.
                    for i in 0..per - 1 {
                        let (a, b) = (base + i, base + i + 1);
                        let (wa, wb) = (if pinned(i) { 0.0 } else { 1.0 }, if pinned(i + 1) { 0.0 } else { 1.0 });
                        if wa + wb == 0.0 {
                            continue;
                        }
                        let d = pos[b] - pos[a];
                        let len = d.length();
                        if len < 1e-9 {
                            continue;
                        }
                        let c = d * ((len - l) / len / (wa + wb));
                        pos[a] += c * wa;
                        pos[b] -= c * wb;
                    }
                } else {
                    // Follow the leader: exact segment lengths from the root out.
                    for i in 1..per {
                        let (a, b) = (base + i - 1, base + i);
                        let d = pos[b] - pos[a];
                        let np = pos[a] + d.normalize_or(Vec3::NEG_Y) * l;
                        corr[b] += np - pos[b];
                        pos[b] = np;
                    }
                }
            }
        }
        // DFTL (Müller et al. 2012): hand each point part of its child's correction as a
        // velocity change, so the strand moves like a rope instead of a whip.
        if !s.pin_tip {
            for j in 0..strands {
                for i in 1..per - 1 {
                    let k = j * per + i;
                    old[k] += corr[k + 1] * 0.9;
                }
            }
        }
        // Collisions (not the root).
        for j in 0..strands {
            for i in 1..per {
                if pinned(i) {
                    continue;
                }
                let k = j * per + i;
                if !grid.is_empty()
                    && let Some((q, n)) = grid.closest(pos[k], r * 4.0, &mut scratch)
                {
                    let d = (pos[k] - q).dot(n);
                    if d < r {
                        pos[k] += n * (r - d);
                    }
                }
                if s.ground && pos[k].y < s.ground_height + r {
                    pos[k].y = s.ground_height + r;
                }
            }
        }
    }
    HairFrame { pos, prev: old, goal: goal_now.to_vec() }
}

fn hair_key(scene: &Scene, id: NodeId, s: &HairSettings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // The output curve is written by the simulation: it isn't an input.
    let mut st = s.clone();
    st.output = None;
    format!("{st:?}").hash(&mut h);
    scene.fps.to_bits().hash(&mut h);
    crate::fx::hash_fields(scene, id, &mut h);
    for c in s.source.iter().chain(&s.colliders) {
        crate::fx::motion_hash(scene, *c, &mut h);
        if let Some(n) = scene.node(*c) {
            n.mesh_rev().hash(&mut h);
            if let Shape::Curve { points, degree, .. } = &n.shape {
                degree.hash(&mut h);
                for p in points {
                    p.to_array().map(f32::to_bits).hash(&mut h);
                }
            }
            if let Some(sk) = &n.skin {
                for j in &sk.joints {
                    crate::fx::motion_hash(scene, *j, &mut h);
                }
            }
            for c in n.clusters.iter().map(|c| c.handle) {
                crate::fx::motion_hash(scene, c, &mut h);
            }
        }
    }
    h.finish()
}

/// Bring every hair system to the current frame (called from `rig::evaluate` after `fx::update`).
/// True when a dynamic curve's output curve changed.
pub fn update(scene: &mut Scene) -> bool {
    let mut curves_changed = false;
    let jobs: Vec<NodeId> = scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Hair(_))).map(|n| n.id).collect();
    let time = scene.time;
    let fps = scene.fps;
    for id in jobs {
        let Some(Shape::Hair(hair)) = scene.node(id).map(|n| &n.shape) else { continue };
        let s = hair.settings.clone();
        let key = hair_key(scene, id, &s);
        let mut cache = (*hair.cache).clone();
        let (base, frac) = crate::fx::sub_frame(time, s.start);
        // Between frames, simulate one frame further and blend.
        let target = if frac > 0.0 { (base + 1).min(MAX_FRAMES) } else { base };
        let stale = cache.key != key || cache.frames.is_empty();
        if stale || target + 1 > cache.frames.len() {
            let mut keep = vec![id];
            keep.extend(s.source.iter().chain(&s.colliders));
            let mut tr = Tracks::new(scene, &keep);
            tr.goto(s.start);
            let Some(setup) = build_setup(&tr, &s) else {
                // Nothing to grow from: an empty cache.
                cache = SimCache { key, frames: vec![Arc::new(HairFrame::default())], current: Some(0), version: cache.version.wrapping_add(1), blend: None, shift: 0.0 };
                if let Some(Shape::Hair(h)) = scene.node_mut(id).map(|n| &mut n.shape) {
                    h.cache = Arc::new(cache);
                }
                continue;
            };
            let Some(g0) = goals(&tr, &s, &setup) else { continue };
            let per = s.segments.max(1) as usize + 1;
            let seg = seg_lengths(&setup, g0.len() / per);
            if stale {
                cache = SimCache { key, frames: vec![Arc::new(HairFrame { pos: g0.clone(), prev: g0.clone(), goal: g0 })], current: None, version: cache.version.wrapping_add(1), blend: None, shift: 0.0 };
            }
            let mut colliders = s.colliders.clone();
            if s.collide_source && s.source.is_some_and(|src| scene.node(src).is_some_and(|n| n.mesh().is_some())) {
                colliders.extend(s.source);
            }
            for k in cache.frames.len()..=target {
                tr.goto(s.start + k as f32);
                let Some(g) = goals(&tr, &s, &setup) else { break };
                let grid = tr.colliders(&colliders);
                let fields = tr.fields(id);
                let next = step_hair(cache.frames.last().unwrap(), &g, &s, &seg, fps, &grid, &fields, k as f32 / fps.max(1.0));
                cache.frames.push(Arc::new(next));
            }
        }
        let cur = base.min(cache.frames.len() - 1);
        let frac = if cur + 1 < cache.frames.len() { frac } else { 0.0 };
        if cache.current != Some(cur) || cache.shift != frac {
            cache.current = Some(cur);
            cache.shift = frac;
            cache.blend = (frac > 0.0).then(|| {
                let (a, b) = (&cache.frames[cur], &cache.frames[cur + 1]);
                Arc::new(HairFrame { pos: crate::fx::lerp_points(&a.pos, &b.pos, frac), prev: a.prev.clone(), goal: crate::fx::lerp_points(&a.goal, &b.goal, frac) })
            });
            cache.version = cache.version.wrapping_add(1);
        }
        let out_pts = cache.blend.as_ref().or(cache.frames.get(cur)).map(|f| f.pos.clone());
        if let Some(Shape::Hair(h)) = scene.node_mut(id).map(|n| &mut n.shape) {
            h.cache = Arc::new(cache);
        }
        // Dynamic curve: write the strand into the output curve (in its local space).
        if let (Some(out), Some(pts)) = (s.output, out_pts)
            && !pts.is_empty()
        {
            let inv = scene.world_matrix(out).inverse();
            if let Some(Shape::Curve { points, knots, .. }) = scene.node_mut(out).map(|n| &mut n.shape) {
                let local: Vec<Vec3> = pts.iter().map(|p| inv.transform_point3(*p)).collect();
                if *points != local {
                    *points = local;
                    knots.clear();
                    curves_changed = true;
                }
            }
        }
    }
    curves_changed
}

/// Tapered tube triangles (world space) for drawing: (position, normal, colour).
pub fn hair_tris(h: &Hair) -> Vec<(Vec3, Vec3, [f32; 3])> {
    let s = &h.settings;
    let mut out = Vec::new();
    let strands = h.strands();
    let sides = 3;
    for st in strands {
        let n = st.len();
        if n < 2 {
            continue;
        }
        // A frame per point (parallel-ish transport from an arbitrary start).
        let mut rings: Vec<Vec<(Vec3, Vec3)>> = Vec::with_capacity(n);
        let mut side = {
            let t = (st[1] - st[0]).normalize_or(Vec3::Y);
            t.any_orthonormal_vector()
        };
        for i in 0..n {
            let t = if i + 1 < n { st[i + 1] - st[i] } else { st[i] - st[i - 1] }.normalize_or(Vec3::Y);
            side = (side - t * side.dot(t)).normalize_or(t.any_orthonormal_vector());
            let up = t.cross(side);
            let k = i as f32 / (n - 1) as f32;
            let r = 0.5 * (s.width + (s.tip_width - s.width) * k);
            rings.push(
                (0..sides)
                    .map(|a| {
                        let ang = a as f32 / sides as f32 * std::f32::consts::TAU;
                        let d = side * ang.cos() + up * ang.sin();
                        (st[i] + d * r, d)
                    })
                    .collect(),
            );
        }
        for i in 0..n - 1 {
            let c = |k: f32| [s.color[0] + (s.tip_color[0] - s.color[0]) * k, s.color[1] + (s.tip_color[1] - s.color[1]) * k, s.color[2] + (s.tip_color[2] - s.color[2]) * k];
            let (c0, c1) = (c(i as f32 / (n - 1) as f32), c((i + 1) as f32 / (n - 1) as f32));
            let (ra, rb) = (&rings[i], &rings[i + 1]);
            for a in 0..sides {
                let b = (a + 1) % sides;
                for (p, nn, col) in [(ra[a], c0), (rb[a], c1), (rb[b], c1), (ra[a], c0), (rb[b], c1), (ra[b], c0)].map(|(x, col)| (x.0, x.1, col)) {
                    out.push((p, nn, col));
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------ app actions

use crate::app::App;

impl App {
    /// Grow a hair system from a mesh. Lengths and widths of 0 are sized from the mesh.
    pub fn create_hair(&mut self, mesh: NodeId, mut s: HairSettings, name: Option<&str>) -> Result<NodeId, String> {
        if self.scene.node(mesh).and_then(|n| n.mesh()).is_none() {
            return Err("hairSystem: select a polygon mesh to grow hair from.".into());
        }
        let size = self.world_bounds(&[mesh]).map(|(a, b)| (b - a).length()).unwrap_or(1.0);
        if s.length <= 0.0 {
            s.length = size * 0.2;
        }
        if s.width <= 0.0 {
            s.width = (size * 0.006).max(1e-4);
            s.tip_width = s.width * 0.25;
        }
        if s.thickness <= 0.0 {
            s.thickness = (size * 0.006).max(1e-4);
        }
        s.source = Some(mesh);
        s.start = self.scene.play_start;
        self.checkpoint();
        let id = self.scene.add_node("hairSystem1", Shape::Hair(Hair { settings: s, cache: Default::default() }), None);
        self.apply_name(id, name);
        self.sel = vec![id];
        Ok(id)
    }

    /// Make a curve dynamic: a one-strand hair system following it, writing to a new output curve.
    pub fn make_curve_dynamic(&mut self, curve: NodeId, pin_tip: bool, name: Option<&str>) -> Result<(NodeId, NodeId), String> {
        let n = self.scene.node(curve).ok_or("makeCurvesDynamic: no such curve")?;
        let Shape::Curve { points, degree, closed, .. } = &n.shape else {
            return Err("makeCurvesDynamic: select a curve.".into());
        };
        if *closed {
            return Err("makeCurvesDynamic: closed curves can't be made dynamic.".into());
        }
        let segs = (points.len() * 3).clamp(8, 40) as u32;
        let deg = (*degree).clamp(1, 3);
        let cname = n.name.clone();
        let c = crate::nurbs::world_curve(&self.scene, curve).filter(|c| c.is_valid()).ok_or("makeCurvesDynamic: invalid curve")?;
        let len = c.length();
        let pts = c.resample(segs as usize + 1);
        self.checkpoint();
        let out = self.scene.add_node(&format!("{cname}_dyn"), Shape::Curve { points: pts, closed: false, degree: deg, knots: Vec::new() }, None);
        self.apply_name(out, Some(&format!("{cname}_dyn")));
        if let Some(o) = self.scene.node_mut(out) {
            o.color = [0.3, 0.9, 1.0];
        }
        let s = HairSettings {
            source: Some(curve),
            output: Some(out),
            count: 1,
            segments: segs,
            length: len,
            stiffness: 0.0,
            tip_stiffness: 0.0,
            drag: 0.2,
            pin_tip,
            collide_source: false,
            width: (len * 0.01).max(1e-4),
            tip_width: (len * 0.01).max(1e-4),
            thickness: (len * 0.01).max(1e-4),
            color: [0.3, 0.9, 1.0],
            tip_color: [0.3, 0.9, 1.0],
            start: self.scene.play_start,
            ..Default::default()
        };
        let id = self.scene.add_node("hairSystem1", Shape::Hair(Hair { settings: s, cache: Default::default() }), None);
        self.apply_name(id, Some(name.unwrap_or(&format!("{cname}_hairSystem"))));
        self.sel = vec![out];
        Ok((id, out))
    }

    /// Bake the current hair strands into static curves (grouped).
    pub fn hair_to_curves(&mut self, id: NodeId) -> Result<NodeId, String> {
        crate::rig::evaluate(&mut self.scene);
        let Some(Shape::Hair(h)) = self.scene.node(id).map(|n| &n.shape) else {
            return Err("hairToCurves: select a hair system.".into());
        };
        let strands: Vec<Vec<Vec3>> = h.strands().into_iter().map(|s| s.to_vec()).collect();
        if strands.is_empty() {
            return Err("hairToCurves: the hair system has no strands yet.".into());
        }
        let name = self.scene.node(id).map(|n| n.name.clone()).unwrap_or_default();
        self.checkpoint();
        let g = self.scene.add_node(&format!("{name}_curves"), Shape::Group, None);
        self.apply_name(g, Some(&format!("{name}_curves")));
        for (i, st) in strands.into_iter().enumerate() {
            let c = self.scene.add_node(&format!("{name}_curve{}", i + 1), Shape::Curve { points: st, closed: false, degree: 3, knots: Vec::new() }, Some(g));
            self.apply_name(c, Some(&format!("{name}_curve{}", i + 1)));
        }
        self.sel = vec![g];
        Ok(g)
    }
}

/// `getAttr` on hair systems.
pub fn get_attr(h: &Hair, attr: &str) -> Option<String> {
    let s = &h.settings;
    Some(match attr {
        "strands" => h.strands().len().to_string(),
        "count" => s.count.to_string(),
        "length" => s.length.to_string(),
        "segments" => s.segments.to_string(),
        "stiffness" => s.stiffness.to_string(),
        "pinTip" => (s.pin_tip as u8).to_string(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::commands::execute;

    fn at(app: &mut App, f: f32) {
        app.set_time(f);
        crate::rig::evaluate(&mut app.scene);
    }

    fn hair(app: &App, name: &str) -> Hair {
        match &app.scene.node(app.scene.find_by_name(name).unwrap()).unwrap().shape {
            Shape::Hair(h) => h.clone(),
            _ => panic!("not hair"),
        }
    }

    #[test]
    fn hair_grows_droops_follows_and_collides() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        execute(&mut app, "polySphere -r 1 -sa 24 -sh 16 -n head; move 0 2 0 head; hairSystem -count 200 -length 0.6 -seg 6 -stiff 0.05 -n hs head").unwrap();
        at(&mut app, 1.0);
        let h = hair(&app, "hs");
        assert_eq!(h.strands().len(), 200);
        // Rest: strands start on the sphere and point outward with their segment length kept.
        for st in h.strands() {
            assert!((st[0].distance(Vec3::new(0.0, 2.0, 0.0)) - 1.0).abs() < 0.03, "root on the surface");
            let len: f32 = st.windows(2).map(|w| w[0].distance(w[1])).sum();
            assert!((len - 0.6).abs() < 0.6 * 0.25, "length {len}");
        }
        // After a second, loose hair hangs down (tips lower than at rest) but never inside the head.
        let tip_y0: f32 = h.strands().iter().map(|s| s.last().unwrap().y).sum::<f32>() / 200.0;
        at(&mut app, 25.0);
        let h = hair(&app, "hs");
        let tip_y1: f32 = h.strands().iter().map(|s| s.last().unwrap().y).sum::<f32>() / 200.0;
        assert!(tip_y1 < tip_y0 - 0.1, "droops: {tip_y0} → {tip_y1}");
        let inside = h.strands().iter().flat_map(|s| s.iter().skip(1)).filter(|p| p.distance(Vec3::new(0.0, 2.0, 0.0)) < 0.97).count();
        assert_eq!(inside, 0, "no hair inside the head");
        for st in h.strands() {
            for w in st.windows(2) {
                assert!((w[0].distance(w[1]) - st[0].distance(st[1])).abs() < 0.02, "inextensible");
            }
        }
        // Moving the head: roots follow.
        execute(&mut app, "currentTime 1; setKeyframe -at tx -v 0 head; currentTime 25; setKeyframe -at tx -v 2 head").unwrap();
        at(&mut app, 25.0);
        let h = hair(&app, "hs");
        for st in h.strands() {
            assert!((st[0].distance(Vec3::new(2.0, 2.0, 0.0)) - 1.0).abs() < 0.03, "root follows the head");
        }
        // Stiff hair stays near its rest shape.
        execute(&mut app, "setAttr hs.stiffness 1; setAttr hs.tipStiffness 1").unwrap();
        at(&mut app, 25.0);
        let h = hair(&app, "hs");
        let ups = h.strands().iter().filter(|s| s[0].y > 2.5 && s.last().unwrap().y > s[0].y + 0.3).count();
        assert!(ups > 10, "stiff hair on top stands up: {ups}");
        assert_eq!(execute(&mut app, "getAttr hs.strands").unwrap(), "200");
        assert!(!hair_tris(&h).is_empty());
        // Convert to curves.
        execute(&mut app, "hairToCurves hs").unwrap();
        assert!(app.scene.find_by_name("hs_curve200").is_some());
    }

    #[test]
    fn dynamic_curve_swings_and_writes_output() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        // A horizontal rope pinned at its start falls and swings below it like a pendulum.
        execute(&mut app, "curve -d 3 -p 0 3 0 -p 1 3 0 -p 2 3 0 -p 3 3 0 -n rope; makeCurvesDynamic rope").unwrap();
        let out = app.scene.find_by_name("rope_dyn").expect("output curve");
        let pts_at = |app: &mut App, f: f32| {
            at(app, f);
            match &app.scene.node(out).unwrap().shape {
                Shape::Curve { points, .. } => points.clone(),
                _ => unreachable!(),
            }
        };
        let (mut min_y, mut min_x) = (f32::MAX, f32::MAX);
        for f in 1..=49 {
            let pts = pts_at(&mut app, f as f32);
            assert!(pts[0].distance(Vec3::new(0.0, 3.0, 0.0)) < 1e-3, "root pinned");
            let len: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
            assert!((len - 3.0).abs() < 0.1, "keeps its length: {len}");
            min_y = min_y.min(pts.last().unwrap().y);
            min_x = min_x.min(pts.last().unwrap().x);
        }
        assert!(min_y < 0.5, "falls the full length: {min_y}");
        assert!(min_x < -1.0, "swings past the pin: {min_x}");
        // Pin both ends: a sagging bridge.
        execute(&mut app, "setAttr rope_hairSystem.pinTip 1").unwrap();
        let pts = pts_at(&mut app, 49.0);
        assert!(pts.last().unwrap().distance(Vec3::new(3.0, 3.0, 0.0)) < 1e-3);
        let mid = pts[pts.len() / 2];
        assert!(mid.y < 2.99 && mid.y > 2.0, "sags a little: {mid}");
        // A surface built from the output curve follows it in the same evaluation.
        execute(&mut app, "circle -r 0.1 -nr 1 0 0 -n prof; extrude -n tube prof rope_dyn").unwrap();
        let tube = app.scene.find_by_name("tube").unwrap();
        let lowest = |app: &App| crate::rig::deformed_positions(&app.scene, tube).unwrap_or_else(|| {
            let w = app.scene.world_matrix(tube);
            app.scene.node(tube).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        }).iter().map(|p| p.y).fold(f32::MAX, f32::min);
        let mid_y = |app: &App| match &app.scene.node(out).unwrap().shape {
            Shape::Curve { points, .. } => points[points.len() / 2].y,
            _ => unreachable!(),
        };
        at(&mut app, 1.0);
        let (l1, m1) = (lowest(&app), mid_y(&app));
        at(&mut app, 30.0);
        let (l30, m30) = (lowest(&app), mid_y(&app));
        // No one-frame lag: evaluating the same frame again changes nothing.
        crate::rig::evaluate(&mut app.scene);
        assert!((lowest(&app) - l30).abs() < 1e-5, "surface updated in the same evaluation");
        assert!(l30 < l1 - 0.05 && m30 < m1 - 0.05, "tube sags with the rope: {l1} → {l30} (rope {m1} → {m30})");
        execute(&mut app, "delete tube prof").unwrap();
        // A wind field blows it sideways.
        execute(&mut app, "setAttr rope_hairSystem.pinTip 0; select -cl; airField -m 6 -dx 0 -dy 0 -dz 1 -s 3 -n gust").unwrap();
        let mean_z: f32 = (25..=49).map(|f| pts_at(&mut app, f as f32).last().unwrap().z).sum::<f32>() / 25.0;
        assert!(mean_z > 0.5, "wind pushes it +Z: {mean_z}");
    }

    #[test]
    fn simulations_export_to_gltf_and_usd() {
        let dir = std::env::temp_dir().join("rm_sim_export");
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "playbackOptions -min 1 -max 24; polySphere -r 0.5 -n head; move 0 2 0 head; hairSystem -count 30 -seg 4 -n fur head").unwrap();
        execute(&mut app, "polyPlane -w 1 -h 1 -sx 4 -sy 4 -n flag; rotate 90 0 0 flag; move 2 2 0 flag; nCloth flag; clothPin flag.vtx[20:24]").unwrap();
        execute(&mut app, "emitter -r 48 -spd 2 -lifespan 0.25 -n sparks; move -2 1 0 sparks; select -cl; turbulenceField -m 2 -n turb").unwrap();
        let glb = dir.join("sims.glb");
        let msg = execute(&mut app, &format!("file -export {}", glb.display())).unwrap();
        eprintln!("{msg}");
        let (doc, bin) = crate::export::gltf::parse_glb(&std::fs::read(&glb).unwrap());
        let nodes = doc["nodes"].as_array().unwrap();
        let meshes = doc["meshes"].as_array().unwrap();
        let mesh_of = |name: &str| -> &serde_json::Value {
            let n = nodes.iter().find(|n| n["name"] == name).unwrap_or_else(|| panic!("node {name}"));
            &meshes[n["mesh"].as_u64().unwrap() as usize]
        };
        // Cloth: 24 per-frame morph targets.
        assert_eq!(mesh_of("flag")["primitives"][0]["targets"].as_array().unwrap().len(), 24);
        // Hair: LINES with per-frame morph targets.
        let fur = mesh_of("fur");
        assert_eq!(fur["primitives"][0]["mode"], 1);
        assert_eq!(fur["primitives"][0]["targets"].as_array().unwrap().len(), 24);
        // Particles: slot nodes under the emitter with translation + STEP scale tracks.
        let sparks = nodes.iter().find(|n| n["name"] == "sparks").unwrap();
        let kids = sparks["children"].as_array().unwrap();
        assert!(kids.len() >= 10 && kids.len() < 30, "slots reused: {}", kids.len());
        let anim = &doc["animations"][0];
        let steps = anim["samplers"].as_array().unwrap().iter().filter(|s| s["interpolation"] == "STEP").count();
        assert_eq!(steps, kids.len());
        let weights = anim["channels"].as_array().unwrap().iter().filter(|c| c["target"]["path"] == "weights").count();
        assert_eq!(weights, 2, "cloth + hair morph weights");
        assert!(bin.len() > 10_000);
        // USD: hair as BasisCurves with time samples.
        let usda = dir.join("sims.usda");
        execute(&mut app, &format!("file -export {}", usda.display())).unwrap();
        let text = std::fs::read_to_string(&usda).unwrap();
        let fur = &text[text.find("def BasisCurves \"furShape\"").expect("hair curves")..];
        assert!(fur.contains("curveVertexCounts = [5, 5") && fur.contains("points.timeSamples") && fur.contains("interpolation = \"vertex\""));
    }

    /// `cargo test --release hair_perf -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn hair_perf() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -sa 32 -sh 24 -n head; move 0 2 0 head; hairSystem -count 1000 -seg 8 -length 0.5 -n hs head; select -cl; turbulenceField -m 3").unwrap();
        let t = std::time::Instant::now();
        at(&mut app, 120.0);
        eprintln!("hair 1000 strands x 8 segments, 120 frames: {:.2}s ({:.1} ms/frame)", t.elapsed().as_secs_f32(), t.elapsed().as_secs_f32() * 1000.0 / 120.0);
        let t = std::time::Instant::now();
        let n = hair_tris(&hair(&app, "hs")).len();
        eprintln!("tube triangles: {} verts in {:.1} ms", n, t.elapsed().as_secs_f32() * 1000.0);
    }
}
