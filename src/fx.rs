//! FX: particles (emitters with forces and collisions) and cloth (position-based dynamics).
//!
//! Simulations are deterministic and cached per frame from their start frame. A cache is
//! rebuilt when anything it depends on changes (settings, the animation of the emitter /
//! pins / colliders, collider meshes). Emitters, pins and colliders follow the fully evaluated
//! rig (keys, constraints, IK, skinning): a light copy of the scene is evaluated at every
//! simulated frame.
//!
//! Particles are a shape (`Shape::Particles`) drawn as small octahedra, so they show in the
//! viewport and the path tracer. Cloth is a deformer on a mesh (`Node::cloth`): the simulated
//! points replace the mesh's points, so the viewport, renderer and USD export use them.
//!
//! Fields (`Shape::Field`: air, gravity, uniform, radial, vortex, turbulence, drag, newton) are
//! scene nodes whose transform places and aims them; they push particles, cloth and hair
//! (`crate::hair`). A field with no targets affects every simulation.

use crate::scene::{Channel, NodeId, Scene, Shape};
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Longest simulation range (frames) computed in one go.
pub const MAX_FRAMES: usize = 20_000;

// ------------------------------------------------------------------------------ helpers

/// Small deterministic RNG (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03 | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in [-1, 1).
    pub fn s(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    pub fn unit(&mut self) -> Vec3 {
        let z = self.s();
        let a = self.f() * std::f32::consts::TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), z, r * a.sin())
    }
}

/// Random direction within `spread` degrees of `dir`.
fn cone(rng: &mut Rng, dir: Vec3, spread_deg: f32) -> Vec3 {
    let d = dir.try_normalize().unwrap_or(Vec3::Y);
    if spread_deg <= 0.0 {
        return d;
    }
    let cos_max = spread_deg.min(180.0).to_radians().cos();
    let z = 1.0 - rng.f() * (1.0 - cos_max);
    let a = rng.f() * std::f32::consts::TAU;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let (bx, by) = {
        let t = if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let bx = d.cross(t).normalize();
        (bx, d.cross(bx))
    };
    (bx * (r * a.cos()) + by * (r * a.sin()) + d * z).normalize()
}

/// Smooth pseudo-turbulence (a sum of sines; cheap and divergence-light).
pub fn turbulence(p: Vec3, t: f32, freq: f32) -> Vec3 {
    let q = p * freq;
    Vec3::new(
        (q.y * 1.3 + q.z * 0.7 + t * 0.9).sin() + 0.5 * (q.z * 2.1 - t * 0.5).cos(),
        (q.z * 1.1 + q.x * 0.9 + t * 1.1).sin() + 0.5 * (q.x * 1.9 + t * 0.3).cos(),
        (q.x * 1.2 + q.y * 0.8 - t * 0.7).sin() + 0.5 * (q.y * 2.3 - t * 0.6).cos(),
    )
}

pub(crate) fn hash_f(h: &mut impl std::hash::Hasher, v: f32) {
    h.write_u32(v.to_bits());
}

/// Hash of a node's motion inputs: anim curves, plus static values of unanimated channels.
pub(crate) fn motion_hash(scene: &Scene, id: NodeId, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(c) = cur {
        let Some(n) = scene.node(c) else { break };
        c.hash(h);
        for ch in [Channel::TranslateX, Channel::TranslateY, Channel::TranslateZ, Channel::RotateX, Channel::RotateY, Channel::RotateZ, Channel::ScaleX, Channel::ScaleY, Channel::ScaleZ] {
            match n.anim.get(&ch) {
                Some(curve) => format!("{curve:?}").hash(h),
                None => hash_f(h, n.get(ch)),
            }
        }
        for v in n.joint_orient.to_array() {
            hash_f(h, v);
        }
        for c in &n.constraints {
            format!("{c:?}").hash(h);
        }
        cur = n.parent;
        guard += 1;
        if guard > 1000 {
            break;
        }
    }
}

// ------------------------------------------------------------------------------ settings

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmitterKind {
    /// From the node's origin in all directions.
    Omni,
    /// From the node's origin along `dir` (node space) within `spread`.
    Directional,
    /// From random points of the source mesh's surface, along its normals.
    Surface,
    /// From random points along the source curve.
    Curve,
    /// From random points inside the node's unit cube (scale it to size the volume).
    Volume,
}

