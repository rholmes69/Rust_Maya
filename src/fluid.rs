//! Smoke and fire: a voxel fluid container (stable-fluids solver) fed by emitters, pushed by
//! buoyancy, vorticity, turbulence and the scene's fields, blocked by colliders, and rendered
//! as a participating medium by the path tracer.
//!
//! The container is a box of `size` (local units) centred on its node; the node's transform
//! places it in the world. Velocity, density and temperature live at cell centres. Each frame
//! is cached (density and temperature quantised to 16 bits); the full solver state is kept
//! only for the last simulated frame, so playback past the end continues the simulation.

use crate::app::App;
use crate::fx::{FieldSet, Rng, SimCache, Tracks, TriGrid, MAX_FRAMES};
use crate::scene::{NodeId, Scene, Shape};
use glam::{Mat3, Mat4, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Something that feeds smoke / heat into a container.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FluidSource {
    /// Mesh (emits from its surface), particle system (from each particle) or any other node
    /// (a sphere around its pivot).
    pub node: NodeId,
    /// Smoke density added per second.
    pub density: f32,
    /// Temperature added per second (drives buoyancy and fire glow).
    pub heat: f32,
    /// Emission radius (world units).
    pub radius: f32,
    /// Initial speed along the source's up axis (local +Y of the source).
    pub speed: f32,
    /// How much of the source's own motion the emitted smoke keeps (0..1).
    pub inherit: f32,
    /// Seconds of emission after the start (0 = forever): short bursts make explosions.
    pub duration: f32,
    /// Push outward from the source as it emits (explosions).
    pub expansion: f32,
}

impl Default for FluidSource {
    fn default() -> Self {
        FluidSource { node: 0, density: 4.0, heat: 5.0, radius: 0.35, speed: 0.0, inherit: 0.5, duration: 0.0, expansion: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FluidSettings {
    /// Voxels along the container's longest side.
    pub res: u32,
    /// Container size (local units).
    pub size: [f32; 3],
    /// First simulated frame.
    pub start: f32,
    /// Solver steps per frame (raised automatically for fast flows, up to 4×).
    pub substeps: u32,
    /// Upward acceleration per unit of temperature.
    pub buoyancy: f32,
    /// Downward pull per unit of density (heavy smoke sinks).
    pub weight: f32,
    /// Temperature lost per second (fraction, 0..).
    pub cooling: f32,
    /// Density lost per second (fraction, 0..).
    pub dissipation: f32,
    /// Vorticity confinement: keeps small swirls alive.
    pub vorticity: f32,
    /// Noise forces (acceleration) and their scale (per unit).
    pub turbulence: f32,
    pub turbulence_freq: f32,
    /// Velocity lost per second (0..).
    pub drag: f32,
    /// Smoke leaves through the sides and top (otherwise the box is closed). The bottom is
    /// always closed.
    pub open: bool,
    pub sources: Vec<FluidSource>,
    pub colliders: Vec<NodeId>,
    // ---- look (not part of the simulation)
    /// Smoke colour = single-scattering albedo (linear).
    pub smoke_color: [f32; 3],
    /// Extinction per unit density per world unit.
    pub density_scale: f32,
    /// Henyey–Greenstein anisotropy of the smoke (0 = isotropic, >0 forward scattering).
    pub anisotropy: f32,
    /// Fire glow strength (0 = smoke only).
    pub fire_intensity: f32,
    /// Fire colour at full temperature (linear); cooler fire fades to deep red.
    pub fire_color: [f32; 3],
    /// Temperature where the glow starts.
    pub fire_temp: f32,
}

impl Default for FluidSettings {
    fn default() -> Self {
        FluidSettings {
            res: 48,
            size: [4.0, 6.0, 4.0],
            start: 1.0,
            substeps: 1,
            buoyancy: 6.0,
            weight: 0.04,
            cooling: 0.4,
            dissipation: 0.08,
            vorticity: 2.0,
            turbulence: 0.0,
            turbulence_freq: 2.0,
            drag: 0.05,
            open: true,
            sources: Vec::new(),
            colliders: Vec::new(),
            smoke_color: [0.55, 0.55, 0.58],
            density_scale: 6.0,
            anisotropy: 0.3,
            fire_intensity: 0.0,
            fire_color: [1.0, 0.42, 0.08],
            fire_temp: 0.35,
        }
    }
}

impl FluidSettings {
    /// Grid resolution (cells per axis) and the (cubic) voxel size in local units.
    pub fn dims(&self) -> ([usize; 3], f32) {
        let longest = self.size[0].max(self.size[1]).max(self.size[2]).max(1e-3);
        let h = longest / self.res.clamp(4, 256) as f32;
        let n = |s: f32| ((s.max(1e-3) / h).round() as usize).clamp(2, 256);
        ([n(self.size[0]), n(self.size[1]), n(self.size[2])], h)
    }

    /// The simulation inputs only (look attributes don't re-simulate).
    fn sim_key(&self) -> String {
        let mut s = self.clone();
        s.smoke_color = [0.0; 3];
        s.density_scale = 0.0;
        s.anisotropy = 0.0;
        s.fire_intensity = 0.0;
        s.fire_color = [0.0; 3];
        s.fire_temp = 0.0;
        format!("{s:?}")
    }

    pub fn set_attr(&mut self, attr: &str, vals: &[f32]) -> Result<(), String> {
        let one = |v: &[f32]| v.first().copied().ok_or_else(|| format!("{attr}: missing value"));
        let three = |v: &[f32]| if v.len() >= 3 { Ok([v[0], v[1], v[2]]) } else { Err(format!("{attr}: needs 3 values")) };
        match attr {
            "res" | "resolution" => self.res = (one(vals)? as u32).clamp(4, 256),
            "size" => self.size = three(vals)?.map(|v| v.max(0.01)),
            "sizeX" => self.size[0] = one(vals)?.max(0.01),
            "sizeY" => self.size[1] = one(vals)?.max(0.01),
            "sizeZ" => self.size[2] = one(vals)?.max(0.01),
            "start" | "startFrame" => self.start = one(vals)?,
            "substeps" => self.substeps = (one(vals)? as u32).clamp(1, 16),
            "buoyancy" => self.buoyancy = one(vals)?,
            "weight" => self.weight = one(vals)?,
            "cooling" => self.cooling = one(vals)?.max(0.0),
            "dissipation" => self.dissipation = one(vals)?.max(0.0),
            "vorticity" => self.vorticity = one(vals)?.max(0.0),
            "turbulence" => self.turbulence = one(vals)?.max(0.0),
            "turbulenceFrequency" | "turbulenceFreq" => self.turbulence_freq = one(vals)?.max(0.0),
            "drag" => self.drag = one(vals)?.max(0.0),
            "open" | "openBoundaries" => self.open = one(vals)? != 0.0,
            "smokeColor" | "color" => self.smoke_color = three(vals)?.map(|v| v.clamp(0.0, 1.0)),
            "densityScale" | "opacity" => self.density_scale = one(vals)?.max(0.0),
            "anisotropy" => self.anisotropy = one(vals)?.clamp(-0.95, 0.95),
            "fireIntensity" | "fire" => self.fire_intensity = one(vals)?.max(0.0),
            "fireColor" => self.fire_color = three(vals)?.map(|v| v.max(0.0)),
            "fireTemperature" | "fireTemp" => self.fire_temp = one(vals)?.max(0.0),
            // Applied to every source.
            "emitDensity" | "density" => self.sources.iter_mut().for_each(|s| s.density = vals.first().copied().unwrap_or(s.density).max(0.0)),
            "emitHeat" | "heat" => self.sources.iter_mut().for_each(|s| s.heat = vals.first().copied().unwrap_or(s.heat).max(0.0)),
            "emitRadius" | "radius" => self.sources.iter_mut().for_each(|s| s.radius = vals.first().copied().unwrap_or(s.radius).max(1e-3)),
            "emitSpeed" | "speed" => self.sources.iter_mut().for_each(|s| s.speed = vals.first().copied().unwrap_or(s.speed)),
            "expansion" => self.sources.iter_mut().for_each(|s| s.expansion = vals.first().copied().unwrap_or(s.expansion).max(0.0)),
            "duration" => self.sources.iter_mut().for_each(|s| s.duration = vals.first().copied().unwrap_or(s.duration).max(0.0)),
            _ => return Err(format!("fluid: unknown attribute '{attr}'")),
        }
        Ok(())
    }
}

/// A cached frame: density and temperature quantised to 16 bits against the frame's maxima.
#[derive(Clone, Debug, Default)]
pub struct FluidFrame {
    pub dims: [usize; 3],
    pub density: Vec<u16>,
    pub temp: Vec<u16>,
    pub dmax: f32,
    pub tmax: f32,
    /// Full solver state (last simulated frame only).
    state: Option<Arc<State>>,
}

impl FluidFrame {
    pub fn density_at(&self, i: usize) -> f32 {
        self.density.get(i).map(|&q| q as f32 / 65535.0 * self.dmax).unwrap_or(0.0)
    }
    pub fn temp_at(&self, i: usize) -> f32 {
        self.temp.get(i).map(|&q| q as f32 / 65535.0 * self.tmax).unwrap_or(0.0)
    }
    pub fn densities(&self) -> Vec<f32> {
        (0..self.density.len()).map(|i| self.density_at(i)).collect()
    }
    pub fn temps(&self) -> Vec<f32> {
        (0..self.temp.len()).map(|i| self.temp_at(i)).collect()
    }

    fn from_state(s: &State, dims: [usize; 3], keep: bool) -> FluidFrame {
        let dmax = s.d.iter().fold(0.0f32, |a, &b| a.max(b));
        let tmax = s.t.iter().fold(0.0f32, |a, &b| a.max(b));
        let q = |v: f32, m: f32| if m > 0.0 { ((v / m).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16 } else { 0 };
        FluidFrame { dims, density: s.d.iter().map(|&v| q(v, dmax)).collect(), temp: s.t.iter().map(|&v| q(v, tmax)).collect(), dmax, tmax, state: keep.then(|| Arc::new(s.clone())) }
    }

    /// Linear blend toward `b` (sub-frame motion blur / scrubbing).
    fn lerp(&self, b: &FluidFrame, t: f32) -> FluidFrame {
        if self.dims != b.dims {
            return FluidFrame { state: None, ..self.clone() };
        }
        let d: Vec<f32> = (0..self.density.len()).map(|i| self.density_at(i) * (1.0 - t) + b.density_at(i) * t).collect();
        let tt: Vec<f32> = (0..self.temp.len()).map(|i| self.temp_at(i) * (1.0 - t) + b.temp_at(i) * t).collect();
        let s = State { v: Vec::new(), d, t: tt };
        FluidFrame::from_state(&s, self.dims, false)
    }
}

/// Full solver state.
#[derive(Clone, Debug, Default)]
struct State {
    v: Vec<Vec3>,
    d: Vec<f32>,
    t: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Fluid {
    pub settings: FluidSettings,
    #[serde(skip)]
    pub cache: Arc<SimCache<FluidFrame>>,
}

impl Fluid {
    /// The frame shown now.
    pub fn current(&self) -> Option<&FluidFrame> {
        if let Some(b) = &self.cache.blend {
            return Some(b);
        }
        self.cache.current.and_then(|i| self.cache.frames.get(i)).map(|f| f.as_ref())
    }
}

// ------------------------------------------------------------------------------ grid helpers

#[derive(Clone, Copy)]
struct Grid {
    n: [usize; 3],
    h: f32,
    /// Local position of cell (0,0,0)'s centre.
    origin: Vec3,
}

impl Grid {
    fn new(s: &FluidSettings) -> Grid {
        let (n, h) = s.dims();
        let size = Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) * h;
        Grid { n, h, origin: -size * 0.5 + Vec3::splat(h * 0.5) }
    }
    fn len(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.n[0] * (y + self.n[1] * z)
    }
    fn xyz(&self, i: usize) -> (usize, usize, usize) {
        (i % self.n[0], (i / self.n[0]) % self.n[1], i / (self.n[0] * self.n[1]))
    }
    fn center(&self, i: usize) -> Vec3 {
        let (x, y, z) = self.xyz(i);
        self.origin + Vec3::new(x as f32, y as f32, z as f32) * self.h
    }
    /// Continuous grid coordinates of a local point (cell centres at integers).
    fn coords(&self, p: Vec3) -> Vec3 {
        (p - self.origin) / self.h
    }
}

/// Trilinear sample of a cell-centred field at grid coordinates `g` (clamped to the grid);
/// also the min / max of the 8 corners (for MacCormack clamping).
fn sample<T: Copy + Lerp>(grid: &Grid, f: &[T], g: Vec3) -> T {
    let mx = Vec3::new(grid.n[0] as f32 - 1.0, grid.n[1] as f32 - 1.0, grid.n[2] as f32 - 1.0);
    let g = g.clamp(Vec3::ZERO, mx);
    let (x0, y0, z0) = (g.x.floor() as usize, g.y.floor() as usize, g.z.floor() as usize);
    let (x1, y1, z1) = ((x0 + 1).min(grid.n[0] - 1), (y0 + 1).min(grid.n[1] - 1), (z0 + 1).min(grid.n[2] - 1));
    let (fx, fy, fz) = (g.x - x0 as f32, g.y - y0 as f32, g.z - z0 as f32);
    let v = |x, y, z| f[grid.idx(x, y, z)];
    let c00 = v(x0, y0, z0).lerp_to(v(x1, y0, z0), fx);
    let c10 = v(x0, y1, z0).lerp_to(v(x1, y1, z0), fx);
    let c01 = v(x0, y0, z1).lerp_to(v(x1, y0, z1), fx);
    let c11 = v(x0, y1, z1).lerp_to(v(x1, y1, z1), fx);
    c00.lerp_to(c10, fy).lerp_to(c01.lerp_to(c11, fy), fz)
}

fn corners_minmax(grid: &Grid, f: &[f32], g: Vec3) -> (f32, f32) {
    let mx = Vec3::new(grid.n[0] as f32 - 1.0, grid.n[1] as f32 - 1.0, grid.n[2] as f32 - 1.0);
    let g = g.clamp(Vec3::ZERO, mx);
    let (x0, y0, z0) = (g.x.floor() as usize, g.y.floor() as usize, g.z.floor() as usize);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for z in [z0, (z0 + 1).min(grid.n[2] - 1)] {
        for y in [y0, (y0 + 1).min(grid.n[1] - 1)] {
            for x in [x0, (x0 + 1).min(grid.n[0] - 1)] {
                let v = f[grid.idx(x, y, z)];
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    (lo, hi)
}

trait Lerp {
    fn lerp_to(self, b: Self, t: f32) -> Self;
}
impl Lerp for f32 {
    fn lerp_to(self, b: f32, t: f32) -> f32 {
        self + (b - self) * t
    }
}
impl Lerp for Vec3 {
    fn lerp_to(self, b: Vec3, t: f32) -> Vec3 {
        self.lerp(b, t)
    }
}

fn par<T: Send, F: Fn(usize) -> T + Sync>(n: usize, f: F) -> Vec<T> {
    let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).min(16);
    if n < 4096 || threads < 2 {
        return (0..n).map(f).collect();
    }
    let chunk = n.div_ceil(threads);
    let f = &f;
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads).map(|k| s.spawn(move || (k * chunk..((k + 1) * chunk).min(n)).map(f).collect::<Vec<T>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

// ------------------------------------------------------------------------------ emission

/// Emission points of one source at one frame (world space): (position, velocity).
struct Emit {
    pts: Vec<(Vec3, Vec3)>,
    src: FluidSource,
    /// The source's up axis (world).
    up: Vec3,
    /// Source centre (for expansion).
    centre: Vec3,
}

/// A fixed point on a mesh: (triangle corner indices, barycentric weights).
type SurfaceSample = ([u32; 3], [f32; 3]);

/// Fixed surface samples of a mesh.
fn surface_samples(faces: &[Vec<u32>], pts: &[Vec3], spacing: f32, seed: u64) -> Vec<SurfaceSample> {
    let mut tris = Vec::new();
    for f in faces {
        for i in 1..f.len().saturating_sub(1) {
            tris.push([f[0], f[i], f[i + 1]]);
        }
    }
    let area = |t: &[u32; 3]| (pts[t[1] as usize] - pts[t[0] as usize]).cross(pts[t[2] as usize] - pts[t[0] as usize]).length() * 0.5;
    let total: f32 = tris.iter().map(area).sum();
    let count = ((total / (spacing * spacing).max(1e-8)) as usize).clamp(16, 20_000);
    let mut cdf = Vec::with_capacity(tris.len());
    let mut acc = 0.0;
    for t in &tris {
        acc += area(t);
        cdf.push(acc);
    }
    let mut rng = Rng::new(seed);
    (0..count)
        .filter_map(|_| {
            let r = rng.f() * acc;
            let k = cdf.partition_point(|&c| c < r).min(tris.len().checked_sub(1)?);
            let (mut a, mut b) = (rng.f(), rng.f());
            if a + b > 1.0 {
                a = 1.0 - a;
                b = 1.0 - b;
            }
            Some((tris[k], [1.0 - a - b, a, b]))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn emit_points(scene: &Scene, tr: &Tracks, prev: Option<&Tracks>, src: &FluidSource, frame: f32, fps: f32, spacing: f32, samples: &mut Option<Vec<SurfaceSample>>) -> Option<Emit> {
    let n = scene.node(src.node)?;
    let w = tr.world(src.node);
    let up = w.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
    let centre = w.transform_point3(Vec3::ZERO);
    let dt = 1.0 / fps.max(1.0);
    match &n.shape {
        Shape::Particles(ps) => {
            // The particle system's own cache (the copy in `tr` doesn't simulate).
            let k = ((frame - ps.settings.start).round().max(0.0)) as usize;
            let f = ps.cache.frames.get(k.min(ps.cache.frames.len().saturating_sub(1)))?;
            Some(Emit { pts: f.particles.iter().map(|q| (q.p, q.v)).collect(), src: src.clone(), up, centre })
        }
        Shape::Poly { .. } => {
            let (pts, faces) = tr.mesh_world(src.node)?;
            let smp = samples.get_or_insert_with(|| surface_samples(&faces, &pts, spacing, src.node.wrapping_mul(7919)));
            let at = |p: &[Vec3], s: &SurfaceSample| p[s.0[0] as usize] * s.1[0] + p[s.0[1] as usize] * s.1[1] + p[s.0[2] as usize] * s.1[2];
            let before = prev.and_then(|p| p.mesh_world(src.node)).filter(|(p, _)| p.len() == pts.len());
            let out = smp
                .iter()
                .filter(|s| s.0.iter().all(|&i| (i as usize) < pts.len()))
                .map(|s| {
                    let p = at(&pts, s);
                    let v = before.as_ref().map(|(b, _)| (p - at(b, s)) / dt).unwrap_or(Vec3::ZERO);
                    (p, v)
                })
                .collect();
            Some(Emit { pts: out, src: src.clone(), up, centre })
        }
        _ => {
            let v = prev.map(|p| (centre - p.world(src.node).transform_point3(Vec3::ZERO)) / dt).unwrap_or(Vec3::ZERO);
            Some(Emit { pts: vec![(centre, v)], src: src.clone(), up, centre })
        }
    }
}

// ------------------------------------------------------------------------------ solver

struct Frame<'a> {
    grid: Grid,
    s: &'a FluidSettings,
    /// World ↔ local.
    world: Mat4,
    inv: Mat4,
    inv3: Mat3,
    w3: Mat3,
    /// World units per local unit (average scale).
    scale: f32,
    fields: FieldSet,
    emits: Vec<Emit>,
    solid: Vec<bool>,
    /// Seconds since the start.
    time: f32,
}

fn step_frame(prev: &State, fr: &Frame, fps: f32) -> State {
    let g = fr.grid;
    let mut st = prev.clone();
    if st.d.len() != g.len() {
        st = State { v: vec![Vec3::ZERO; g.len()], d: vec![0.0; g.len()], t: vec![0.0; g.len()] };
    }
    let frame_dt = 1.0 / fps.max(1.0);
    // Substeps: honour the setting, add more for fast flows (CFL ≈ 2 cells).
    let vmax = st.v.iter().fold(0.0f32, |a, v| a.max(v.length()));
    let need = ((vmax * frame_dt) / (2.0 * g.h)).ceil() as u32;
    let subs = fr.s.substeps.max(need).clamp(1, (fr.s.substeps * 4).max(4));
    let dt = frame_dt / subs as f32;
    for k in 0..subs {
        let t = fr.time + k as f32 * dt;
        substep(&mut st, fr, dt, t);
    }
    st
}

fn substep(st: &mut State, fr: &Frame, dt: f32, t: f32) {
    let g = fr.grid;
    let s = fr.s;
    let n = g.len();
    // 1. Sources.
    let mut div_target = vec![0.0f32; n];
    for e in &fr.emits {
        let src = &e.src;
        if src.duration > 0.0 && t > src.duration {
            continue;
        }
        let r_local = (src.radius / fr.scale).max(g.h * 1.0);
        let rc = (r_local / g.h).ceil() as i64 + 1;
        let up_l = (fr.inv3 * e.up).normalize_or(Vec3::Y);
        let centre_l = fr.inv.transform_point3(e.centre);
        // Spread the emission over the points so the total doesn't scale with the count.
        let per = 1.0 / (e.pts.len().max(1) as f32).sqrt().max(1.0);
        for &(pw, vw) in &e.pts {
            let pl = fr.inv.transform_point3(pw);
            let c = g.coords(pl);
            let (cx, cy, cz) = (c.x.round() as i64, c.y.round() as i64, c.z.round() as i64);
            let vel_l = fr.inv3 * vw * src.inherit + up_l * src.speed;
            for z in (cz - rc).max(0)..=(cz + rc).min(g.n[2] as i64 - 1) {
                for y in (cy - rc).max(0)..=(cy + rc).min(g.n[1] as i64 - 1) {
                    for x in (cx - rc).max(0)..=(cx + rc).min(g.n[0] as i64 - 1) {
                        let i = g.idx(x as usize, y as usize, z as usize);
                        if fr.solid[i] {
                            continue;
                        }
                        let d = g.center(i).distance(pl);
                        if d > r_local {
                            continue;
                        }
                        let w = (1.0 - (d / r_local).powi(2)) * per;
                        st.d[i] += src.density * dt * w;
                        st.t[i] += src.heat * dt * w;
                        if vel_l.length_squared() > 0.0 {
                            st.v[i] = st.v[i].lerp(vel_l, (w * 0.5).min(1.0));
                        }
                        if src.expansion > 0.0 {
                            div_target[i] += src.expansion * w;
                            let out = (g.center(i) - centre_l).normalize_or_zero();
                            st.v[i] += out * src.expansion * w * dt * 4.0;
                        }
                    }
                }
            }
        }
    }
    // 2. Forces: buoyancy, weight, turbulence, fields, drag.
    let up_l = (fr.inv3 * Vec3::Y).normalize_or(Vec3::Y);
    let has_fields = !fr.fields.is_empty();
    let forces: Vec<Vec3> = par(n, |i| {
        if fr.solid[i] {
            return Vec3::ZERO;
        }
        let mut a = up_l * (s.buoyancy * st.t[i] - s.weight * st.d[i]);
        if s.turbulence > 0.0 && (st.d[i] > 1e-3 || st.t[i] > 1e-3) {
            a += crate::fx::turbulence(g.center(i), t, s.turbulence_freq) * s.turbulence;
        }
        if has_fields {
            let pw = fr.world.transform_point3(g.center(i));
            let vw = fr.w3 * st.v[i];
            a += fr.inv3 * fr.fields.accel(pw, vw, t);
        }
        a - st.v[i] * s.drag
    });
    for (v, a) in st.v.iter_mut().zip(&forces) {
        *v += *a * dt;
    }
    // 3. Vorticity confinement.
    if s.vorticity > 0.0 {
        let curl = curl_of(&g, &st.v);
        let mag: Vec<f32> = curl.iter().map(|c| c.length()).collect();
        let f: Vec<Vec3> = par(n, |i| {
            if fr.solid[i] {
                return Vec3::ZERO;
            }
            let gr = gradient(&g, &mag, i);
            let len = gr.length();
            if len < 1e-6 {
                return Vec3::ZERO;
            }
            (gr / len).cross(curl[i]) * (s.vorticity * g.h)
        });
        for (v, a) in st.v.iter_mut().zip(&f) {
            *v += *a * dt;
        }
    }
    // 4. Advect velocity (semi-Lagrangian).
    let v0 = st.v.clone();
    st.v = par(n, |i| {
        if fr.solid[i] {
            return Vec3::ZERO;
        }
        let back = g.coords(g.center(i) - v0[i] * dt);
        sample(&g, &v0, back)
    });
    enforce_walls(&g, s, &fr.solid, &mut st.v);
    // 5. Project.
    project(&g, s, &fr.solid, &mut st.v, &div_target);
    // 6. Advect density and temperature (MacCormack, clamped).
    let vel = &st.v;
    let open = s.open;
    let adv = |f: &[f32]| -> Vec<f32> {
        let fwd: Vec<f32> = par(n, |i| sample_scalar(&g, f, g.coords(g.center(i) - vel[i] * dt), open));
        let back: Vec<f32> = par(n, |i| sample_scalar(&g, &fwd, g.coords(g.center(i) + vel[i] * dt), open));
        par(n, |i| {
            if fr.solid[i] {
                return 0.0;
            }
            let c = g.coords(g.center(i) - vel[i] * dt);
            let (lo, hi) = corners_minmax(&g, f, c);
            let v = fwd[i] + 0.5 * (f[i] - back[i]);
            if inside(&g, c) { v.clamp(lo.min(fwd[i]), hi.max(fwd[i])) } else { fwd[i] }
        })
    };
    st.d = adv(&st.d);
    st.t = adv(&st.t);
    // 7. Dissipation / cooling.
    let (kd, kt) = ((-s.dissipation * dt).exp(), (-s.cooling * dt).exp());
    for d in &mut st.d {
        *d = (*d * kd).max(0.0);
    }
    for t in &mut st.t {
        *t = (*t * kt).max(0.0);
    }
}

fn inside(g: &Grid, c: Vec3) -> bool {
    c.x >= 0.0 && c.y >= 0.0 && c.z >= 0.0 && c.x <= g.n[0] as f32 - 1.0 && c.y <= g.n[1] as f32 - 1.0 && c.z <= g.n[2] as f32 - 1.0
}

/// Scalar sample: outside an open container (except below the floor) is empty.
fn sample_scalar(g: &Grid, f: &[f32], c: Vec3, open: bool) -> f32 {
    if open {
        let m = 0.5;
        if c.x < -m || c.z < -m || c.x > g.n[0] as f32 - 1.0 + m || c.z > g.n[2] as f32 - 1.0 + m || c.y > g.n[1] as f32 - 1.0 + m {
            return 0.0;
        }
    }
    sample(g, f, c)
}

fn neighbor<T: Copy>(g: &Grid, f: &[T], i: usize, axis: usize, dir: i64) -> Option<T> {
    let (x, y, z) = g.xyz(i);
    let mut c = [x as i64, y as i64, z as i64];
    c[axis] += dir;
    if c[axis] < 0 || c[axis] >= g.n[axis] as i64 {
        return None;
    }
    Some(f[g.idx(c[0] as usize, c[1] as usize, c[2] as usize)])
}

fn gradient(g: &Grid, f: &[f32], i: usize) -> Vec3 {
    let mut out = [0.0; 3];
    for (a, o) in out.iter_mut().enumerate() {
        let lo = neighbor(g, f, i, a, -1).unwrap_or(f[i]);
        let hi = neighbor(g, f, i, a, 1).unwrap_or(f[i]);
        *o = (hi - lo) / (2.0 * g.h);
    }
    Vec3::from(out)
}

fn curl_of(g: &Grid, v: &[Vec3]) -> Vec<Vec3> {
    par(g.len(), |i| {
        let d = |axis: usize| {
            let lo = neighbor(g, v, i, axis, -1).unwrap_or(v[i]);
            let hi = neighbor(g, v, i, axis, 1).unwrap_or(v[i]);
            (hi - lo) / (2.0 * g.h)
        };
        let (dx, dy, dz) = (d(0), d(1), d(2));
        Vec3::new(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x)
    })
}

/// Closed walls (and the floor) stop flow through them; solid cells don't move.
fn enforce_walls(g: &Grid, s: &FluidSettings, solid: &[bool], v: &mut [Vec3]) {
    for i in 0..v.len() {
        if solid[i] {
            v[i] = Vec3::ZERO;
            continue;
        }
        let (x, y, z) = g.xyz(i);
        if y == 0 && v[i].y < 0.0 {
            v[i].y = 0.0;
        }
        if !s.open {
            if (x == 0 && v[i].x < 0.0) || (x == g.n[0] - 1 && v[i].x > 0.0) {
                v[i].x = 0.0;
            }
            if y == g.n[1] - 1 && v[i].y > 0.0 {
                v[i].y = 0.0;
            }
            if (z == 0 && v[i].z < 0.0) || (z == g.n[2] - 1 && v[i].z > 0.0) {
                v[i].z = 0.0;
            }
        }
    }
}

/// Make the velocity divergence-free (except where sources expand): Jacobi pressure solve.
fn project(g: &Grid, s: &FluidSettings, solid: &[bool], v: &mut [Vec3], div_target: &[f32]) {
    let n = g.len();
    let h = g.h;
    let comp = |i: usize, axis: usize, dir: i64| -> f32 {
        match neighbor(g, v, i, axis, dir) {
            Some(_) if {
                let (x, y, z) = g.xyz(i);
                let mut c = [x as i64, y as i64, z as i64];
                c[axis] += dir;
                solid[g.idx(c[0] as usize, c[1] as usize, c[2] as usize)]
            } => 0.0,
            Some(q) => q[axis],
            // Outside: closed walls reflect (zero), open sides carry the flow on.
            None => {
                if s.open && !(axis == 1 && dir < 0) {
                    v[i][axis]
                } else {
                    0.0
                }
            }
        }
    };
    let div: Vec<f32> = par(n, |i| {
        if solid[i] {
            return 0.0;
        }
        let mut d = 0.0;
        for a in 0..3 {
            d += (comp(i, a, 1) - comp(i, a, -1)) / (2.0 * h);
        }
        d - div_target[i]
    });
    let mut p = vec![0.0f32; n];
    let iters = 40;
    for _ in 0..iters {
        let pn: Vec<f32> = par(n, |i| {
            if solid[i] {
                return 0.0;
            }
            let mut sum = 0.0;
            let mut cnt = 0.0;
            for a in 0..3 {
                for dir in [-1i64, 1] {
                    let (x, y, z) = g.xyz(i);
                    let mut c = [x as i64, y as i64, z as i64];
                    c[a] += dir;
                    if c[a] < 0 || c[a] >= g.n[a] as i64 {
                        // Open sides: p = 0 outside (Dirichlet). Closed / floor: mirror.
                        if s.open && !(a == 1 && dir < 0) {
                            cnt += 1.0;
                        } else {
                            sum += p[i];
                            cnt += 1.0;
                        }
                        continue;
                    }
                    let j = g.idx(c[0] as usize, c[1] as usize, c[2] as usize);
                    sum += if solid[j] { p[i] } else { p[j] };
                    cnt += 1.0;
                }
            }
            (sum - div[i] * h * h) / cnt
        });
        p = pn;
    }
    let grad: Vec<Vec3> = par(n, |i| {
        if solid[i] {
            return Vec3::ZERO;
        }
        let mut out = [0.0; 3];
        for (a, o) in out.iter_mut().enumerate() {
            let side = |dir: i64| -> f32 {
                let (x, y, z) = g.xyz(i);
                let mut c = [x as i64, y as i64, z as i64];
                c[a] += dir;
                if c[a] < 0 || c[a] >= g.n[a] as i64 {
                    return if s.open && !(a == 1 && dir < 0) { 0.0 } else { p[i] };
                }
                let j = g.idx(c[0] as usize, c[1] as usize, c[2] as usize);
                if solid[j] { p[i] } else { p[j] }
            };
            *o = (side(1) - side(-1)) / (2.0 * h);
        }
        Vec3::from(out)
    });
    for (vi, gi) in v.iter_mut().zip(&grad) {
        *vi -= *gi;
    }
    enforce_walls(g, s, solid, v);
}

/// Cells blocked by colliders (near or inside their surfaces).
fn solid_cells(g: &Grid, world: &Mat4, scale: f32, grid: &TriGrid) -> Vec<bool> {
    if grid.is_empty() {
        return vec![false; g.len()];
    }
    let hw = g.h * scale;
    par(g.len(), |i| {
        let pw = world.transform_point3(g.center(i));
        let mut scratch = Vec::new();
        match grid.closest(pw, hw * 3.0, &mut scratch) {
            Some((q, n)) => {
                let d = pw - q;
                d.length() < hw * 0.75 || d.dot(n) < 0.0
            }
            None => false,
        }
    })
}

fn fluid_key(scene: &Scene, id: NodeId, s: &FluidSettings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.sim_key().hash(&mut h);
    scene.fps.to_bits().hash(&mut h);
    crate::fx::hash_fields(scene, id, &mut h);
    crate::fx::motion_hash(scene, id, &mut h);
    for c in s.sources.iter().map(|x| x.node).chain(s.colliders.iter().copied()) {
        crate::fx::motion_hash(scene, c, &mut h);
        if let Some(n) = scene.node(c) {
            n.mesh_rev().hash(&mut h);
            if let Shape::Particles(p) = &n.shape {
                p.cache.key.hash(&mut h);
            }
            if let Some(sk) = &n.skin {
                for j in &sk.joints {
                    crate::fx::motion_hash(scene, *j, &mut h);
                }
            }
        }
    }
    h.finish()
}

/// Bring every fluid container to the current time (called from `rig::evaluate`).
pub fn update(scene: &mut Scene) {
    let jobs: Vec<NodeId> = scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Fluid(_))).map(|n| n.id).collect();
    let time = scene.time;
    let fps = scene.fps;
    for id in jobs {
        let Some(Shape::Fluid(fl)) = scene.node(id).map(|n| &n.shape) else { continue };
        let s = fl.settings.clone();
        let key = fluid_key(scene, id, &s);
        let mut cache = (*fl.cache).clone();
        let (base, frac) = crate::fx::sub_frame(time, s.start);
        let target = if frac > 0.0 { (base + 1).min(MAX_FRAMES) } else { base };
        let grid = Grid::new(&s);
        let stale = cache.key != key || cache.frames.is_empty() || cache.frames[0].dims != grid.n;
        if stale {
            let empty = State { v: vec![Vec3::ZERO; grid.len()], d: vec![0.0; grid.len()], t: vec![0.0; grid.len()] };
            cache = SimCache { key, frames: vec![Arc::new(FluidFrame::from_state(&empty, grid.n, true))], current: None, version: cache.version.wrapping_add(1), blend: None, shift: 0.0 };
        }
        if target + 1 > cache.frames.len() {
            let mut keep = vec![id];
            keep.extend(s.sources.iter().map(|x| x.node).chain(s.colliders.iter().copied()));
            let mut tr = Tracks::new(scene, &keep);
            let mut prev_tr = Tracks::new(scene, &keep);
            let mut samples: Vec<Option<Vec<SurfaceSample>>> = vec![None; s.sources.len()];
            let mut state = cache.frames.last().and_then(|f| f.state.clone()).map(|a| (*a).clone()).unwrap_or_default();
            for k in cache.frames.len()..=target {
                let f = s.start + k as f32;
                tr.goto(f);
                prev_tr.goto(f - 1.0);
                let world = tr.world(id);
                let inv = world.inverse();
                let w3 = Mat3::from_mat4(world);
                let inv3 = Mat3::from_mat4(inv);
                let scale = ((w3.x_axis.length() + w3.y_axis.length() + w3.z_axis.length()) / 3.0).max(1e-6);
                let emits: Vec<Emit> = s.sources.iter().zip(samples.iter_mut()).filter_map(|(src, smp)| emit_points(scene, &tr, Some(&prev_tr), src, f, fps, grid.h * scale * 0.7, smp)).collect();
                let solid = solid_cells(&grid, &world, scale, &tr.colliders(&s.colliders));
                let fr = Frame { grid, s: &s, world, inv, inv3, w3, scale, fields: tr.fields(id), emits, solid, time: (k as f32 - 1.0).max(0.0) / fps.max(1.0) };
                state = step_frame(&state, &fr, fps);
                // Only the newest frame keeps the full state.
                if let Some(last) = cache.frames.last_mut()
                    && last.state.is_some()
                {
                    *last = Arc::new(FluidFrame { state: None, ..(**last).clone() });
                }
                cache.frames.push(Arc::new(FluidFrame::from_state(&state, grid.n, true)));
            }
        }
        let cur = base.min(cache.frames.len() - 1);
        let frac = if cur + 1 < cache.frames.len() { frac } else { 0.0 };
        if cache.current != Some(cur) || cache.shift != frac {
            cache.current = Some(cur);
            cache.shift = frac;
            cache.blend = (frac > 0.0).then(|| Arc::new(cache.frames[cur].lerp(&cache.frames[cur + 1], frac)));
            cache.version = cache.version.wrapping_add(1);
        }
        if let Some(Shape::Fluid(f)) = scene.node_mut(id).map(|n| &mut n.shape) {
            f.cache = Arc::new(cache);
        }
    }
}

// ------------------------------------------------------------------------------ drawing / rendering

/// What the renderers need from a container at the current time (world placement + grids).
#[derive(Clone, Debug)]
pub struct VolumeGeom {
    pub node: NodeId,
    pub world: Mat4,
    pub dims: [usize; 3],
    pub h: f32,
    pub density: Vec<f32>,
    pub temp: Vec<f32>,
    pub smoke_color: [f32; 3],
    pub density_scale: f32,
    pub anisotropy: f32,
    pub fire_intensity: f32,
    pub fire_color: [f32; 3],
    pub fire_temp: f32,
}

impl VolumeGeom {
    pub fn from_fluid(node: NodeId, world: Mat4, f: &Fluid) -> Option<VolumeGeom> {
        let fr = f.current()?;
        let (dims, h) = f.settings.dims();
        if fr.dims != dims || (fr.dmax <= 0.0 && fr.tmax <= 0.0) {
            return None;
        }
        let s = &f.settings;
        Some(VolumeGeom {
            node,
            world,
            dims,
            h,
            density: fr.densities(),
            temp: fr.temps(),
            smoke_color: s.smoke_color,
            density_scale: s.density_scale,
            anisotropy: s.anisotropy,
            fire_intensity: s.fire_intensity,
            fire_color: s.fire_color,
            fire_temp: s.fire_temp,
        })
    }
}

/// Fire colour for a temperature: deep red near the threshold, the fire colour at full heat,
/// toward yellow-white above (linear radiance per unit length).
pub fn fire_emission(temp: f32, fire_temp: f32, color: [f32; 3], intensity: f32) -> Vec3 {
    if intensity <= 0.0 || temp <= fire_temp {
        return Vec3::ZERO;
    }
    let x = ((temp - fire_temp) / (1.0 - fire_temp).max(0.05)).max(0.0);
    let c = Vec3::from(color);
    let deep = Vec3::new(0.6, 0.06, 0.0);
    let hot = Vec3::new(1.0, 0.85, 0.55);
    let col = if x < 1.0 { deep.lerp(c, x) } else { c.lerp(hot, ((x - 1.0) * 0.5).min(1.0)) };
    let k = x.min(1.0);
    col * (intensity * k * k * (1.0 + (x - 1.0).max(0.0)))
}

/// Container box edges (local space).
pub fn box_lines(s: &FluidSettings) -> Vec<(Vec3, Vec3)> {
    let (n, h) = s.dims();
    let e = Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) * h * 0.5;
    let c = |x: f32, y: f32, z: f32| Vec3::new(x * e.x, y * e.y, z * e.z);
    let mut out = Vec::new();
    for &(a, b) in &[(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
        out.push((c(-1.0, a, b), c(1.0, a, b)));
        out.push((c(a, -1.0, b), c(a, 1.0, b)));
        out.push((c(a, b, -1.0), c(a, b, 1.0)));
    }
    out
}

/// Viewport preview: translucent faces of the smoke at half resolution (world space):
/// (position, normal, rgba).
pub fn preview_tris(f: &Fluid, world: Mat4) -> Vec<(Vec3, Vec3, [f32; 4])> {
    let Some(fr) = f.current() else { return Vec::new() };
    let s = &f.settings;
    let (n, h) = s.dims();
    if fr.dims != n || fr.dmax <= 0.0 && fr.tmax <= 0.0 {
        return Vec::new();
    }
    let g = Grid { n, h, origin: -Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) * h * 0.5 + Vec3::splat(h * 0.5) };
    // Half-resolution blocks.
    let b = [n[0].div_ceil(2), n[1].div_ceil(2), n[2].div_ceil(2)];
    let bidx = |x: usize, y: usize, z: usize| x + b[0] * (y + b[1] * z);
    let mut dens = vec![0.0f32; b[0] * b[1] * b[2]];
    let mut temp = vec![0.0f32; dens.len()];
    for i in 0..g.len() {
        let (x, y, z) = g.xyz(i);
        let k = bidx(x / 2, y / 2, z / 2);
        dens[k] += fr.density_at(i) / 8.0;
        temp[k] = temp[k].max(fr.temp_at(i));
    }
    let glow = |t: f32| s.fire_intensity > 0.0 && t > s.fire_temp;
    let thresh = 0.04 / (s.density_scale.max(0.1) * h * 2.0).max(1e-3);
    let on = |k: usize| dens[k] > thresh || glow(temp[k]);
    let bh = h * 2.0;
    let corner = g.origin - Vec3::splat(h * 0.5);
    let n3 = Mat3::from_mat4(world).inverse().transpose();
    let smoke = Vec3::from(s.smoke_color);
    let mut out = Vec::new();
    for z in 0..b[2] {
        for y in 0..b[1] {
            for x in 0..b[0] {
                let k = bidx(x, y, z);
                if !on(k) {
                    continue;
                }
                let (col, a) = if glow(temp[k]) {
                    let e = fire_emission(temp[k], s.fire_temp, s.fire_color, 1.0);
                    ((e / e.max_element().max(1e-3)).min(Vec3::ONE), 0.55)
                } else {
                    let op = 1.0 - (-dens[k] * s.density_scale * bh).exp();
                    (smoke * (0.6 + 0.4 * op), (0.12 + 0.6 * op).min(0.7))
                };
                let lo = corner + Vec3::new(x as f32, y as f32, z as f32) * bh;
                for axis in 0..3 {
                    for dir in [-1i64, 1] {
                        let mut c = [x as i64, y as i64, z as i64];
                        c[axis] += dir;
                        let outside = c[axis] < 0 || c[axis] >= b[axis] as i64;
                        if !outside && on(bidx(c[0] as usize, c[1] as usize, c[2] as usize)) {
                            continue;
                        }
                        let mut nrm = Vec3::ZERO;
                        nrm[axis] = dir as f32;
                        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                        let mut base = lo;
                        if dir > 0 {
                            base[axis] += bh;
                        }
                        let mut du = Vec3::ZERO;
                        du[u] = bh;
                        let mut dv = Vec3::ZERO;
                        dv[v] = bh;
                        let q = [base, base + du, base + du + dv, base + dv];
                        let q = q.map(|p| world.transform_point3(p));
                        let nw = (n3 * nrm).normalize_or(nrm);
                        let rgba = [col.x, col.y, col.z, a];
                        let quad = if dir > 0 { [q[0], q[1], q[2], q[0], q[2], q[3]] } else { [q[0], q[2], q[1], q[0], q[3], q[2]] };
                        for p in quad {
                            out.push((p, nw, rgba));
                        }
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------ app actions

impl App {
    /// Create a fluid container (at the selection's centre when something is selected).
    pub fn create_fluid(&mut self, s: FluidSettings, at: Option<Vec3>, name: Option<&str>) -> NodeId {
        let mut s = s;
        s.start = self.scene.play_start;
        self.checkpoint();
        let id = self.scene.add_node("fluid1", Shape::Fluid(Fluid { settings: s, cache: Default::default() }), None);
        self.apply_name(id, name);
        if let Some(p) = at
            && let Some(n) = self.scene.node_mut(id)
        {
            n.translate = p;
        }
        self.sel = vec![id];
        id
    }

    /// Add (or replace) an emitter source on a container.
    pub fn add_fluid_source(&mut self, fluid: NodeId, src: FluidSource) -> Result<(), String> {
        if !matches!(self.scene.node(fluid).map(|n| &n.shape), Some(Shape::Fluid(_))) {
            return Err("fluidEmitter: not a fluid container.".into());
        }
        if src.node == fluid {
            return Err("fluidEmitter: a container can't emit into itself.".into());
        }
        self.scene.node(src.node).ok_or("fluidEmitter: no such source")?;
        self.checkpoint();
        if let Some(Shape::Fluid(f)) = self.scene.node_mut(fluid).map(|n| &mut n.shape) {
            f.settings.sources.retain(|s| s.node != src.node);
            f.settings.sources.push(src);
        }
        Ok(())
    }

    /// The fluid containers among the selection, else all of them.
    pub fn target_fluids(&self) -> Vec<NodeId> {
        let sel: Vec<NodeId> = self.sel.iter().copied().filter(|i| matches!(self.scene.node(*i).map(|n| &n.shape), Some(Shape::Fluid(_)))).collect();
        if !sel.is_empty() {
            return sel;
        }
        self.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Fluid(_))).map(|n| n.id).collect()
    }

    /// A ready-to-play smoke / fire / explosion setup: a source (the given node, or a new
    /// sphere / locator) inside a container sized to it.
    pub fn fluid_preset(&mut self, kind: &str, source: Option<NodeId>, name: Option<&str>) -> Result<NodeId, String> {
        let base = match source {
            Some(id) => self
                .world_bounds(&[id])
                .filter(|(a, b)| (*b - *a).max_element() > 1e-4)
                .map(|(a, b)| ((a + b) * 0.5, (b - a).max_element().max(0.2)))
                .unwrap_or((self.scene.world_matrix(id).transform_point3(Vec3::ZERO), 0.0)),
            None => (Vec3::ZERO, 0.0),
        };
        let (c, extent) = base;
        let mut s = FluidSettings::default();
        let mut src = FluidSource::default();
        let make_src: &str;
        let ground = (c.y - extent * 0.5).min(c.y);
        let (size, centre) = match kind {
            "smoke" => {
                src.density = 5.0;
                src.heat = 4.0;
                src.radius = 0.45 * extent.max(1.0);
                make_src = "locator";
                s.vorticity = 3.0;
                s.turbulence = 1.5;
                ([4.0 * extent.max(1.0), 7.0 * extent.max(1.0), 4.0 * extent.max(1.0)], Vec3::new(c.x, ground + 3.4 * extent.max(1.0), c.z))
            }
            "fire" | "campfire" | "torch" => {
                src.density = 1.2;
                src.heat = 8.0;
                src.radius = 0.35 * extent.max(1.0);
                src.speed = 0.5;
                s.cooling = 1.6;
                s.buoyancy = 5.0;
                s.vorticity = 3.0;
                s.turbulence = 2.0;
                s.turbulence_freq = 3.0;
                s.fire_intensity = 6.0;
                s.density_scale = 3.0;
                s.smoke_color = [0.25, 0.24, 0.24];
                make_src = "locator";
                ([3.0 * extent.max(1.0), 5.0 * extent.max(1.0), 3.0 * extent.max(1.0)], Vec3::new(c.x, ground + 2.4 * extent.max(1.0), c.z))
            }
            "explosion" => {
                src.density = 40.0;
                src.heat = 40.0;
                src.radius = 0.6 * extent.max(1.0);
                src.duration = 0.15;
                src.expansion = 6.0;
                src.inherit = 0.0;
                s.cooling = 2.2;
                s.dissipation = 0.2;
                s.buoyancy = 3.0;
                s.vorticity = 4.0;
                s.turbulence = 3.0;
                s.fire_intensity = 2.5;
                s.fire_temp = 0.6;
                s.res = 64;
                s.density_scale = 2.5;
                s.smoke_color = [0.3, 0.29, 0.28];
                make_src = "sphere";
                ([9.0 * extent.max(1.0), 11.0 * extent.max(1.0), 9.0 * extent.max(1.0)], Vec3::new(c.x, ground + 5.3 * extent.max(1.0), c.z))
            }
            other => return Err(format!("fluid preset '{other}': use smoke, fire or explosion")),
        };
        s.size = size;
        let src_node = match source {
            Some(id) => id,
            None => {
                let at = Vec3::new(c.x, ground + 0.3, c.z);
                let id = match make_src {
                    "sphere" => {
                        crate::commands::execute(self, &format!("polySphere -r 0.4 -n {kind}Source"))?;
                        let id = self.sel[0];
                        // A hidden source: only the smoke shows.
                        if let Some(n) = self.scene.node_mut(id) {
                            n.visible = false;
                        }
                        id
                    }
                    _ => {
                        crate::commands::execute(self, &format!("spaceLocator -n {kind}Source"))?;
                        self.sel[0]
                    }
                };
                if let Some(n) = self.scene.node_mut(id) {
                    n.translate = at;
                }
                id
            }
        };
        src.node = src_node;
        let id = self.create_fluid(s, Some(centre), name.or(Some(&format!("{kind}Fluid"))));
        self.add_fluid_source(id, src)?;
        self.sel = vec![id];
        Ok(id)
    }
}

impl App {
    /// OpenVDB grids (density, temperature) of a container at the current time, in world
    /// space.
    pub fn fluid_vdb_bytes(&self, id: NodeId) -> Option<Vec<u8>> {
        vdb_for(&self.scene, id, true)
    }
}

/// OpenVDB file image of a container's current density and temperature, in world space or
/// in the container's own space (for formats that carry the transform themselves).
pub fn vdb_for(scene: &Scene, id: NodeId, world_space: bool) -> Option<Vec<u8>> {
    {
        let Some(Shape::Fluid(f)) = scene.node(id).map(|n| &n.shape) else { return None };
        let fr = f.current()?;
        let (n, h) = f.settings.dims();
        if fr.dims != n {
            return None;
        }
        let origin = -Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) * h * 0.5 + Vec3::splat(h * 0.5);
        let place = if world_space { scene.world_matrix(id) } else { Mat4::IDENTITY };
        let m = place * Mat4::from_translation(origin) * Mat4::from_scale(Vec3::splat(h));
        let m = glam::DMat4::from_cols_array(&m.to_cols_array().map(|v| v as f64));
        let (d, t) = (fr.densities(), fr.temps());
        let grids = [
            crate::export::vdb::VdbGrid { name: "density", dims: n, values: &d, index_to_world: m, class: "fog volume" },
            crate::export::vdb::VdbGrid { name: "temperature", dims: n, values: &t, index_to_world: m, class: "unknown" },
        ];
        crate::export::vdb::vdb_bytes(&grids).ok()
    }
}

impl App {
    /// Write `<dir>/<container>.####.vdb` for frames start..=end.
    pub fn export_vdb_sequence(&mut self, ids: &[NodeId], dir: &std::path::Path, start: i32, end: i32) -> Result<Vec<String>, String> {
        let ids: Vec<NodeId> = ids.iter().copied().filter(|i| matches!(self.scene.node(*i).map(|n| &n.shape), Some(Shape::Fluid(_)))).collect();
        if ids.is_empty() {
            return Err("fluidExport: no fluid containers".into());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let t0 = self.scene.time;
        let mut files = Vec::new();
        let res = (|| {
            for f in start..=end.max(start) {
                self.set_time(f as f32);
                for &id in &ids {
                    let name = self.scene.node(id).map(|n| n.name.clone()).unwrap_or_default();
                    let path = dir.join(format!("{name}.{f:04}.vdb"));
                    let bytes = self.fluid_vdb_bytes(id).ok_or("fluidExport: nothing simulated")?;
                    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                    files.push(path.to_string_lossy().into_owned());
                }
            }
            Ok::<(), String>(())
        })();
        self.set_time(t0);
        res.map(|_| files)
    }
}

/// `setAttr` / `getAttr` on containers.
pub fn get_attr(f: &Fluid, attr: &str) -> Option<String> {
    let s = &f.settings;
    Some(match attr {
        "res" | "resolution" => s.res.to_string(),
        "dims" => {
            let (n, _) = s.dims();
            format!("{} {} {}", n[0], n[1], n[2])
        }
        "voxels" => {
            let (n, _) = s.dims();
            (n[0] * n[1] * n[2]).to_string()
        }
        "maxDensity" => f.current().map(|c| c.dmax).unwrap_or(0.0).to_string(),
        "maxTemperature" => f.current().map(|c| c.tmax).unwrap_or(0.0).to_string(),
        "totalDensity" => {
            let (_, h) = s.dims();
            f.current().map(|c| c.densities().iter().sum::<f32>() * h * h * h).unwrap_or(0.0).to_string()
        }
        "sources" => s.sources.len().to_string(),
        "buoyancy" => s.buoyancy.to_string(),
        "fireIntensity" => s.fire_intensity.to_string(),
        "densityScale" => s.density_scale.to_string(),
        _ => return None,
    })
}

/// Fluid MEL. None = not one of these commands.
pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let mut t: Vec<String> = rest.to_vec();
    let take = |t: &mut Vec<String>, flags: &[&str]| -> Option<String> {
        let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
        let v = t.get(i + 1).cloned()?;
        t.drain(i..i + 2);
        Some(v)
    };
    let take3 = |t: &mut Vec<String>, flags: &[&str]| -> Option<Vec3> {
        let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
        let n: Vec<f32> = t.iter().skip(i + 1).take(3).filter_map(|x| x.parse().ok()).collect();
        if n.len() < 3 {
            return None;
        }
        t.drain(i..i + 4);
        Some(Vec3::new(n[0], n[1], n[2]))
    };
    let num = |v: Option<String>| v.and_then(|s| s.parse::<f32>().ok());
    let name_of = |app: &App, id: NodeId| app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default();
    let r = match cmd.as_str() {
        "fluidContainer" | "create3DContainer" => (|| {
            let mut s = FluidSettings::default();
            if let Some(v) = num(take(&mut t, &["res", "r", "resolution"])) {
                s.res = (v as u32).clamp(4, 256);
            }
            if let Some(v) = take3(&mut t, &["size", "s"]) {
                s.size = v.max(Vec3::splat(0.01)).to_array();
            }
            let at = take3(&mut t, &["p", "pos", "position"]);
            let name = take(&mut t, &["n", "name"]);
            let id = app.create_fluid(s, at, name.as_deref());
            Ok(name_of(app, id))
        })(),
        "fluidEmitter" => (|| {
            // fluidEmitter [-dens 4 -heat 2 -r 0.3 -spd 0 -inherit 0.5 -dur 0 -exp 0] source [fluid]
            let mut src = FluidSource::default();
            if let Some(v) = num(take(&mut t, &["dens", "density", "d"])) {
                src.density = v.max(0.0);
            }
            if let Some(v) = num(take(&mut t, &["heat", "temperature", "t"])) {
                src.heat = v.max(0.0);
            }
            if let Some(v) = num(take(&mut t, &["r", "radius"])) {
                src.radius = v.max(1e-3);
            }
            if let Some(v) = num(take(&mut t, &["spd", "speed"])) {
                src.speed = v;
            }
            if let Some(v) = num(take(&mut t, &["inherit", "inh"])) {
                src.inherit = v.clamp(0.0, 1.0);
            }
            if let Some(v) = num(take(&mut t, &["dur", "duration"])) {
                src.duration = v.max(0.0);
            }
            if let Some(v) = num(take(&mut t, &["exp", "expansion"])) {
                src.expansion = v.max(0.0);
            }
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let is_fluid = |app: &App, i: NodeId| matches!(app.scene.node(i).map(|n| &n.shape), Some(Shape::Fluid(_)));
            let fluid = ids.iter().copied().find(|&i| is_fluid(app, i)).or_else(|| {
                let all: Vec<NodeId> = app.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Fluid(_))).map(|n| n.id).collect();
                (all.len() == 1).then(|| all[0])
            });
            let fluid = fluid.ok_or("fluidEmitter: name the container (fluidEmitter source fluid1) or create one first")?;
            let sources: Vec<NodeId> = ids.into_iter().filter(|&i| !is_fluid(app, i)).collect();
            if sources.is_empty() {
                return Err("fluidEmitter: select or name a source object (mesh, particles, locator)".into());
            }
            for s in &sources {
                app.add_fluid_source(fluid, FluidSource { node: *s, ..src.clone() })?;
            }
            Ok(format!("{} source(s) → {}", sources.len(), name_of(app, fluid)))
        })(),
        "fluidPreset" => (|| {
            let name = take(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let (kind, rest) = objs.split_first().ok_or("fluidPreset smoke|fire|explosion [source]")?;
            let src = match rest.first() {
                Some(n) => Some(app.scene.find_by_name(n).ok_or_else(|| format!("No object matches name: {n}"))?),
                None => None,
            };
            let id = app.fluid_preset(kind, src, name.as_deref())?;
            Ok(name_of(app, id))
        })(),
        "fluidExport" | "exportVdb" => (|| {
            // fluidExport -dir C:/path [-s 1 -e 48] [fluid]
            let dir = take(&mut t, &["dir", "d", "o"]).map(|d| d.trim_matches('"').to_string()).ok_or("fluidExport -dir C:/path/folder [-s 1 -e 48] [fluid]")?;
            let st = num(take(&mut t, &["s", "st", "start"]));
            let en = num(take(&mut t, &["e", "end"]));
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = if objs.is_empty() { app.target_fluids() } else { crate::commands::targets(app, &objs)? };
            let st = st.unwrap_or(app.scene.play_start) as i32;
            let en = en.unwrap_or(app.scene.play_end) as i32;
            let files = app.export_vdb_sequence(&ids, std::path::Path::new(&dir), st, en)?;
            Ok(format!("{} VDB files → {dir}", files.len()))
        })(),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    fn fluid(app: &App, name: &str) -> Fluid {
        match &app.scene.node(app.scene.find_by_name(name).unwrap()).unwrap().shape {
            Shape::Fluid(f) => f.clone(),
            _ => panic!("not a fluid"),
        }
    }

    /// (total density, density-weighted mean height in local units).
    fn stats(f: &Fluid) -> (f32, f32) {
        let fr = f.current().unwrap();
        let g = Grid::new(&f.settings);
        let (mut tot, mut y) = (0.0, 0.0);
        for i in 0..g.len() {
            let d = fr.density_at(i);
            tot += d;
            y += d * g.center(i).y;
        }
        (tot * g.h.powi(3), if tot > 0.0 { y / tot } else { 0.0 })
    }

    #[test]
    fn smoke_rises_and_caches() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "fluidContainer -res 24 -size 2 4 2 -p 0 2 0 -n box; spaceLocator -n src; move 0 0.4 0 src; fluidEmitter -r 0.3 src box").unwrap();
        let t = std::time::Instant::now();
        app.set_time(12.0);
        let (d12, y12) = stats(&fluid(&app, "box"));
        app.set_time(30.0);
        let (d30, y30) = stats(&fluid(&app, "box"));
        eprintln!("30 frames in {:.2}s: total {d12:.3} → {d30:.3}, height {y12:.2} → {y30:.2}", t.elapsed().as_secs_f32());
        assert!(d12 > 0.01 && d30 > d12 * 0.8, "smoke accumulates");
        assert!(y30 > y12 + 0.2, "hot smoke rises");
        assert!(fluid(&app, "box").current().unwrap().tmax > 0.0);
        // Cached: going back is instant and identical.
        let a = fluid(&app, "box").current().unwrap().density.clone();
        app.set_time(12.0);
        app.set_time(30.0);
        assert_eq!(fluid(&app, "box").current().unwrap().density, a);
        // Only the newest frame keeps the solver state.
        let f = fluid(&app, "box");
        assert_eq!(f.cache.frames.iter().filter(|x| x.state.is_some()).count(), 1);
        // A look attribute doesn't re-simulate; a sim attribute does.
        let key = f.cache.key;
        execute(&mut app, "setAttr box.densityScale 9").unwrap();
        app.set_time(30.0);
        assert_eq!(fluid(&app, "box").cache.key, key);
        execute(&mut app, "setAttr box.buoyancy 0").unwrap();
        app.set_time(30.0);
        assert_ne!(fluid(&app, "box").cache.key, key);
        assert_eq!(execute(&mut app, "getAttr box.dims").unwrap(), "12 24 12");
    }

    #[test]
    fn colliders_block_and_particles_emit() {
        let mut app = App::new();
        app.scene.nodes.clear();
        // Smoke from a locator under a plate: with the plate as a collider less smoke gets above it.
        execute(&mut app, "fluidContainer -res 24 -size 3 4 3 -p 0 2 0 -n box; spaceLocator -n src; move 0 0.4 0 src; fluidEmitter -r 0.4 -heat 8 src box; polyCube -w 2.4 -h 0.2 -d 2.4 -n plate; move 0 1.6 0 plate").unwrap();
        let above = |app: &App| {
            let f = fluid(app, "box");
            let g = Grid::new(&f.settings);
            let fr = f.current().unwrap();
            (0..g.len()).filter(|&i| g.center(i).y > 0.0 && g.center(i).x.abs() < 0.8 && g.center(i).z.abs() < 0.8).map(|i| fr.density_at(i)).sum::<f32>()
        };
        app.set_time(36.0);
        let free = above(&app);
        execute(&mut app, "connectDynamic -c plate box").unwrap();
        app.set_time(36.0);
        let blocked = above(&app);
        eprintln!("smoke above the plate: free {free:.2}, with collider {blocked:.2}");
        assert!(free > 0.5 && blocked < free * 0.5);
        assert!(fluid(&app, "box").current().unwrap().dmax > 0.1, "the smoke still exists, under / around the plate");

        // Particles as a source.
        execute(&mut app, "emitter -type omni -r 60 -spd 1 -n spray; move 0 1 0 spray; fluidContainer -res 16 -size 4 4 4 -p 0 1 0 -n box2; fluidEmitter -heat 0 -r 0.2 spray box2").unwrap();
        app.set_time(20.0);
        let f = fluid(&app, "box2");
        assert!(f.current().unwrap().dmax > 0.0, "particles emit smoke");
        assert_eq!(f.settings.sources.len(), 1);
        // Errors.
        assert!(execute(&mut app, "fluidEmitter box2").is_err());
        assert!(execute(&mut app, "fluidPreset lava").is_err());
        assert!(execute(&mut app, "setAttr box2.bogus 1").is_err());
    }

    #[test]
    fn presets_render_as_volumes() {
        use crate::raytrace::{Pass, RenderSettings};
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 10 -h 10 -n floor; fluidPreset fire -n camp; camera -n cam -p 0 2 7 -aim 0 1.2 0").unwrap();
        // Only the fire lights the scene (no lights, no sky): the floor near it must glow.
        app.set_time(30.0);
        let s = RenderSettings { width: 64, height: 48, spp: 16, bounces: 2, denoise: false, env: 0.0, ..Default::default() };
        let job = app.render_now(Some("cam"), s.clone()).unwrap();
        let em = job.pass_linear(Pass::Emission);
        let dd = job.pass_linear(Pass::DiffuseDirect);
        let sum = |v: &[[f32; 4]]| v.iter().map(|p| p[0] + p[1] + p[2]).sum::<f32>();
        eprintln!("fire: emission {:.1}, floor lit {:.1}", sum(&em), sum(&dd));
        assert!(sum(&em) > 1.0, "flames are visible");
        assert!(sum(&dd) > 0.5, "the fire lights the floor");
        // The container is a matte-able object.
        let sets = app.matte_sets(&job, "camp").unwrap();
        assert!(job.matte(&sets[0].1).iter().sum::<f32>() > 1.0);
        // Smoke blocks the background and casts a shadow.
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 12 -h 12 -n floor; fluidPreset smoke -n plume; camera -n cam -p 0 3 10 -aim 0 2 0; directionalLight -n sun -i 1 -rot -35 60 0").unwrap();
        app.set_time(40.0);
        let s = RenderSettings { width: 48, height: 48, spp: 16, bounces: 1, denoise: false, env: 0.0, ..Default::default() };
        let job = app.render_now(Some("cam"), s.clone()).unwrap();
        let lit = job.linear();
        let covered = lit.iter().filter(|p| p[3] > 0.0).count();
        assert!(covered > 100);
        // Shadow: the visible floor (not behind smoke) is darker than with the smoke hidden.
        let sets = app.matte_sets(&job, "plume,floor").unwrap();
        let (pm, fm) = (job.matte(&sets[0].1), job.matte(&sets[1].1));
        let clear: Vec<usize> = (0..lit.len()).filter(|&i| pm[i] < 0.01 && fm[i] > 0.99).collect();
        assert!(clear.len() > 100);
        let floor_sum = |img: &[[f32; 4]]| clear.iter().map(|&i| img[i][0]).sum::<f32>();
        let with = floor_sum(&lit);
        let plume = app.scene.find_by_name("plume").unwrap();
        app.scene.node_mut(plume).unwrap().visible = false;
        let without = floor_sum(&app.render_now(Some("cam"), s).unwrap().linear());
        eprintln!("visible floor: {with:.2} with smoke vs {without:.2} without");
        assert!(with < without * 0.98, "smoke shadows the floor");
    }

    #[test]
    fn vdb_files_and_usd_volumes() {
        let dir = std::env::temp_dir().join(format!("rm_vdb_{}", std::process::id()));
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "fluidContainer -res 20 -size 2 3 2 -p 0 1.5 0 -n box; spaceLocator -n src; move 0 0.3 0 src; fluidEmitter src box").unwrap();
        let files = app.export_vdb_sequence(&app.target_fluids(), &dir, 9, 10).unwrap();
        assert_eq!(files.len(), 2);
        let b = std::fs::read(&files[1]).unwrap();
        assert_eq!(&b[0..8], &0x5644_4220i64.to_le_bytes());
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 224);
        // Grid descriptors: density then temperature.
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("density") && s.contains("temperature") && s.contains("Tree_float_5_4_3") && s.contains("AffineMap"));
        // Optional cross-check with OpenVDB's Python module when present.
        if std::path::Path::new("/usr/bin/python3.12").exists() {
            let out = std::process::Command::new("/usr/bin/python3.12")
                .args(["-c", &format!("import pyopenvdb as v; g=v.read(r'{}','density'); print(g.activeVoxelCount(), g.evalMinMax()[1])", files[1])])
                .output();
            if let Ok(o) = out
                && o.status.success()
            {
                let txt = String::from_utf8_lossy(&o.stdout);
                eprintln!("pyopenvdb: {txt}");
                let n: usize = txt.split_whitespace().next().unwrap().parse().unwrap();
                assert!(n > 10);
            }
        }
        // USD: a Volume whose fields point at per-frame VDB files.
        execute(&mut app, "playbackOptions -min 1 -max 4").unwrap();
        let usd = dir.join("scene.usda");
        let mut opts = crate::export::ExportOptions::from_app(&app, false);
        opts.start = 1.0;
        opts.end = 4.0;
        crate::export::export_path(&app, &usd, &opts).unwrap();
        let text = std::fs::read_to_string(&usd).unwrap();
        assert!(text.contains("def Volume") && text.contains("def OpenVDBAsset \"density\"") && text.contains("rel field:density = <density>"));
        assert!(dir.join("scene_vdb/box.0004.vdb").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn director_fx_and_shot_parse() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "fxPreset fire -wind").unwrap();
        assert!(app.scene.find_by_name("fire").is_some() && app.scene.find_by_name("fireWind").is_some());
        let s = crate::director::parse_shot("a robot by a campfire at night with smoke");
        assert!(s.fx.contains(&"fire".to_string()) && s.fx.contains(&"smoke".to_string()), "{:?}", s.fx);
    }
}