impl EmitterKind {
    pub fn parse(s: &str) -> Option<EmitterKind> {
        Some(match s.to_lowercase().as_str() {
            "omni" | "point" => EmitterKind::Omni,
            "dir" | "directional" => EmitterKind::Directional,
            "surface" | "surf" | "mesh" => EmitterKind::Surface,
            "curve" => EmitterKind::Curve,
            "volume" | "cube" | "box" => EmitterKind::Volume,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            EmitterKind::Omni => "omni",
            EmitterKind::Directional => "directional",
            EmitterKind::Surface => "surface",
            EmitterKind::Curve => "curve",
            EmitterKind::Volume => "volume",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticleSettings {
    pub kind: EmitterKind,
    /// Source mesh / curve for surface and curve emitters.
    pub source: Option<NodeId>,
    pub dir: Vec3,
    /// Cone half-angle (degrees) around the emission direction.
    pub spread: f32,
    /// Particles per second.
    pub rate: f32,
    pub speed: f32,
    /// ± fraction of speed.
    pub speed_random: f32,
    /// Seconds (0 = forever).
    pub lifespan: f32,
    pub lifespan_random: f32,
    pub gravity: Vec3,
    /// Air drag toward the wind velocity (1/s).
    pub drag: f32,
    pub wind: Vec3,
    pub turbulence: f32,
    pub turbulence_freq: f32,
    pub colliders: Vec<NodeId>,
    pub ground: bool,
    pub ground_height: f32,
    /// Normal velocity kept after a hit (0 = stick, 1 = perfect bounce).
    pub bounce: f32,
    /// Tangential velocity lost on a hit.
    pub friction: f32,
    pub radius: f32,
    pub color: [f32; 3],
    pub color_end: [f32; 3],
    pub max_count: u32,
    pub seed: u32,
    pub start: f32,
    /// Substeps per frame.
    pub substeps: u32,
}

impl Default for ParticleSettings {
    fn default() -> Self {
        ParticleSettings {
            kind: EmitterKind::Omni,
            source: None,
            dir: Vec3::Y,
            spread: 25.0,
            rate: 100.0,
            speed: 3.0,
            speed_random: 0.2,
            lifespan: 3.0,
            lifespan_random: 0.2,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            drag: 0.05,
            wind: Vec3::ZERO,
            turbulence: 0.0,
            turbulence_freq: 1.0,
            colliders: Vec::new(),
            ground: true,
            ground_height: 0.0,
            bounce: 0.4,
            friction: 0.2,
            radius: 0.03,
            color: [1.0, 0.75, 0.3],
            color_end: [0.9, 0.2, 0.1],
            max_count: 20_000,
            seed: 1,
            start: 1.0,
            substeps: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub p: Vec3,
    pub v: Vec3,
    pub age: f32,
    pub life: f32,
    pub id: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ParticleFrame {
    pub particles: Vec<Particle>,
    /// Fractional emission carried to the next frame.
    pub carry: f32,
    pub next_id: u32,
}

/// Cached simulation (shared between undo snapshots through `Arc`).
#[derive(Clone, Default)]
pub struct SimCache<F> {
    pub key: u64,
    pub frames: Vec<Arc<F>>,
    /// Frame index shown now (into `frames`), and a version for the viewport cache.
    pub current: Option<usize>,
    pub version: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Particles {
    pub settings: ParticleSettings,
    #[serde(skip)]
    pub cache: Arc<SimCache<ParticleFrame>>,
}

impl<F> std::fmt::Debug for SimCache<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SimCache({} frames, current {:?})", self.frames.len(), self.current)
    }
}

impl Particles {
    /// Particles at the current time.
    pub fn current(&self) -> &[Particle] {
        self.cache.current.and_then(|i| self.cache.frames.get(i)).map(|f| f.particles.as_slice()).unwrap_or(&[])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClothSettings {
    /// Edge stiffness (0..1, 1 = inextensible).
    pub stretch: f32,
    /// Bending stiffness (0..1).
    pub bend: f32,
    /// Velocity damping per second (0..1).
    pub damping: f32,
    pub gravity: Vec3,
    pub wind: Vec3,
    /// Aerodynamic drag (1/s): normal to the cloth, plus 5 % tangential.
    pub drag: f32,
    pub substeps: u32,
    pub iterations: u32,
    /// Collision distance kept from colliders.
    pub thickness: f32,
    pub friction: f32,
    pub colliders: Vec<NodeId>,
    pub ground: bool,
    pub ground_height: f32,
    /// Pinned vertices follow `pin_to` (or the cloth's own transform).
    pub pins: Vec<u32>,
    pub pin_to: Option<NodeId>,
    pub start: f32,
    /// Keep the cloth from passing through itself (vertex–vertex repulsion).
    pub self_collide: bool,
    /// Self-collision radius per vertex (0 = auto: 45 % of the average edge length).
    pub self_thickness: f32,
}

impl Default for ClothSettings {
    fn default() -> Self {
        ClothSettings {
            stretch: 1.0,
            bend: 0.1,
            damping: 0.02,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            wind: Vec3::ZERO,
            drag: 1.0,
            substeps: 8,
            iterations: 2,
            thickness: 0.02,
            friction: 0.3,
            colliders: Vec::new(),
            ground: true,
            ground_height: 0.0,
            pins: Vec::new(),
            pin_to: None,
            start: 1.0,
            self_collide: false,
            self_thickness: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClothFrame {
    pub pos: Vec<Vec3>,
    pub prev: Vec<Vec3>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Cloth {
    pub settings: ClothSettings,
    #[serde(skip)]
    pub cache: Arc<SimCache<ClothFrame>>,
}

impl Cloth {
    /// Simulated world positions at the current time.
    pub fn current(&self) -> Option<&[Vec3]> {
        self.cache.current.and_then(|i| self.cache.frames.get(i)).map(|f| f.pos.as_slice())
    }
}

// ------------------------------------------------------------------------------ attributes

pub(crate) fn v3(vals: &[f32]) -> Result<Vec3, String> {
    match vals {
        [x, y, z, ..] => Ok(Vec3::new(*x, *y, *z)),
        [v] => Ok(Vec3::splat(*v)),
        _ => Err("expected 3 values".into()),
    }
}

pub(crate) fn one(vals: &[f32]) -> Result<f32, String> {
    vals.first().copied().ok_or_else(|| "expected a value".to_string())
}

impl ParticleSettings {
    pub fn set_attr(&mut self, attr: &str, vals: &[f32]) -> Result<(), String> {
        match attr {
            "rate" => self.rate = one(vals)?.max(0.0),
            "speed" => self.speed = one(vals)?,
            "speedRandom" => self.speed_random = one(vals)?.clamp(0.0, 1.0),
            "spread" => self.spread = one(vals)?.clamp(0.0, 180.0),
            "direction" | "dir" => self.dir = v3(vals)?,
            "lifespan" => self.lifespan = one(vals)?.max(0.0),
            "lifespanRandom" => self.lifespan_random = one(vals)?.clamp(0.0, 1.0),
            "gravity" => self.gravity = if vals.len() >= 3 { v3(vals)? } else { Vec3::new(0.0, -one(vals)?, 0.0) },
            "drag" => self.drag = one(vals)?.max(0.0),
            "wind" => self.wind = v3(vals)?,
            "turbulence" => self.turbulence = one(vals)?.max(0.0),
            "turbulenceFrequency" => self.turbulence_freq = one(vals)?.max(0.0),
            "ground" => self.ground = one(vals)? > 0.5,
            "groundHeight" => self.ground_height = one(vals)?,
            "bounce" => self.bounce = one(vals)?.clamp(0.0, 1.0),
            "friction" => self.friction = one(vals)?.clamp(0.0, 1.0),
            "radius" => self.radius = one(vals)?.max(0.0005),
            "color" => self.color = v3(vals)?.to_array(),
            "colorEnd" => self.color_end = v3(vals)?.to_array(),
            "maxCount" => self.max_count = one(vals)?.max(0.0) as u32,
            "seed" => self.seed = one(vals)?.max(0.0) as u32,
            "startFrame" => self.start = one(vals)?,
            "substeps" => self.substeps = (one(vals)? as u32).clamp(1, 16),
            _ => return Err(format!("Unknown particle attribute '{attr}' (rate speed speedRandom spread direction lifespan lifespanRandom gravity drag wind turbulence turbulenceFrequency ground groundHeight bounce friction radius color colorEnd maxCount seed startFrame substeps)")),
        }
        Ok(())
    }
}

impl ClothSettings {
    pub fn set_attr(&mut self, attr: &str, vals: &[f32]) -> Result<(), String> {
        match attr {
            "stretch" | "stretchResistance" => self.stretch = one(vals)?.clamp(0.0, 1.0),
            "bend" | "bendResistance" => self.bend = one(vals)?.clamp(0.0, 1.0),
            "damping" | "damp" => self.damping = one(vals)?.clamp(0.0, 1.0),
            "gravity" => self.gravity = if vals.len() >= 3 { v3(vals)? } else { Vec3::new(0.0, -one(vals)?, 0.0) },
            "wind" => self.wind = v3(vals)?,
            "drag" => self.drag = one(vals)?.max(0.0),
            "substeps" => self.substeps = (one(vals)? as u32).clamp(1, 64),
            "iterations" => self.iterations = (one(vals)? as u32).clamp(1, 32),
            "thickness" => self.thickness = one(vals)?.max(0.0),
            "friction" => self.friction = one(vals)?.clamp(0.0, 1.0),
            "ground" => self.ground = one(vals)? > 0.5,
            "groundHeight" => self.ground_height = one(vals)?,
            "startFrame" => self.start = one(vals)?,
            "selfCollide" | "selfCollision" => self.self_collide = one(vals)? > 0.5,
            "selfThickness" | "selfCollideWidth" => self.self_thickness = one(vals)?.max(0.0),
            _ => return Err(format!("Unknown cloth attribute '{attr}' (stretch bend damping gravity wind drag substeps iterations thickness friction ground groundHeight startFrame selfCollide selfThickness)")),
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------ collisions

/// Triangles in a uniform grid (for particle segment hits and cloth closest points).
pub struct TriGrid {
    tris: Vec<[Vec3; 3]>,
    cell: f32,
    map: HashMap<[i32; 3], Vec<u32>>,
}

impl TriGrid {
    pub fn new(tris: Vec<[Vec3; 3]>) -> TriGrid {
        let mut avg = 0.0;
        for t in &tris {
            avg += t[0].distance(t[1]).max(t[1].distance(t[2])).max(t[2].distance(t[0]));
        }
        let cell = (avg / tris.len().max(1) as f32 * 1.5).max(1e-3);
        let mut g = TriGrid { tris, cell, map: HashMap::new() };
        for (i, t) in g.tris.iter().enumerate() {
            let lo = t[0].min(t[1]).min(t[2]);
            let hi = t[0].max(t[1]).max(t[2]);
            let (a, b) = (g.key(lo), g.key(hi));
            let n = (b[0] - a[0] + 1) as i64 * (b[1] - a[1] + 1) as i64 * (b[2] - a[2] + 1) as i64;
            if n > 4096 {
                continue; // degenerate giant triangle
            }
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        g.map.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        g
    }

    fn key(&self, p: Vec3) -> [i32; 3] {
        let q = (p / self.cell).floor();
        [q.x as i32, q.y as i32, q.z as i32]
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    fn candidates(&self, lo: Vec3, hi: Vec3, out: &mut Vec<u32>) {
        out.clear();
        let (a, b) = (self.key(lo), self.key(hi));
        if (b[0] - a[0] + 1) as i64 * (b[1] - a[1] + 1) as i64 * (b[2] - a[2] + 1) as i64 > 512 {
            out.extend(0..self.tris.len() as u32);
            return;
        }
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    if let Some(v) = self.map.get(&[x, y, z]) {
                        out.extend_from_slice(v);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// First hit of segment a→b: (t, point, normal facing a).
    pub fn segment_hit(&self, a: Vec3, b: Vec3, scratch: &mut Vec<u32>) -> Option<(f32, Vec3, Vec3)> {
        self.candidates(a.min(b), a.max(b), scratch);
        let d = b - a;
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for &i in scratch.iter() {
            let t = &self.tris[i as usize];
            let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
            let pv = d.cross(e2);
            let det = e1.dot(pv);
            if det.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / det;
            let tv = a - t[0];
            let u = tv.dot(pv) * inv;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let qv = tv.cross(e1);
            let v = d.dot(qv) * inv;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let s = e2.dot(qv) * inv;
            if (0.0..=1.0).contains(&s) && best.is_none_or(|b| s < b.0) {
                let mut n = e1.cross(e2).normalize_or_zero();
                if n.dot(d) > 0.0 {
                    n = -n;
                }
                best = Some((s, a + d * s, n));
            }
        }
        best
    }

    /// Closest surface point within `r` of p: (point, outward face normal).
    pub fn closest(&self, p: Vec3, r: f32, scratch: &mut Vec<u32>) -> Option<(Vec3, Vec3)> {
        self.candidates(p - Vec3::splat(r), p + Vec3::splat(r), scratch);
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for &i in scratch.iter() {
            let t = &self.tris[i as usize];
            let b = crate::deform::closest_bary(p, t[0], t[1], t[2]);
            let q = t[0] * b[0] + t[1] * b[1] + t[2] * b[2];
            let d = q.distance_squared(p);
            if d <= r * r && best.is_none_or(|x| d < x.0) {
                best = Some((d, q, (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero()));
            }
        }
        best.map(|b| (b.1, b.2))
    }
}

// ------------------------------------------------------------------------------ evaluation context

/// A light copy of the scene for evaluating transforms (and keeping only the meshes the
/// simulation reads) at past / future frames.
pub(crate) struct Tracks {
    pub(crate) sc: Scene,
}

impl Tracks {
    /// Copy the scene without the heavy data the simulation doesn't read (meshes, skin weights
    /// and blend shapes of other objects are set aside during the clone, then restored).
    pub(crate) fn new(scene: &mut Scene, keep_meshes: &[NodeId]) -> Tracks {
        let mut stash: Vec<(usize, crate::mesh::Mesh, Option<crate::rig::Skin>, Option<crate::rig::BlendShape>)> = Vec::new();
        for (i, n) in scene.nodes.iter_mut().enumerate() {
            if keep_meshes.contains(&n.id) {
                continue;
            }
            if let Shape::Poly { mesh, .. } = &mut n.shape {
                stash.push((i, std::mem::take(mesh), n.skin.take(), n.blend.take()));
            }
        }
        let mut sc = scene.clone();
        for (i, mesh, skin, blend) in stash {
            let n = &mut scene.nodes[i];
            if let Shape::Poly { mesh: m, .. } = &mut n.shape {
                *m = mesh;
            }
            n.skin = skin;
            n.blend = blend;
        }
        for n in &mut sc.nodes {
            n.surf = None;
            n.cloth = None;
            if matches!(n.shape, Shape::Particles(_) | Shape::Hair(_)) {
                n.shape = Shape::Locator;
            }
            if !keep_meshes.contains(&n.id) {
                if let Shape::Poly { history, .. } = &mut n.shape {
                    *history = None;
                }
                n.wires.clear();
                n.lattices.clear();
                n.wrap = None;
                n.clusters.clear();
            }
        }
        Tracks { sc }
    }

    pub(crate) fn goto(&mut self, frame: f32) {
        self.sc.time = frame;
        self.sc.evaluate();
        crate::rig::evaluate(&mut self.sc);
    }

    pub(crate) fn world(&self, id: NodeId) -> Mat4 {
        self.sc.world_matrix(id)
    }

    /// World positions of a mesh node (deformed when it has deformers).
    pub(crate) fn mesh_world(&self, id: NodeId) -> Option<(Vec<Vec3>, Vec<Vec<u32>>)> {
        let n = self.sc.node(id)?;
        let m = n.mesh()?;
        let pts = crate::rig::deformed_positions(&self.sc, id).unwrap_or_else(|| {
            let w = self.sc.world_matrix(id);
            m.positions.iter().map(|p| w.transform_point3(*p)).collect()
        });
        Some((pts, m.faces.clone()))
    }

    pub(crate) fn colliders(&self, ids: &[NodeId]) -> TriGrid {
        let mut tris = Vec::new();
        for id in ids {
            if let Some((p, faces)) = self.mesh_world(*id) {
                for f in &faces {
                    for i in 1..f.len().saturating_sub(1) {
                        tris.push([p[f[0] as usize], p[f[i] as usize], p[f[i + 1] as usize]]);
                    }
                }
            }
        }
        TriGrid::new(tris)
    }

    /// The fields acting on `target`, evaluated at the current frame.
    pub(crate) fn fields(&self, target: NodeId) -> FieldSet {
        FieldSet::eval(&self.sc, target)
    }
}

// ------------------------------------------------------------------------------ fields

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    /// Pushes toward a wind velocity (`direction` × magnitude m/s) at rate `speed`.
    Air,
    /// Constant acceleration along `direction` (default down).
    Gravity,
    /// Constant force along `direction`.
    Uniform,
    /// Away from the field's position (negative magnitude pulls in).
    Radial,
    /// Around the field's axis (`direction`).
    Vortex,
    /// Smooth noise; `frequency` sets its scale and `speed` how fast it changes.
    Turbulence,
    /// Slows things down (proportional to velocity).
    Drag,
    /// Attraction toward the field's position (use attenuation 2 for inverse-square).
    Newton,
}

impl FieldKind {
    pub const ALL: [FieldKind; 8] = [FieldKind::Air, FieldKind::Gravity, FieldKind::Uniform, FieldKind::Radial, FieldKind::Vortex, FieldKind::Turbulence, FieldKind::Drag, FieldKind::Newton];
    pub fn parse(s: &str) -> Option<FieldKind> {
        let s = s.to_lowercase();
        let s = s.trim_end_matches("field");
        Some(match s {
            "air" | "wind" => FieldKind::Air,
            "gravity" => FieldKind::Gravity,
            "uniform" | "push" => FieldKind::Uniform,
            "radial" => FieldKind::Radial,
            "vortex" | "swirl" => FieldKind::Vortex,
            "turbulence" | "noise" => FieldKind::Turbulence,
            "drag" => FieldKind::Drag,
            "newton" | "attract" | "attractor" => FieldKind::Newton,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            FieldKind::Air => "air",
            FieldKind::Gravity => "gravity",
            FieldKind::Uniform => "uniform",
            FieldKind::Radial => "radial",
            FieldKind::Vortex => "vortex",
            FieldKind::Turbulence => "turbulence",
            FieldKind::Drag => "drag",
            FieldKind::Newton => "newton",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Field {
    pub kind: FieldKind,
    pub magnitude: f32,
    /// Direction / axis in the field node's space.
    pub direction: Vec3,
    /// Falloff exponent: strength × (1 + distance)^-attenuation.
    pub attenuation: f32,
    /// Beyond this distance the field does nothing (0 = no limit).
    pub max_distance: f32,
    /// Turbulence spatial frequency.
    pub frequency: f32,
    /// Air: how fast things reach the wind speed (1/s). Turbulence: how fast the noise changes.
    pub speed: f32,
    /// Simulations this field acts on (empty = all of them).
    pub targets: Vec<NodeId>,
}

impl Default for Field {
    fn default() -> Self {
        Field::new(FieldKind::Air)
    }
}

impl Field {
    pub fn new(kind: FieldKind) -> Field {
        let (magnitude, direction, attenuation) = match kind {
            FieldKind::Air => (4.0, Vec3::X, 0.0),
            FieldKind::Gravity => (9.81, Vec3::NEG_Y, 0.0),
            FieldKind::Uniform => (3.0, Vec3::X, 0.0),
            FieldKind::Radial => (5.0, Vec3::Y, 1.0),
            FieldKind::Vortex => (5.0, Vec3::Y, 1.0),
            FieldKind::Turbulence => (5.0, Vec3::Y, 0.0),
            FieldKind::Drag => (0.5, Vec3::Y, 0.0),
            FieldKind::Newton => (5.0, Vec3::Y, 1.0),
        };
        Field { kind, magnitude, direction, attenuation, max_distance: 0.0, frequency: 1.0, speed: 1.0, targets: Vec::new() }
    }

    pub fn affects(&self, id: NodeId) -> bool {
        self.targets.is_empty() || self.targets.contains(&id)
    }

    pub fn set_attr(&mut self, attr: &str, vals: &[f32]) -> Result<(), String> {
        match attr {
            "magnitude" | "mag" | "m" => self.magnitude = one(vals)?,
            "direction" | "dir" | "axis" => self.direction = v3(vals)?,
            "attenuation" | "att" => self.attenuation = one(vals)?.max(0.0),
            "maxDistance" | "mxd" => self.max_distance = one(vals)?.max(0.0),
            "frequency" | "freq" | "f" => self.frequency = one(vals)?.max(0.0),
            "speed" | "phase" => self.speed = one(vals)?.max(0.0),
            _ => return Err(format!("Unknown field attribute '{attr}' (magnitude direction attenuation maxDistance frequency speed)")),
        }
        Ok(())
    }
}

/// One field evaluated in world space at a frame.
#[derive(Clone, Debug)]
pub struct FieldEval {
    pub f: Field,
    pub origin: Vec3,
    /// World direction / axis (normalised).
    pub dir: Vec3,
}

/// The fields acting on one simulation at one frame.
#[derive(Clone, Debug, Default)]
pub struct FieldSet(pub Vec<FieldEval>);

impl FieldSet {
    pub fn eval(scene: &Scene, target: NodeId) -> FieldSet {
        let mut out = Vec::new();
        for n in &scene.nodes {
            if let Shape::Field(f) = &n.shape
                && f.affects(target)
                && scene.is_visible(n.id)
            {
                let w = scene.world_matrix(n.id);
                out.push(FieldEval { f: f.clone(), origin: w.transform_point3(Vec3::ZERO), dir: w.transform_vector3(f.direction).normalize_or(Vec3::Y) });
            }
        }
        FieldSet(out)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Total acceleration at `p` moving with `v` at time `t` (seconds), for particles and hair.
    pub fn accel(&self, p: Vec3, v: Vec3, t: f32) -> Vec3 {
        let (a, _, air) = self.sample(p, v, t);
        a + air
    }

    /// (acceleration from non-air fields, air wind velocity, air pull toward that wind).
    /// Cloth uses the wind through its aerodynamic model instead of the air pull.
    pub fn sample(&self, p: Vec3, v: Vec3, t: f32) -> (Vec3, Vec3, Vec3) {
        let mut acc = Vec3::ZERO;
        let mut wind = Vec3::ZERO;
        let mut air = Vec3::ZERO;
        for e in &self.0 {
            let f = &e.f;
            let r = p - e.origin;
            let d = r.length();
            let mut k = f.magnitude;
            if f.max_distance > 0.0 {
                if d >= f.max_distance {
                    continue;
                }
                // Soft edge over the outer 10 %.
                k *= ((f.max_distance - d) / (0.1 * f.max_distance)).min(1.0);
            }
            if f.attenuation > 0.0 {
                k *= (1.0 + d).powf(-f.attenuation);
            }
            match f.kind {
                FieldKind::Air => {
                    let w = e.dir * k;
                    wind += w;
                    air += (w - v) * f.speed;
                }
                FieldKind::Gravity | FieldKind::Uniform => acc += e.dir * k,
                FieldKind::Radial => acc += r.normalize_or_zero() * k,
                FieldKind::Newton => acc -= r.normalize_or_zero() * k,
                FieldKind::Vortex => acc += e.dir.cross(r).normalize_or_zero() * k,
                FieldKind::Turbulence => acc += turbulence(p, t * f.speed, f.frequency) * k,
                FieldKind::Drag => acc -= v * k,
            }
        }
        (acc, wind, air)
    }
}

/// Fields acting on `target` (for cache keys).
pub(crate) fn hash_fields(scene: &Scene, target: NodeId, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    for n in &scene.nodes {
        if let Shape::Field(f) = &n.shape
            && f.affects(target)
        {
            format!("{f:?}").hash(h);
            scene.is_visible(n.id).hash(h);
            motion_hash(scene, n.id, h);
        }
    }
}

/// Wire icon of a field (local space line segments).
pub fn field_icon(f: &Field) -> Vec<(Vec3, Vec3)> {
    let mut l = Vec::new();
    let d = f.direction.normalize_or(Vec3::Y);
    let (bx, by) = {
        let t = if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let bx = d.cross(t).normalize();
        (bx, d.cross(bx))
    };
    let arrow = |l: &mut Vec<(Vec3, Vec3)>, o: Vec3, d: Vec3, len: f32| {
        let tip = o + d * len;
        l.push((o, tip));
        l.push((tip, tip - d * 0.15 + bx * 0.08));
        l.push((tip, tip - d * 0.15 - bx * 0.08));
    };
    let circle = |l: &mut Vec<(Vec3, Vec3)>, r: f32, h: f32| {
        for i in 0..16 {
            let a0 = i as f32 / 16.0 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / 16.0 * std::f32::consts::TAU;
            l.push((bx * a0.cos() * r + by * a0.sin() * r + d * h, bx * a1.cos() * r + by * a1.sin() * r + d * h));
        }
    };
    match f.kind {
        FieldKind::Air | FieldKind::Uniform => {
            for o in [-0.2, 0.0, 0.2] {
                arrow(&mut l, by * o - d * 0.3, d, 0.6);
            }
        }
        FieldKind::Gravity => {
            arrow(&mut l, -d * 0.3, d, 0.6);
            circle(&mut l, 0.25, -0.3);
        }
        FieldKind::Radial | FieldKind::Newton => {
            for v in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
                if f.kind == FieldKind::Radial {
                    arrow(&mut l, v * 0.1, v, 0.35);
                } else {
                    arrow(&mut l, v * 0.45, -v, 0.35);
                }
            }
        }
        FieldKind::Vortex => {
            circle(&mut l, 0.35, 0.0);
            circle(&mut l, 0.2, 0.15);
            l.push((-d * 0.3, d * 0.4));
        }
        FieldKind::Turbulence => {
            for k in 0..3 {
                let y = (k as f32 - 1.0) * 0.2;
                for i in 0..10 {
                    let x0 = -0.4 + i as f32 * 0.08;
                    let x1 = x0 + 0.08;
                    l.push((bx * x0 + by * y + d * ((x0 * 12.0).sin() * 0.06), bx * x1 + by * y + d * ((x1 * 12.0).sin() * 0.06)));
                }
            }
        }
        FieldKind::Drag => {
            circle(&mut l, 0.3, 0.0);
            arrow(&mut l, d * 0.5, -d, 0.45);
        }
    }
    if f.max_distance > 0.0 {
        // Max-distance ring (three great circles).
        let r = f.max_distance;
        for (a, b) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
            for i in 0..32 {
                let a0 = i as f32 / 32.0 * std::f32::consts::TAU;
                let a1 = (i + 1) as f32 / 32.0 * std::f32::consts::TAU;
                l.push(((a * a0.cos() + b * a0.sin()) * r, (a * a1.cos() + b * a1.sin()) * r));
            }
        }
    }
    l
}

// ------------------------------------------------------------------------------ particles

fn particle_key(scene: &Scene, id: NodeId, s: &ParticleSettings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{s:?}").hash(&mut h);
    scene.fps.to_bits().hash(&mut h);
    motion_hash(scene, id, &mut h);
    hash_fields(scene, id, &mut h);
    for c in s.source.iter().chain(&s.colliders) {
        motion_hash(scene, *c, &mut h);
        if let Some(n) = scene.node(*c) {
            n.mesh_rev().hash(&mut h);
            if let Shape::Curve { points, .. } = &n.shape {
                for p in points {
                    p.to_array().map(f32::to_bits).hash(&mut h);
                }
            }
            if let Some(sk) = &n.skin {
                for j in &sk.joints {
                    motion_hash(scene, *j, &mut h);
                }
            }
        }
    }
    h.finish()
}

/// Emission sampler for one frame.
enum Source {
    Point(Vec3, Vec3),
    Volume(Mat4),
    Surface { tris: Vec<[Vec3; 3]>, cum: Vec<f32> },
    Curve(crate::nurbs::Curve),
    None,
}

impl Source {
    fn build(tr: &Tracks, id: NodeId, s: &ParticleSettings) -> Source {
        let w = tr.world(id);
        match s.kind {
            EmitterKind::Omni => Source::Point(w.transform_point3(Vec3::ZERO), Vec3::ZERO),
            EmitterKind::Directional => Source::Point(w.transform_point3(Vec3::ZERO), w.transform_vector3(s.dir).normalize_or(Vec3::Y)),
            EmitterKind::Volume => Source::Volume(w),
            EmitterKind::Surface => match s.source.and_then(|m| tr.mesh_world(m)) {
                Some((p, faces)) => {
                    let mut tris = Vec::new();
                    let mut cum = Vec::new();
                    let mut acc = 0.0;
                    for f in &faces {
                        for i in 1..f.len().saturating_sub(1) {
                            let t = [p[f[0] as usize], p[f[i] as usize], p[f[i + 1] as usize]];
                            acc += (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5;
                            tris.push(t);
                            cum.push(acc);
                        }
                    }
                    if tris.is_empty() { Source::None } else { Source::Surface { tris, cum } }
                }
                None => Source::None,
            },
            EmitterKind::Curve => match s.source.and_then(|c| crate::nurbs::world_curve(&tr.sc, c)) {
                Some(c) if c.is_valid() => Source::Curve(c),
                _ => Source::None,
            },
        }
    }

    /// (position, base direction or zero for "random").
    fn sample(&self, rng: &mut Rng) -> Option<(Vec3, Vec3)> {
        match self {
            Source::Point(p, d) => Some((*p, *d)),
            Source::Volume(m) => Some((m.transform_point3(Vec3::new(rng.s(), rng.s(), rng.s()) * 0.5), Vec3::ZERO)),
            Source::Surface { tris, cum } => {
                let total = *cum.last()?;
                let x = rng.f() * total;
                let i = cum.partition_point(|c| *c < x).min(tris.len() - 1);
                let t = &tris[i];
                let (mut u, mut v) = (rng.f(), rng.f());
                if u + v > 1.0 {
                    u = 1.0 - u;
                    v = 1.0 - v;
                }
                let p = t[0] + (t[1] - t[0]) * u + (t[2] - t[0]) * v;
                Some((p, (t[1] - t[0]).cross(t[2] - t[0]).normalize_or(Vec3::Y)))
            }
            Source::Curve(c) => {
                let (a, b) = c.domain();
                Some((c.point(a + (b - a) * rng.f()), Vec3::ZERO))
            }
            Source::None => None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn step_particles(prev: &ParticleFrame, s: &ParticleSettings, tr: &Tracks, emitter: NodeId, frame_index: usize, fps: f32, grid: &TriGrid, fields: &FieldSet) -> ParticleFrame {
    let mut out = prev.clone();
    let sub = s.substeps.clamp(1, 16) as usize;
    let dt = 1.0 / fps.max(1.0) / sub as f32;
    let mut rng = Rng::new(((s.seed as u64) << 32) ^ frame_index as u64);
    let source = Source::build(tr, emitter, s);
    let t0 = frame_index as f32 / fps.max(1.0);
    let mut scratch = Vec::new();
    for k in 0..sub {
        // Emission.
        let want = s.rate * dt + out.carry;
        let mut n = want.floor() as usize;
        out.carry = want - n as f32;
        n = n.min((s.max_count as usize).saturating_sub(out.particles.len()));
        for _ in 0..n {
            let Some((p, d)) = source.sample(&mut rng) else { break };
            let dir = if d == Vec3::ZERO { rng.unit() } else { cone(&mut rng, d, s.spread) };
            let sp = s.speed * (1.0 + s.speed_random * rng.s());
            let life = if s.lifespan <= 0.0 { f32::INFINITY } else { (s.lifespan * (1.0 + s.lifespan_random * rng.s())).max(1e-3) };
            // Spread births over the substep so bursts don't stack.
            let age = rng.f() * dt;
            let v = dir * sp;
            out.particles.push(Particle { p: p + v * age, v, age, life, id: out.next_id });
            out.next_id = out.next_id.wrapping_add(1);
        }
        // Forces + integration + collisions.
        let t = t0 + k as f32 * dt;
        for q in &mut out.particles {
            let mut a = s.gravity + (s.wind - q.v) * s.drag;
            if s.turbulence > 0.0 {
                a += turbulence(q.p, t, s.turbulence_freq) * s.turbulence;
            }
            if !fields.is_empty() {
                a += fields.accel(q.p, q.v, t);
            }
            q.v += a * dt;
            let mut np = q.p + q.v * dt;
            if !grid.is_empty()
                && let Some((_, hit, n)) = grid.segment_hit(q.p, np, &mut scratch)
            {
                let vn = q.v.dot(n) * n;
                let vt = q.v - vn;
                q.v = vt * (1.0 - s.friction) - vn * s.bounce;
                np = hit + n * 1e-4;
            }
            if s.ground && np.y < s.ground_height + s.radius {
                np.y = s.ground_height + s.radius;
                if q.v.y < 0.0 {
                    q.v.y = -q.v.y * s.bounce;
                    q.v.x *= 1.0 - s.friction;
                    q.v.z *= 1.0 - s.friction;
                }
            }
            q.p = np;
            q.age += dt;
        }
        out.particles.retain(|q| q.age < q.life);
    }
    out
}

// ------------------------------------------------------------------------------ cloth

/// Distance constraints of a cloth mesh: (a, b, rest length, kind 0 = stretch, 1 = bend).
pub fn cloth_constraints(faces: &[Vec<u32>], pos: &[Vec3]) -> Vec<(u32, u32, f32, u8)> {
    use std::collections::{BTreeSet, HashSet};
    let n = pos.len();
    let mut edges: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut nbr: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    for f in faces {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            edges.insert((a.min(b), a.max(b)));
            nbr[a as usize].insert(b);
            nbr[b as usize].insert(a);
        }
        // Shear: quad diagonals.
        if f.len() == 4 {
            edges.insert((f[0].min(f[2]), f[0].max(f[2])));
            edges.insert((f[1].min(f[3]), f[1].max(f[3])));
        }
    }
    let mut out: Vec<(u32, u32, f32, u8)> = edges.iter().map(|&(a, b)| (a, b, pos[a as usize].distance(pos[b as usize]), 0)).collect();
    // Bend: vertices two edges apart (skip-one springs).
    let mut bend: BTreeSet<(u32, u32)> = BTreeSet::new();
    for v in 0..n as u32 {
        for &m in &nbr[v as usize] {
            for &w in &nbr[m as usize] {
                if w != v && !nbr[v as usize].contains(&w) && !edges.contains(&(v.min(w), v.max(w))) {
                    bend.insert((v.min(w), v.max(w)));
                }
            }
        }
    }
    out.extend(bend.into_iter().map(|(a, b)| (a, b, pos[a as usize].distance(pos[b as usize]), 1)));
    out
}

fn cloth_key(scene: &Scene, id: NodeId, s: &ClothSettings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{s:?}").hash(&mut h);
    scene.fps.to_bits().hash(&mut h);
    motion_hash(scene, id, &mut h);
    hash_fields(scene, id, &mut h);
    if let Some(n) = scene.node(id) {
        n.mesh_rev().hash(&mut h);
        if let Some(sk) = &n.skin {
            for j in &sk.joints {
                motion_hash(scene, *j, &mut h);
            }
        }
    }
    for c in s.colliders.iter().chain(&s.pin_to) {
        motion_hash(scene, *c, &mut h);
        if let Some(n) = scene.node(*c) {
            n.mesh_rev().hash(&mut h);
            if let Some(sk) = &n.skin {
                for j in &sk.joints {
                    motion_hash(scene, *j, &mut h);
                }
            }
        }
    }
    h.finish()
}

struct ClothSetup {
    cons: Vec<(u32, u32, f32, u8)>,
    /// Start positions (self-collision skips pairs that are this close at rest).
    rest: Vec<Vec3>,
    /// Self-collision radius (0 = off).
    self_r: f32,
    faces: Vec<Vec<u32>>,
    inv_mass: Vec<f32>,
    /// Pinned vertex → position in the pin node's space at the start frame.
    pin_local: Vec<(u32, Vec3)>,
    pin_node: NodeId,
}

fn cloth_setup(tr: &Tracks, id: NodeId, s: &ClothSettings, start_pos: &[Vec3], faces: &[Vec<u32>]) -> ClothSetup {
    let cons = cloth_constraints(faces, start_pos);
    let mut inv_mass = vec![1.0f32; start_pos.len()];
    let pin_node = s.pin_to.filter(|p| tr.sc.node(*p).is_some()).unwrap_or(id);
    let inv = tr.world(pin_node).inverse();
    let mut pin_local = Vec::new();
    for &v in &s.pins {
        if let Some(p) = start_pos.get(v as usize) {
            inv_mass[v as usize] = 0.0;
            pin_local.push((v, inv.transform_point3(*p)));
        }
    }
    let self_r = if s.self_collide {
        if s.self_thickness > 0.0 {
            s.self_thickness
        } else {
            let edges: Vec<f32> = cons.iter().filter(|c| c.3 == 0).map(|c| c.2).collect();
            0.45 * edges.iter().sum::<f32>() / edges.len().max(1) as f32
        }
    } else {
        0.0
    };
    ClothSetup { cons, rest: start_pos.to_vec(), self_r, faces: faces.to_vec(), inv_mass, pin_local, pin_node }
}

#[allow(clippy::too_many_arguments)]
fn step_cloth(prev: &ClothFrame, s: &ClothSettings, setup: &ClothSetup, tr: &Tracks, fps: f32, grid: &TriGrid, fields: &FieldSet, t0: f32) -> ClothFrame {
    let sub = s.substeps.clamp(1, 64) as usize;
    let iters = s.iterations.clamp(1, 32) as usize;
    let dt = 1.0 / fps.max(1.0) / sub as f32;
    let mut pos = prev.pos.clone();
    let mut old = prev.prev.clone();
    let pin_w = tr.world(setup.pin_node);
    let pin_targets: Vec<(u32, Vec3)> = setup.pin_local.iter().map(|(v, l)| (*v, pin_w.transform_point3(*l))).collect();
    // Pins move linearly from last frame's position to this frame's target over the substeps.
    let pin_start: Vec<Vec3> = pin_targets.iter().map(|(v, _)| prev.pos[*v as usize]).collect();
    let k_iter = |k: f32| 1.0 - (1.0 - k.clamp(0.0, 1.0)).powf(1.0 / iters as f32);
    let (ks, kb) = (k_iter(s.stretch), k_iter(s.bend));
    let keep = (1.0 - s.damping.clamp(0.0, 1.0)).powf(dt);
    let mut scratch = Vec::new();
    let r = s.thickness.max(1e-4);
    let mut nrm = vec![Vec3::ZERO; pos.len()];
    for step in 0..sub {
        let f = (step + 1) as f32 / sub as f32;
        // Vertex normals (area weighted) for the aerodynamic force.
        if s.drag > 0.0 {
            nrm.iter_mut().for_each(|n| *n = Vec3::ZERO);
            for face in &setup.faces {
                let p0 = pos[face[0] as usize];
                for k in 1..face.len().saturating_sub(1) {
                    let n = (pos[face[k] as usize] - p0).cross(pos[face[k + 1] as usize] - p0);
                    for &v in [face[0], face[k], face[k + 1]].iter() {
                        nrm[v as usize] += n;
                    }
                }
            }
            nrm.iter_mut().for_each(|n| *n = n.normalize_or_zero());
        }
        for i in 0..pos.len() {
            if setup.inv_mass[i] == 0.0 {
                continue;
            }
            let v = (pos[i] - old[i]) / dt * keep;
            // Aerodynamics: the relative wind pushes along the surface normal (sails, flags),
            // with a little tangential drag.
            // Air fields act through the aerodynamic term (not their direct pull).
            let (fa, fw, _) = if fields.is_empty() { (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO) } else { fields.sample(pos[i], v, t0 + step as f32 * dt) };
            let rel = s.wind + fw - v;
            let n = nrm[i];
            let a = s.gravity + fa + n * (rel.dot(n) * s.drag) + (rel - n * rel.dot(n)) * (s.drag * 0.05);
            old[i] = pos[i];
            pos[i] += v * dt + a * dt * dt;
        }
        for (k, (v, target)) in pin_targets.iter().enumerate() {
            old[*v as usize] = pos[*v as usize];
            pos[*v as usize] = pin_start[k].lerp(*target, f);
        }
        for _ in 0..iters {
            for &(a, b, rest, kind) in &setup.cons {
                let (a, b) = (a as usize, b as usize);
                let (wa, wb) = (setup.inv_mass[a], setup.inv_mass[b]);
                let w = wa + wb;
                if w == 0.0 {
                    continue;
                }
                let d = pos[b] - pos[a];
                let len = d.length();
                if len < 1e-9 {
                    continue;
                }
                let k = if kind == 0 { ks } else { kb };
                let c = d * ((len - rest) / len * k / w);
                pos[a] += c * wa;
                pos[b] -= c * wb;
            }
        }
        if setup.self_r > 0.0 {
            self_collide(&mut pos, setup);
        }
        // Collisions (after the constraints, so nothing ends up inside).
        for i in 0..pos.len() {
            if setup.inv_mass[i] == 0.0 {
                continue;
            }
            let mut hit = false;
            if !grid.is_empty()
                && let Some((q, n)) = grid.closest(pos[i], r * 4.0, &mut scratch)
            {
                let d = (pos[i] - q).dot(n);
                if d < r {
                    pos[i] += n * (r - d);
                    hit = true;
                }
            }
            if s.ground && pos[i].y < s.ground_height + r {
                pos[i].y = s.ground_height + r;
                hit = true;
            }
            if hit && s.friction > 0.0 {
                // Friction: pull the previous position along so the tangential velocity drops.
                let mv = pos[i] - old[i];
                old[i] += mv * s.friction;
            }
        }
    }
    ClothFrame { pos, prev: old }
}

/// Push apart vertices closer than twice the self-collision radius (pairs that were close at
/// rest — neighbours — are skipped). Uniform hash grid, two passes.
fn self_collide(pos: &mut [Vec3], setup: &ClothSetup) {
    let r = setup.self_r;
    let dia = 2.0 * r;
    let skip = dia * 1.25;
    let cell = dia;
    let key = |p: Vec3| {
        let q = (p / cell).floor();
        [q.x as i32, q.y as i32, q.z as i32]
    };
    for _ in 0..2 {
        let mut grid: HashMap<[i32; 3], Vec<u32>> = HashMap::with_capacity(pos.len());
        for (i, p) in pos.iter().enumerate() {
            grid.entry(key(*p)).or_default().push(i as u32);
        }
        for i in 0..pos.len() {
            let k = key(pos[i]);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(list) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else { continue };
                        for &j in list {
                            let j = j as usize;
                            if j <= i {
                                continue;
                            }
                            let d = pos[j] - pos[i];
                            let l2 = d.length_squared();
                            if l2 >= dia * dia || setup.rest[i].distance_squared(setup.rest[j]) < skip * skip {
                                continue;
                            }
                            let (wi, wj) = (setup.inv_mass[i], setup.inv_mass[j]);
                            let w = wi + wj;
                            if w == 0.0 {
                                continue;
                            }
                            let l = l2.sqrt();
                            let n = if l > 1e-9 { d / l } else { Vec3::Y };
                            let c = n * ((dia - l) / w);
                            pos[i] -= c * wi;
                            pos[j] += c * wj;
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------ update

/// Bring every simulation to the scene's current frame (called from `rig::evaluate`).
pub fn update(scene: &mut Scene) {
    let jobs: Vec<NodeId> = scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Particles(_)) || n.cloth.is_some()).map(|n| n.id).collect();
    if jobs.is_empty() {
        return;
    }
    let time = scene.time;
    let fps = scene.fps;
    for id in jobs {
        let n = scene.node(id).unwrap();
        if let Shape::Particles(ps) = &n.shape {
            let key = particle_key(scene, id, &ps.settings);
            let mut cache = (*ps.cache).clone();
            if cache.key != key || cache.frames.is_empty() {
                cache = SimCache { key, frames: vec![Arc::new(ParticleFrame::default())], current: None, version: cache.version.wrapping_add(1) };
            }
            let s = ps.settings.clone();
            // frames[k] = state k frames after the start frame (frames[0] = nothing emitted).
            let target = ((time - s.start).round().max(0.0) as usize).min(MAX_FRAMES);
            let have = cache.frames.len() - 1;
            if target > have {
                let mut keep = vec![id];
                keep.extend(s.source.iter().chain(&s.colliders));
                let mut tr = Tracks::new(scene, &keep);
                for k in have..target {
                    let f = s.start + k as f32;
                    tr.goto(f);
                    let grid = tr.colliders(&s.colliders);
                    let fields = tr.fields(id);
                    let next = step_particles(cache.frames.last().unwrap(), &s, &tr, id, k, fps, &grid, &fields);
                    cache.frames.push(Arc::new(next));
                }
            }
            let cur = if time < s.start { 0 } else { target.min(cache.frames.len() - 1) };
            if cache.current != Some(cur) {
                cache.current = Some(cur);
                cache.version = cache.version.wrapping_add(1);
            }
            if let Some(Shape::Particles(p)) = scene.node_mut(id).map(|n| &mut n.shape) {
                p.cache = Arc::new(cache);
            }
        }
        let n = scene.node(id).unwrap();
        if let Some(cl) = &n.cloth {
            let key = cloth_key(scene, id, &cl.settings);
            let s = cl.settings.clone();
            let mut cache = (*cl.cache).clone();
            let target = ((time - s.start).round().max(0.0) as usize).min(MAX_FRAMES);
            let stale = cache.key != key || cache.frames.is_empty();
            if stale || target + 1 > cache.frames.len() {
                let mut keep = vec![id];
                keep.extend(s.colliders.iter().chain(&s.pin_to));
                let mut tr = Tracks::new(scene, &keep);
                tr.goto(s.start);
                let Some((start_pos, faces)) = tr.mesh_world(id) else { continue };
                let setup = cloth_setup(&tr, id, &s, &start_pos, &faces);
                if stale {
                    cache = SimCache { key, frames: vec![Arc::new(ClothFrame { pos: start_pos.clone(), prev: start_pos.clone() })], current: None, version: cache.version.wrapping_add(1) };
                }
                for k in cache.frames.len()..=target {
                    tr.goto(s.start + k as f32);
                    let grid = tr.colliders(&s.colliders);
                    let fields = tr.fields(id);
                    let next = step_cloth(cache.frames.last().unwrap(), &s, &setup, &tr, fps, &grid, &fields, k as f32 / fps.max(1.0));
                    cache.frames.push(Arc::new(next));
                }
            }
            let cur = target.min(cache.frames.len() - 1);
            if cache.current != Some(cur) {
                cache.current = Some(cur);
                cache.version = cache.version.wrapping_add(1);
            }
            if let Some(c) = scene.node_mut(id).and_then(|n| n.cloth.as_mut()) {
                c.cache = Arc::new(cache);
            }
        }
    }
}

/// Octahedron triangles (world space) for drawing particles: (position, normal, colour).
pub fn particle_tris(ps: &Particles) -> Vec<(Vec3, Vec3, [f32; 3])> {
    let s = &ps.settings;
    let r = s.radius;
    let dirs = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    let faces: [(usize, usize, usize); 8] = [(0, 2, 4), (2, 1, 4), (1, 3, 4), (3, 0, 4), (2, 0, 5), (1, 2, 5), (3, 1, 5), (0, 3, 5)];
    let mut out = Vec::with_capacity(ps.current().len() * 24);
    for q in ps.current() {
        let k = if q.life.is_finite() { (q.age / q.life).clamp(0.0, 1.0) } else { 0.0 };
        let c = [s.color[0] + (s.color_end[0] - s.color[0]) * k, s.color[1] + (s.color_end[1] - s.color[1]) * k, s.color[2] + (s.color_end[2] - s.color[2]) * k];
        for &(a, b, cc) in &faces {
            let n = (dirs[a] + dirs[b] + dirs[cc]).normalize();
            for d in [dirs[a], dirs[b], dirs[cc]] {
                out.push((q.p + d * r, n, c));
            }
        }
    }
    out
}


// ------------------------------------------------------------------------------ app actions

use crate::app::App;

impl App {
    /// Create a particle system. Surface / curve emitters take their source mesh / curve.
    pub fn create_emitter(&mut self, mut settings: ParticleSettings, at: Option<Vec3>, name: Option<&str>) -> Result<NodeId, String> {
        if let Some(src) = settings.source {
            let n = self.scene.node(src).ok_or("emitter: no such source object")?;
            match (&n.shape, settings.kind) {
                (Shape::Poly { .. }, EmitterKind::Omni | EmitterKind::Surface) => settings.kind = EmitterKind::Surface,
                (Shape::Curve { .. }, EmitterKind::Omni | EmitterKind::Curve) => settings.kind = EmitterKind::Curve,
                (_, EmitterKind::Surface) => return Err("emitter: a surface emitter needs a polygon mesh".into()),
                (_, EmitterKind::Curve) => return Err("emitter: a curve emitter needs a curve".into()),
                _ => {}
            }
        } else if matches!(settings.kind, EmitterKind::Surface | EmitterKind::Curve) {
            return Err(format!("emitter: a {} emitter needs a source object", settings.kind.name()));
        }
        settings.start = self.scene.play_start;
        self.checkpoint();
        let id = self.scene.add_node("particle1", Shape::Particles(Particles { settings, cache: Default::default() }), None);
        self.apply_name(id, name);
        if let Some(p) = at
            && let Some(n) = self.scene.node_mut(id)
        {
            n.translate = p;
        }
        self.sel = vec![id];
        Ok(id)
    }

    /// Make meshes into cloth.
    pub fn create_cloth(&mut self, ids: &[NodeId], f: impl Fn(&mut ClothSettings)) -> Result<usize, String> {
        let meshes: Vec<NodeId> = ids.iter().copied().filter(|i| self.scene.node(*i).is_some_and(|n| n.mesh().is_some())).collect();
        if meshes.is_empty() {
            return Err("nCloth: select polygon meshes.".into());
        }
        self.checkpoint();
        let start = self.scene.play_start;
        for id in &meshes {
            let size = self.world_bounds(&[*id]).map(|(a, b)| (b - a).length()).unwrap_or(1.0);
            let mut s = ClothSettings { start, thickness: (size * 0.008).clamp(0.002, 0.2), ..Default::default() };
            f(&mut s);
            if let Some(n) = self.scene.node_mut(*id) {
                n.cloth = Some(Cloth { settings: s, cache: Default::default() });
            }
        }
        Ok(meshes.len())
    }

    pub fn remove_cloth(&mut self, ids: &[NodeId]) -> usize {
        self.checkpoint();
        let mut n = 0;
        for id in ids {
            if let Some(x) = self.scene.node_mut(*id)
                && x.cloth.take().is_some()
            {
                n += 1;
            }
        }
        n
    }

    /// Pin cloth vertices (to `to`, or to the cloth's own transform). Replaces the pin set when
    /// `replace`.
    pub fn pin_cloth(&mut self, id: NodeId, verts: &[u32], to: Option<NodeId>, replace: bool) -> Result<usize, String> {
        let nv = self.scene.node(id).and_then(|n| n.mesh()).map(|m| m.positions.len()).ok_or("clothPin: not a mesh")?;
        if self.scene.node(id).and_then(|n| n.cloth.as_ref()).is_none() {
            return Err("clothPin: make the mesh nCloth first.".into());
        }
        self.checkpoint();
        let c = self.scene.node_mut(id).unwrap().cloth.as_mut().unwrap();
        if replace {
            c.settings.pins.clear();
        }
        for v in verts.iter().copied().filter(|v| (*v as usize) < nv) {
            if !c.settings.pins.contains(&v) {
                c.settings.pins.push(v);
            }
        }
        if to.is_some() {
            c.settings.pin_to = to;
        }
        Ok(c.settings.pins.len())
    }

    /// Create a field. When dynamics objects are selected it acts on them only; otherwise on all.
    pub fn create_field(&mut self, mut f: Field, at: Option<Vec3>, name: Option<&str>, targets: Vec<NodeId>) -> Result<NodeId, String> {
        f.targets = targets;
        self.checkpoint();
        let id = self.scene.add_node(&format!("{}Field1", f.kind.name()), Shape::Field(f), None);
        self.apply_name(id, name);
        if let Some(p) = at
            && let Some(n) = self.scene.node_mut(id)
        {
            n.translate = p;
        }
        self.sel = vec![id];
        Ok(id)
    }

    /// Restrict a field to a simulation (or remove it from the field's targets).
    pub fn connect_field(&mut self, field: NodeId, target: NodeId, remove: bool) -> Result<(), String> {
        let ok = self.scene.node(target).is_some_and(|n| matches!(n.shape, Shape::Particles(_) | Shape::Hair(_)) || n.cloth.is_some());
        if !ok {
            return Err("connectDynamic -f: the target must be a particle system, an nCloth mesh or a hair system.".into());
        }
        if !matches!(self.scene.node(field).map(|n| &n.shape), Some(Shape::Field(_))) {
            return Err("connectDynamic -f: not a field.".into());
        }
        self.checkpoint();
        if let Some(Shape::Field(f)) = self.scene.node_mut(field).map(|n| &mut n.shape) {
            f.targets.retain(|t| *t != target);
            if !remove {
                f.targets.push(target);
            }
        }
        Ok(())
    }

    /// Dynamics objects (particles, cloth, hair) among the selection.
    pub fn selected_dynamics(&self) -> Vec<NodeId> {
        self.sel.iter().copied().filter(|i| self.scene.node(*i).is_some_and(|n| matches!(n.shape, Shape::Particles(_) | Shape::Hair(_)) || n.cloth.is_some())).collect()
    }

    /// Add (or remove) a passive collider for particles / cloth.
    pub fn connect_collider(&mut self, collider: NodeId, target: NodeId, remove: bool) -> Result<(), String> {
        if !remove && self.scene.node(collider).and_then(|n| n.mesh()).is_none() {
            return Err("connectDynamic: the collider must be a polygon mesh.".into());
        }
        if collider == target {
            return Err("connectDynamic: an object can't collide with itself.".into());
        }
        self.checkpoint();
        let n = self.scene.node_mut(target).ok_or("connectDynamic: no such target")?;
        let list = match (&mut n.shape, &mut n.cloth) {
            (Shape::Particles(p), _) => &mut p.settings.colliders,
            (Shape::Hair(h), _) => &mut h.settings.colliders,
            (_, Some(c)) => &mut c.settings.colliders,
            _ => return Err("connectDynamic: the target must be a particle system, an nCloth mesh or a hair system.".into()),
        };
        list.retain(|c| *c != collider);
        if !remove {
            list.push(collider);
        }
        Ok(())
    }
}

/// `setAttr` on particle systems and cloth (`p.rate 100`, `flag.stretch 0.9`). None when the
/// node / attribute isn't an FX one.
pub fn set_attr(app: &mut App, spec: &str, vals: &[f32]) -> Option<Result<(), String>> {
    let (node, attr) = spec.split_once('.')?;
    if Channel::parse(attr).is_some() {
        return None;
    }
    let id = app.scene.find_by_name(node)?;
    let n = app.scene.node(id)?;
    if !matches!(n.shape, Shape::Particles(_) | Shape::Field(_) | Shape::Hair(_)) && n.cloth.is_none() {
        return None;
    }
    // Validate on a copy first so a bad attribute doesn't create an undo step.
    let r = match (&n.shape, &n.cloth) {
        (Shape::Particles(p), _) => p.settings.clone().set_attr(attr, vals),
        (Shape::Field(f), _) => f.clone().set_attr(attr, vals),
        (Shape::Hair(h), _) => h.settings.clone().set_attr(attr, vals),
        (_, Some(c)) => c.settings.clone().set_attr(attr, vals),
        _ => return None,
    };
    if let Err(e) = r {
        return Some(Err(e));
    }
    app.checkpoint();
    let n = app.scene.node_mut(id).unwrap();
    Some(match (&mut n.shape, &mut n.cloth) {
        (Shape::Particles(p), _) => p.settings.set_attr(attr, vals),
        (Shape::Field(f), _) => f.set_attr(attr, vals),
        (Shape::Hair(h), _) => h.settings.set_attr(attr, vals),
        (_, Some(c)) => c.settings.set_attr(attr, vals),
        _ => Ok(()),
    })
}

/// `getAttr p.count` (live particles) and settings values.
pub fn get_attr(app: &App, spec: &str) -> Option<String> {
    let (node, attr) = spec.split_once('.')?;
    let n = app.scene.node(app.scene.find_by_name(node)?)?;
    match &n.shape {
        Shape::Particles(p) => Some(match attr {
            "count" => p.current().len().to_string(),
            "rate" => p.settings.rate.to_string(),
            "speed" => p.settings.speed.to_string(),
            "lifespan" => p.settings.lifespan.to_string(),
            _ => return None,
        }),
        Shape::Field(f) => Some(match attr {
            "magnitude" | "mag" => f.magnitude.to_string(),
            "attenuation" => f.attenuation.to_string(),
            "maxDistance" => f.max_distance.to_string(),
            "type" => f.kind.name().to_string(),
            _ => return None,
        }),
        Shape::Hair(h) => crate::hair::get_attr(h, attr),
        _ => n.cloth.as_ref().and_then(|c| match attr {
            "stretch" => Some(c.settings.stretch.to_string()),
            "bend" => Some(c.settings.bend.to_string()),
            "pins" => Some(c.settings.pins.len().to_string()),
            "selfCollide" => Some((c.settings.self_collide as u8).to_string()),
            _ => None,
        }),
    }
}

fn take_val(t: &mut Vec<String>, flags: &[&str]) -> Option<String> {
    let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
    let v = t.get(i + 1).cloned()?;
    t.drain(i..i + 2);
    Some(v)
}

fn take_vec(t: &mut Vec<String>, flags: &[&str]) -> Option<Vec3> {
    let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
    let n: Vec<f32> = t.iter().skip(i + 1).take(3).filter_map(|x| x.parse().ok()).collect();
    if n.len() < 3 {
        return None;
    }
    t.drain(i..i + 4);
    Some(Vec3::new(n[0], n[1], n[2]))
}

fn take_flag(t: &mut Vec<String>, flags: &[&str]) -> bool {
    match t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}"))) {
        Some(i) => {
            t.remove(i);
            true
        }
        None => false,
    }
}

fn numv(v: Option<String>) -> Option<f32> {
    v.and_then(|s| s.parse().ok())
}

/// FX MEL commands. None = not one of these.
pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let mut t: Vec<String> = rest.to_vec();
    let name_of = |app: &App, id: NodeId| app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default();
    let r = match cmd.as_str() {
        "emitter" | "particleEmitter" => (|| {
            let mut s = ParticleSettings::default();
            if let Some(k) = take_val(&mut t, &["type", "typ"]) {
                s.kind = EmitterKind::parse(&k).ok_or(format!("emitter: unknown type '{k}' (omni dir surface curve volume)"))?;
            }
            if let Some(d) = take_vec(&mut t, &["dir", "direction", "d"]) {
                s.dir = d;
                if s.kind == EmitterKind::Omni {
                    s.kind = EmitterKind::Directional;
                }
            }
            let at = take_vec(&mut t, &["p", "pos", "position"]);
            if let Some(v) = numv(take_val(&mut t, &["r", "rate"])) {
                s.rate = v.max(0.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["spd", "speed"])) {
                s.speed = v;
            }
            if let Some(v) = numv(take_val(&mut t, &["srn", "speedRandom"])) {
                s.speed_random = v.clamp(0.0, 1.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["sp", "spread"])) {
                s.spread = v.clamp(0.0, 180.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["lifespan", "ls"])) {
                s.lifespan = v.max(0.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["lsr", "lifespanRandom"])) {
                s.lifespan_random = v.clamp(0.0, 1.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["rad", "radius"])) {
                s.radius = v.max(0.0005);
            }
            if let Some(c) = take_vec(&mut t, &["rgb", "color"]) {
                s.color = c.to_array();
                s.color_end = c.to_array();
            }
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            if !objs.is_empty() {
                s.source = Some(*crate::commands::resolve(app, &objs)?.first().unwrap());
            }
            let id = app.create_emitter(s, at, name.as_deref())?;
            Ok(name_of(app, id))
        })(),
        "nCloth" | "createNCloth" => (|| {
            let stretch = numv(take_val(&mut t, &["stretch", "s"]));
            let bend = numv(take_val(&mut t, &["bend", "b"]));
            let thick = numv(take_val(&mut t, &["thickness", "th"]));
            let self_col = take_flag(&mut t, &["selfCollide", "sc"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let n = app.create_cloth(&ids, |s| {
                if let Some(v) = stretch {
                    s.stretch = v.clamp(0.0, 1.0);
                }
                if let Some(v) = bend {
                    s.bend = v.clamp(0.0, 1.0);
                }
                if let Some(v) = thick {
                    s.thickness = v.max(0.0);
                }
                if self_col {
                    s.self_collide = true;
                }
            })?;
            Ok(format!("{n} nCloth"))
        })(),
        "removeNCloth" => (|| {
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            Ok(format!("{} removed", app.remove_cloth(&ids)))
        })(),
        "clothPin" => (|| {
            // clothPin [-to node] [-clear] mesh.vtx[0:10] ...   (or the selected vertices)
            let to = take_val(&mut t, &["to", "t"]);
            let clear = take_flag(&mut t, &["clear", "c"]);
            let to = match to {
                Some(n) => Some(app.scene.find_by_name(&n).ok_or_else(|| format!("No object matches name: {n}"))?),
                None => None,
            };
            let specs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            if clear {
                let ids = crate::commands::targets(app, &specs)?;
                for id in ids {
                    app.pin_cloth(id, &[], None, true)?;
                }
                return Ok("pins cleared".into());
            }
            if specs.iter().any(|x| x.contains('[')) {
                crate::commands::execute(app, &format!("select {}", specs.join(" ")))?;
            }
            let mut per: HashMap<NodeId, Vec<u32>> = HashMap::new();
            for (id, vs) in app.comp_vertices() {
                per.entry(id).or_default().extend(vs);
            }
            if per.is_empty() {
                return Err("clothPin: select cloth vertices (mesh.vtx[...]).".into());
            }
            let mut total = 0;
            for (id, vs) in per {
                total = app.pin_cloth(id, &vs, to, false)?;
            }
            Ok(format!("{total} pinned"))
        })(),
        "field" | "airField" | "gravityField" | "uniformField" | "radialField" | "vortexField" | "turbulenceField" | "dragField" | "newtonField" => (|| {
            let kind = if cmd == "field" {
                let k = take_val(&mut t, &["type", "typ", "t"]).unwrap_or_else(|| "air".into());
                FieldKind::parse(&k).ok_or(format!("field: unknown type '{k}' (air gravity uniform radial vortex turbulence drag newton)"))?
            } else {
                FieldKind::parse(cmd).unwrap()
            };
            let mut f = Field::new(kind);
            if let Some(v) = numv(take_val(&mut t, &["m", "magnitude", "mag"])) {
                f.magnitude = v;
            }
            if let Some(v) = numv(take_val(&mut t, &["att", "attenuation"])) {
                f.attenuation = v.max(0.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["mxd", "maxDistance"])) {
                f.max_distance = v.max(0.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["f", "frequency", "freq"])) {
                f.frequency = v.max(0.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["s", "speed"])) {
                f.speed = v.max(0.0);
            }
            if let Some(d) = take_vec(&mut t, &["dir", "direction", "axis"]) {
                f.direction = d;
            }
            let (dx, dy, dz) = (numv(take_val(&mut t, &["dx"])), numv(take_val(&mut t, &["dy"])), numv(take_val(&mut t, &["dz"])));
            if dx.is_some() || dy.is_some() || dz.is_some() {
                f.direction = Vec3::new(dx.unwrap_or(0.0), dy.unwrap_or(0.0), dz.unwrap_or(0.0));
            }
            let at = take_vec(&mut t, &["p", "pos", "position"]);
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let targets = if objs.is_empty() { app.selected_dynamics() } else { crate::commands::resolve(app, &objs)? };
            let id = app.create_field(f, at, name.as_deref(), targets)?;
            Ok(name_of(app, id))
        })(),
        "hairSystem" | "createHair" => (|| {
            let mut s = crate::hair::HairSettings { length: 0.0, width: 0.0, thickness: 0.0, ..Default::default() };
            if let Some(v) = numv(take_val(&mut t, &["count", "c"])) {
                s.count = (v as u32).clamp(1, 200_000);
            }
            if let Some(v) = numv(take_val(&mut t, &["length", "len", "l"])) {
                s.length = v.max(1e-4);
            }
            if let Some(v) = numv(take_val(&mut t, &["lr", "lengthRandom"])) {
                s.length_random = v.clamp(0.0, 1.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["seg", "segments"])) {
                s.segments = (v as u32).clamp(1, 64);
            }
            if let Some(v) = numv(take_val(&mut t, &["curl"])) {
                s.curl = v.clamp(0.0, 720.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["tilt"])) {
                s.tilt = v.clamp(0.0, 90.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["stiff", "stiffness"])) {
                s.stiffness = v.clamp(0.0, 1.0);
            }
            if let Some(v) = numv(take_val(&mut t, &["w", "width"])) {
                s.width = v.max(1e-5);
                s.tip_width = s.width * 0.25;
            }
            if let Some(c) = take_vec(&mut t, &["rgb", "color"]) {
                s.color = c.to_array();
                s.tip_color = (c * 1.4).min(Vec3::ONE).to_array();
            }
            if let Some(c) = take_vec(&mut t, &["tip", "tipColor"]) {
                s.tip_color = c.to_array();
            }
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let mesh = crate::commands::targets(app, &objs)?.into_iter().find(|i| app.scene.node(*i).is_some_and(|n| n.mesh().is_some())).ok_or("hairSystem: select a polygon mesh")?;
            let id = app.create_hair(mesh, s, name.as_deref())?;
            Ok(name_of(app, id))
        })(),
        "makeCurvesDynamic" | "makeDynamic" => (|| {
            let pin_tip = take_flag(&mut t, &["pinTip", "pt", "both"]);
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let mut outs = Vec::new();
            for id in ids {
                let (_, out) = app.make_curve_dynamic(id, pin_tip, name.as_deref())?;
                outs.push(name_of(app, out));
            }
            if outs.is_empty() {
                return Err("makeCurvesDynamic: select curves".into());
            }
            Ok(outs.join(" "))
        })(),
        "hairToCurves" => (|| {
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let id = crate::commands::targets(app, &objs)?.first().copied().ok_or("hairToCurves hairSystem")?;
            let g = app.hair_to_curves(id)?;
            Ok(name_of(app, g))
        })(),
        "connectDynamic" => (|| {
            // connectDynamic -c collider target | -d collider target | -f field target | -rf field target
            if let Some(fname) = take_val(&mut t, &["f", "fields"]).map(|f| (f, false)).or_else(|| take_val(&mut t, &["rf", "removeField"]).map(|f| (f, true))) {
                let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
                let fid = app.scene.find_by_name(&fname.0).ok_or_else(|| format!("No object matches name: {}", fname.0))?;
                for target in crate::commands::targets(app, &objs)? {
                    app.connect_field(fid, target, fname.1)?;
                }
                return Ok(String::new());
            }
            let col = take_val(&mut t, &["c", "collisions"]);
            let rm = take_val(&mut t, &["d", "delete"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let target = crate::commands::targets(app, &objs)?.first().copied().ok_or("connectDynamic -c collider target")?;
            let (c, remove) = match (col, rm) {
                (Some(c), _) => (c, false),
                (_, Some(c)) => (c, true),
                _ => return Err("connectDynamic -c collider target".into()),
            };
            let cid = app.scene.find_by_name(&c).ok_or_else(|| format!("No object matches name: {c}"))?;
            app.connect_collider(cid, target, remove)?;
            Ok(String::new())
        })(),
        _ => return None,
    };
    Some(r)
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

    fn wpts(app: &App, id: NodeId) -> Vec<Vec3> {
        crate::rig::deformed_positions(&app.scene, id).unwrap_or_else(|| {
            let w = app.scene.world_matrix(id);
            app.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        })
    }

    fn parts(app: &App, name: &str) -> Vec<Particle> {
        match &app.scene.node(app.scene.find_by_name(name).unwrap()).unwrap().shape {
            Shape::Particles(p) => p.current().to_vec(),
            _ => panic!("not particles"),
        }
    }

    #[test]
    fn particles_emit_fall_bounce_and_are_deterministic() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        execute(&mut app, "emitter -type omni -r 48 -spd 0 -lifespan 0 -n fountain; move 0 2 0 fountain; setAttr fountain.drag 0; setAttr fountain.bounce 0.5").unwrap();
        at(&mut app, 1.0);
        assert!(parts(&app, "fountain").len() <= 2, "start frame");
        at(&mut app, 25.0);
        let p = parts(&app, "fountain");
        assert!((p.len() as i32 - 48).abs() <= 2, "rate 48/s for 1 s: {}", p.len());
        // The oldest particle fell under gravity: y ≈ 2 - g t²/2, and hit the ground (t ≈ 0.64 s).
        let oldest = p.iter().min_by_key(|q| q.id).unwrap();
        assert!(oldest.p.y < 2.0 && oldest.p.y >= 0.0);
        let young = p.iter().max_by_key(|q| q.id).unwrap();
        let fall = 2.0 - young.p.y;
        assert!((fall - 0.5 * 9.81 * young.age * young.age).abs() < 0.05, "ballistic: fell {fall} in {}s", young.age);
        // Scrubbing back and forward is exact.
        at(&mut app, 10.0);
        let mid = parts(&app, "fountain");
        at(&mut app, 30.0);
        at(&mut app, 10.0);
        assert_eq!(mid, parts(&app, "fountain"));
        // Changing a setting re-simulates.
        execute(&mut app, "setAttr fountain.rate 96").unwrap();
        at(&mut app, 25.0);
        assert!((parts(&app, "fountain").len() as i32 - 96).abs() <= 3);
    }

    #[test]
    fn surface_emitter_and_mesh_collider() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -n src; move 0 3 0 src; polyPlane -w 6 -h 6 -n floor; move 0 1 0 floor").unwrap();
        execute(&mut app, "emitter -r 200 -spd 0 -lifespan 0 -n rain src; setAttr rain.ground 0; connectDynamic -c floor rain").unwrap();
        let id = app.scene.find_by_name("rain").unwrap();
        // Born on the sphere surface.
        at(&mut app, 2.0);
        for q in parts(&app, "rain") {
            assert!(((q.p - Vec3::new(0.0, 3.0, 0.0)).length() - 1.0).abs() < 0.15, "on the surface: {}", q.p);
        }
        // After 2 s nothing has fallen through the raised floor (y = 1).
        at(&mut app, 49.0);
        let p = parts(&app, "rain");
        assert!(!p.is_empty());
        let below: Vec<_> = p.iter().filter(|q| q.p.y < 0.99 && q.p.x.abs() < 2.9 && q.p.z.abs() < 2.9).collect();
        assert!(below.is_empty(), "{} fell through", below.len());
        assert!(p.iter().any(|q| (q.p.y - 1.0).abs() < 0.05), "some rest on the floor");
        assert!(matches!(&app.scene.node(id).unwrap().shape, Shape::Particles(ps) if ps.settings.colliders.len() == 1));
    }

    #[test]
    fn cloth_hangs_from_pins_and_drapes_over_a_sphere() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        // A flag pinned along its top edge hangs down.
        execute(&mut app, "polyPlane -w 2 -h 2 -sx 10 -sy 10 -n flag; rotate 90 0 0 flag; move 0 3 0 flag; nCloth flag").unwrap();
        let fid = app.scene.find_by_name("flag").unwrap();
        let rest = wpts(&app, fid);
        let top_y = rest.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let top: Vec<u32> = rest.iter().enumerate().filter(|(_, p)| (p.y - top_y).abs() < 1e-4).map(|(i, _)| i as u32).collect();
        assert_eq!(top.len(), 11);
        let spec: Vec<String> = top.iter().map(|v| format!("flag.vtx[{v}]")).collect();
        execute(&mut app, &format!("clothPin {}", spec.join(" "))).unwrap();
        // Wind pushes it sideways.
        execute(&mut app, "setAttr flag.wind 0 0 4").unwrap();
        at(&mut app, 48.0);
        let p = wpts(&app, fid);
        for v in &top {
            assert!(p[*v as usize].distance(rest[*v as usize]) < 1e-4, "pinned");
        }
        // Hangs: edges keep their length (inextensible), bottom is below the top.
        let m = app.scene.node(fid).unwrap().mesh().unwrap().clone();
        for (a, b) in m.edges() {
            let (r, c) = (rest[a as usize].distance(rest[b as usize]), p[a as usize].distance(p[b as usize]));
            assert!((c - r).abs() < r * 0.12, "stretch {r} → {c}");
        }
        let lowest = p.iter().map(|q| q.y).fold(f32::MAX, f32::min);
        assert!(lowest < top_y - 1.5, "{lowest}");
        assert!(p.iter().map(|q| q.z).sum::<f32>() / p.len() as f32 > 0.05, "wind blew it +Z");
        // A sheet dropped on a sphere drapes over it without passing through.
        execute(&mut app, "polySphere -r 1 -sa 24 -sh 16 -n ball; polyPlane -w 3 -h 3 -sx 20 -sy 20 -n sheet; move 0 1.6 0 sheet; nCloth sheet; connectDynamic -c ball sheet").unwrap();
        let sid = app.scene.find_by_name("sheet").unwrap();
        at(&mut app, 60.0);
        let p = wpts(&app, sid);
        let inside = p.iter().filter(|q| q.length() < 0.97).count();
        assert_eq!(inside, 0, "nothing inside the sphere");
        let centre = p.iter().min_by(|a, b| (a.x * a.x + a.z * a.z).total_cmp(&(b.x * b.x + b.z * b.z))).unwrap();
        assert!((centre.y - 1.0).abs() < 0.1, "rests on top of the ball: {centre}");
        let corners_low = p.iter().filter(|q| q.x.abs() > 1.3 && q.z.abs() > 1.3).all(|q| q.y < 0.9);
        assert!(corners_low, "corners droop");
    }

    #[test]
    fn pins_follow_constrained_nodes_and_colliders_move() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        // A keyed driver; the pin target only follows it through a constraint (no keys).
        execute(&mut app, "spaceLocator -n drv; spaceLocator -n holder; pointConstraint drv holder").unwrap();
        execute(&mut app, "currentTime 1; setKeyframe -at translateX -v 0 drv; currentTime 25; setKeyframe -at translateX -v 3 drv; currentTime 1").unwrap();
        execute(&mut app, "polyPlane -w 1 -h 1 -sx 4 -sy 4 -n rag; move 0 2 0 rag; nCloth rag").unwrap();
        let rid = app.scene.find_by_name("rag").unwrap();
        let r0 = wpts(&app, rid);
        let corner = r0.iter().enumerate().min_by(|a, b| (a.1.x + a.1.z).total_cmp(&(b.1.x + b.1.z))).unwrap().0;
        execute(&mut app, &format!("clothPin -to holder rag.vtx[{corner}]")).unwrap();
        at(&mut app, 25.0);
        let p = wpts(&app, rid);
        assert!((p[corner] - (r0[corner] + Vec3::X * 3.0)).length() < 1e-3, "pin rode the constraint: {} vs {}", p[corner], r0[corner]);
        // A keyed collider sweeps through falling particles.
        execute(&mut app, "polyCube -w 4 -h 0.2 -d 4 -n paddle; emitter -type volume -r 300 -spd 0 -lifespan 0 -n dust; move 0 3 0 dust; scale 3 0.1 3 dust; setAttr dust.ground 1; connectDynamic -c paddle dust").unwrap();
        execute(&mut app, "currentTime 1; setKeyframe -at translateY -v 1 paddle; currentTime 49; setKeyframe -at translateY -v 1 paddle; currentTime 1").unwrap();
        at(&mut app, 49.0);
        let id = app.scene.find_by_name("dust").unwrap();
        let ps = match &app.scene.node(id).unwrap().shape {
            Shape::Particles(p) => p.current().to_vec(),
            _ => unreachable!(),
        };
        let under = ps.iter().filter(|q| q.p.y < 0.85 && q.p.x.abs() < 1.9 && q.p.z.abs() < 1.9).count();
        assert_eq!(under, 0, "particles stay above the paddle");
        // Moving the paddle invalidates the cache and re-simulates.
        execute(&mut app, "currentTime 1; setKeyframe -at translateY -v 2.5 paddle; currentTime 49; setKeyframe -at translateY -v 2.5 paddle").unwrap();
        at(&mut app, 49.0);
        let ps2 = match &app.scene.node(id).unwrap().shape {
            Shape::Particles(p) => p.current().to_vec(),
            _ => unreachable!(),
        };
        assert!(ps2.iter().filter(|q| q.p.x.abs() < 1.9 && q.p.z.abs() < 1.9).all(|q| q.p.y > 2.5), "re-simulated with the higher paddle");
    }

    #[test]
    fn fx_saves_and_exports_to_usd() {
        let dir = std::env::temp_dir().join("rm_fx_io");
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "playbackOptions -min 1 -max 12; emitter -r 120 -spd 2 -n sparks; polyPlane -w 1 -h 1 -sx 3 -sy 3 -n cloth1; move 0 2 0 cloth1; nCloth cloth1; clothPin cloth1.vtx[0]").unwrap();
        at(&mut app, 12.0);
        // .rma keeps the settings (caches are rebuilt on load).
        let rma = dir.join("fx.rma");
        app.save_to(rma.clone());
        let mut b = App::new();
        b.open_from(rma);
        at(&mut b, 12.0);
        assert_eq!(parts(&app, "sparks"), parts(&b, "sparks"), "same simulation after reload");
        let cid = b.scene.find_by_name("cloth1").unwrap();
        assert_eq!(b.scene.node(cid).unwrap().cloth.as_ref().unwrap().settings.pins, vec![0]);
        assert_eq!(wpts(&app, app.scene.find_by_name("cloth1").unwrap()), wpts(&b, cid));
        // USD: animated Points for the particles, baked points for the cloth.
        let usda = dir.join("fx.usda");
        let opts = crate::export::ExportOptions::from_app(&app, false);
        crate::export::export_path(&app, &usda, &opts).unwrap();
        let text = std::fs::read_to_string(&usda).unwrap();
        assert!(text.contains("def Points \"sparksShape\"") && text.contains("points.timeSamples"), "particles");
        assert!(text.contains("def Mesh \"cloth1Shape\""));
        let mesh_part = &text[text.find("def Mesh \"cloth1Shape\"").unwrap()..];
        assert!(mesh_part.contains("point3f[] points.timeSamples"), "cloth baked");
        // Undo of a setting change restores the old simulation.
        let before = parts(&app, "sparks").len();
        execute(&mut app, "setAttr sparks.rate 10").unwrap();
        at(&mut app, 12.0);
        assert!(parts(&app, "sparks").len() < before);
        app.undo();
        at(&mut app, 12.0);
        assert_eq!(parts(&app, "sparks").len(), before);
        assert_eq!(execute(&mut app, "getAttr sparks.count").unwrap(), before.to_string());
        assert!(execute(&mut app, "setAttr sparks.bogus 1").is_err());
    }

    #[test]
    fn fields_push_particles_and_cloth() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        // Weightless particles at rest: an air field carries them along +X at ~its speed.
        execute(&mut app, "emitter -type omni -r 24 -spd 0 -lifespan 0 -n p; move 0 2 0 p; setAttr p.gravity 0 0 0; setAttr p.drag 0; setAttr p.ground 0").unwrap();
        execute(&mut app, "select -cl; airField -m 3 -s 4 -n wind").unwrap();
        assert_eq!(execute(&mut app, "getAttr wind.type").unwrap(), "air");
        at(&mut app, 49.0);
        let ps = parts(&app, "p");
        let oldest = ps.iter().min_by_key(|q| q.id).unwrap();
        assert!((oldest.v.x - 3.0).abs() < 0.2 && oldest.p.x > 3.0, "carried by the wind: v {} p {}", oldest.v, oldest.p);
        // Restricting the field to another object stops it acting here.
        execute(&mut app, "polyPlane -w 1 -h 1 -sx 4 -sy 4 -n rag; rotate 0 0 90 rag; move 0 3 0 rag; nCloth rag; connectDynamic -f wind rag").unwrap();
        at(&mut app, 49.0);
        let ps = parts(&app, "p");
        assert!(ps.iter().all(|q| q.v.length() < 1e-4), "wind only acts on its target now");
        // ...and the cloth (falling, unpinned) drifts +X with the wind.
        let rid = app.scene.find_by_name("rag").unwrap();
        let cx = wpts(&app, rid).iter().map(|p| p.x).sum::<f32>() / 25.0;
        assert!(cx > 0.2, "cloth drifts with the air field: {cx}");
        // A vortex swirls; radial pushes out; newton pulls in; gravity field adds weight.
        execute(&mut app, "connectDynamic -rf wind rag; delete wind rag").unwrap();
        execute(&mut app, "vortexField -m 4 -n vx p").unwrap();
        at(&mut app, 25.0);
        let ps = parts(&app, "p");
        let q = ps.iter().min_by_key(|q| q.id).unwrap();
        // Particle at (0,2,0) on the vortex axis line? the field sits at the origin with axis Y,
        // so particles there are on the axis: no swirl. Move the field off-axis instead.
        let _ = q;
        execute(&mut app, "move 1 2 0 vx").unwrap();
        at(&mut app, 25.0);
        let ps = parts(&app, "p");
        let q = ps.iter().min_by_key(|q| q.id).unwrap();
        // r = p - c = (-1, 0, 0); Y × (-1,0,0) = (0,0,1): swirl toward +Z.
        assert!(q.p.z > 0.3, "vortex swirls: {}", q.p);
        execute(&mut app, "delete vx; radialField -m 5 -att 0 -p 0 1 0 -n blast").unwrap();
        at(&mut app, 25.0);
        let q = *parts(&app, "p").iter().min_by_key(|q| q.id).unwrap();
        assert!(q.p.y > 2.5, "radial pushes away from (0,1,0): {}", q.p);
        execute(&mut app, "setAttr blast.maxDistance 0.5").unwrap();
        at(&mut app, 25.0);
        let q = *parts(&app, "p").iter().min_by_key(|q| q.id).unwrap();
        assert!((q.p.y - 2.0).abs() < 1e-3, "out of range: {}", q.p);
        execute(&mut app, "delete blast; newtonField -m 3 -att 0 -p 0 0 0 -n pull; gravityField -m 2 -n g2").unwrap();
        at(&mut app, 13.0);
        let q = *parts(&app, "p").iter().min_by_key(|q| q.id).unwrap();
        // Both pull down: (3 + 2) m/s² for 0.5 s ≈ 0.6 m.
        assert!(q.p.y < 1.6 && q.p.y > 1.2, "newton + gravity: {}", q.p);
        assert!(execute(&mut app, "field -type bogus").is_err());
        assert!(execute(&mut app, "setAttr g2.nope 1").is_err());
    }

    #[test]
    fn cloth_self_collision_keeps_layers_apart() {
        let mut app = App::new();
        app.scene.nodes.clear();
        app.scene.fps = 24.0;
        // A sheet crumpled by a strong attractor (no gravity) folds onto itself.
        let drop = |app: &mut App, name: &str, extra: &str| -> f32 {
            execute(app, &format!("polyPlane -w 2 -h 2 -sx 16 -sy 16 -n {name}; move 0 0.5 0 {name}; nCloth -th 0.005 {extra} {name}; setAttr {name}.gravity 0 0 0; setAttr {name}.drag 0.2; select -cl; newtonField -m 15 -att 0 -p 0 0.4 0 -n pull_{name} {name}")).unwrap();
            at(app, 60.0);
            let id = app.scene.find_by_name(name).unwrap();
            let p = wpts(app, id);
            let rest: Vec<Vec3> = {
                let w = app.scene.world_matrix(id);
                app.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|q| w.transform_point3(*q)).collect()
            };
            let ext = p.iter().map(|q| (q - Vec3::new(0.0, 0.4, 0.0)).length()).fold(0.0, f32::max);
            let mut min = f32::MAX;
            for i in 0..p.len() {
                for j in i + 1..p.len() {
                    if rest[i].distance(rest[j]) > 0.4 {
                        min = min.min(p[i].distance(p[j]));
                    }
                }
            }
            eprintln!("{name}: crumpled to radius {ext:.3}");
            execute(app, &format!("delete {name} pull_{name}")).unwrap();
            min
        };
        let without = drop(&mut app, "a", "");
        let with = drop(&mut app, "b", "-selfCollide");
        eprintln!("closest far-apart vertices: without {without:.4}, with {with:.4}");
        assert!(with > 0.08, "self collision keeps a gap: {with}");
        assert!(without < 0.05, "without it the layers pass through each other: {without}");
        assert!(with > without * 2.0, "{with} vs {without}");
    }

    /// `cargo test --release fx_perf -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn fx_perf() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -sa 32 -sh 24 -n ball; polyPlane -w 3 -h 3 -sx 40 -sy 40 -n sheet; move 0 1.6 0 sheet; nCloth sheet; connectDynamic -c ball sheet").unwrap();
        let t = std::time::Instant::now();
        at(&mut app, 120.0);
        eprintln!("cloth 41x41 verts, 120 frames: {:.2}s ({:.1} ms/frame)", t.elapsed().as_secs_f32(), t.elapsed().as_secs_f32() * 1000.0 / 120.0);
        execute(&mut app, "delete sheet; emitter -r 2000 -spd 4 -sp 30 -dir 0 1 0 -lifespan 3 -n fount; setAttr fount.turbulence 2; connectDynamic -c ball fount").unwrap();
        let t = std::time::Instant::now();
        at(&mut app, 120.0);
        eprintln!("particles (2000/s, ~6000 alive), 120 frames: {:.2}s ({:.1} ms/frame), {} alive", t.elapsed().as_secs_f32(), t.elapsed().as_secs_f32() * 1000.0 / 120.0, parts(&app, "fount").len());
        let t = std::time::Instant::now();
        let g = app.build_geometry();
        eprintln!("geometry build {:.1} ms, {} tri chunks", t.elapsed().as_secs_f32() * 1000.0, g.tris.len());
    }
}
