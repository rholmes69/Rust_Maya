//! A small CPU path tracer for the Render View: BVH over the scene triangles, a diffuse +
//! GGX microfacet material (Maya colour / specular / roughness / metalness / incandescence /
//! transparency / file textures), next-event estimation for directional, point and spot
//! lights, an ambient/sky environment, Russian roulette and progressive multi-threaded
//! accumulation.

use crate::render::FrameGeometry;
use crate::texture::{TEX_SIZE, TexImage};
use glam::{Vec2, Vec3};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const PI: f32 = std::f32::consts::PI;
const EPS: f32 = 1e-4;

// ------------------------------------------------------------------------------ settings

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneMap {
    /// Clamp + sRGB gamma (matches the viewport).
    Srgb,
    /// ACES-style filmic curve.
    Filmic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    /// Samples per pixel.
    pub spp: u32,
    /// Maximum bounces (diffuse + glossy).
    pub bounces: u32,
    /// Sky dome intensity (0 = off).
    pub env: f32,
    /// Exposure in stops.
    pub exposure: f32,
    pub tonemap: ToneMap,
    /// Show the sky behind objects (otherwise black, alpha 0).
    pub env_background: bool,
    /// Indirect sample clamp (fights fireflies).
    pub clamp: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings { width: 960, height: 540, spp: 64, bounces: 4, env: 0.25, exposure: 0.0, tonemap: ToneMap::Srgb, env_background: false, clamp: 4.0 }
    }
}

impl RenderSettings {
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.width, self.height, self.spp, self.bounces, self.env.to_bits(), self.env_background, self.clamp.to_bits()).hash(&mut h);
        h.finish()
    }
}

// ------------------------------------------------------------------------------ camera

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RtCamera {
    pub eye: Vec3,
    pub fwd: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    /// tan(fov_y / 2) for perspective cameras.
    pub tan_half: f32,
    /// Half height of the view volume for orthographic cameras.
    pub ortho_half: Option<f32>,
}

impl RtCamera {
    pub fn from_view(c: &crate::camera::Camera) -> RtCamera {
        RtCamera {
            eye: c.eye(),
            fwd: c.forward(),
            right: c.right(),
            up: c.up(),
            tan_half: (c.fov_y.to_radians() * 0.5).tan(),
            ortho_half: c.is_ortho().then_some(c.ortho_half),
        }
    }

    /// Camera node: position and orientation from its world matrix (looks down local -Z).
    pub fn from_node(m: glam::Mat4, fov_y_deg: f32) -> RtCamera {
        let eye = m.transform_point3(Vec3::ZERO);
        let fwd = m.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
        let up0 = m.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
        let right = fwd.cross(up0).normalize_or(Vec3::X);
        let up = right.cross(fwd).normalize();
        RtCamera { eye, fwd, right, up, tan_half: (fov_y_deg.to_radians() * 0.5).tan(), ortho_half: None }
    }

    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in [self.eye, self.fwd, self.up] {
            v.to_array().map(f32::to_bits).hash(&mut h);
        }
        (self.tan_half.to_bits(), self.ortho_half.map(f32::to_bits)).hash(&mut h);
        h.finish()
    }

    /// Ray through normalized screen coords (sx, sy in -1..1, y up).
    fn ray(&self, sx: f32, sy: f32, aspect: f32) -> (Vec3, Vec3) {
        match self.ortho_half {
            Some(h) => {
                let o = self.eye + self.right * (sx * h * aspect) + self.up * (sy * h);
                (o, self.fwd)
            }
            None => {
                let d = self.fwd + self.right * (sx * self.tan_half * aspect) + self.up * (sy * self.tan_half);
                (self.eye, d.normalize())
            }
        }
    }
}

// ------------------------------------------------------------------------------ scene

#[derive(Clone, Copy, Debug)]
struct RtMaterial {
    /// Linear base colour.
    albedo: Vec3,
    spec: f32,
    rough: f32,
    metal: f32,
    emission: Vec3,
    transp: f32,
    /// Texture layer index (into `RtScene::textures`), -1 = none.
    tex: i32,
}

#[derive(Clone, Copy)]
struct Tri {
    v0: Vec3,
    e1: Vec3,
    e2: Vec3,
}

#[derive(Clone, Copy)]
struct TriShade {
    n: [Vec3; 3],
    uv: [Vec2; 3],
    has_uv: bool,
    mat: u32,
}

#[derive(Clone, Copy, Debug)]
enum RtLight {
    Dir { dir_to_light: Vec3, rad: Vec3 },
    Point { pos: Vec3, rad: Vec3 },
    Spot { pos: Vec3, axis: Vec3, cos_outer: f32, cos_inner: f32, rad: Vec3 },
}

#[derive(Clone, Copy, Default)]
struct BvhNode {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle index; inner: left child (right = left + 1).
    start: u32,
    /// Triangle count (0 = inner node).
    count: u32,
}

pub struct RtScene {
    tris: Vec<Tri>,
    shade: Vec<TriShade>,
    mats: Vec<RtMaterial>,
    nodes: Vec<BvhNode>,
    lights: Vec<RtLight>,
    /// Uniform ambient radiance (ambient lights).
    ambient: Vec3,
    textures: Vec<Arc<TexImage>>,
    /// Scene uses the default key light (no direct lights in the scene).
    pub default_light: bool,
    pub tri_count: usize,
}

fn srgb(c: [f32; 3]) -> Vec3 {
    Vec3::new(c[0].max(0.0).powf(2.2), c[1].max(0.0).powf(2.2), c[2].max(0.0).powf(2.2))
}

impl RtScene {
    /// Build from the viewport's world-space geometry (shaded triangles of visible meshes)
    /// and lights. `cam` positions the default key light when the scene has no lights.
    pub fn build(g: &FrameGeometry, cam: &RtCamera) -> RtScene {
        let mut tris = Vec::new();
        let mut shade = Vec::new();
        let mut mats: Vec<RtMaterial> = Vec::new();
        let mut mat_ids: std::collections::HashMap<[u32; 12], u32> = Default::default();
        for chunk in g.tris.iter().chain(&g.tris_transp) {
            for t in chunk.data.chunks_exact(3) {
                let (a, b, c) = (Vec3::from(t[0].pos), Vec3::from(t[1].pos), Vec3::from(t[2].pos));
                let e1 = b - a;
                let e2 = c - a;
                if e1.cross(e2).length_squared() < 1e-20 {
                    continue;
                }
                let v = &t[0];
                let key = [
                    v.col[0].to_bits(),
                    v.col[1].to_bits(),
                    v.col[2].to_bits(),
                    v.col[3].to_bits(),
                    v.mat[0].to_bits(),
                    v.mat[1].to_bits(),
                    v.mat[2].to_bits(),
                    v.mat[3].to_bits(),
                    v.emi[0].to_bits(),
                    v.emi[1].to_bits(),
                    v.emi[2].to_bits(),
                    0,
                ];
                let mid = *mat_ids.entry(key).or_insert_with(|| {
                    mats.push(RtMaterial {
                        albedo: srgb([v.col[0], v.col[1], v.col[2]]),
                        spec: v.mat[0].clamp(0.0, 1.0),
                        rough: v.mat[1].clamp(0.03, 1.0),
                        metal: v.mat[2].clamp(0.0, 1.0),
                        emission: srgb([v.emi[0], v.emi[1], v.emi[2]]),
                        transp: (1.0 - v.col[3]).clamp(0.0, 1.0),
                        tex: (v.mat[3] + 0.5) as i32 - 1,
                    });
                    mats.len() as u32 - 1
                });
                let fallback = e1.cross(e2).normalize();
                let n = |i: usize| Vec3::from(t[i].nrm).try_normalize().unwrap_or(fallback);
                tris.push(Tri { v0: a, e1, e2 });
                shade.push(TriShade {
                    n: [n(0), n(1), n(2)],
                    uv: [Vec2::from(t[0].uv), Vec2::from(t[1].uv), Vec2::from(t[2].uv)],
                    has_uv: t[0].emi[3] > 0.5,
                    mat: mid,
                });
            }
        }
        let mut lights = Vec::new();
        let mut ambient = Vec3::ZERO;
        for l in &g.lights {
            let kind = (l.pos[3] + 0.5) as i32;
            let col = Vec3::new(l.col[0], l.col[1], l.col[2]);
            // Maya intensity 1 lights a white Lambert to its colour: radiance scale π.
            let rad = col * PI;
            let dir = Vec3::new(l.dir[0], l.dir[1], l.dir[2]).normalize_or(Vec3::NEG_Y);
            let pos = Vec3::new(l.pos[0], l.pos[1], l.pos[2]);
            match kind {
                0 => ambient += col,
                1 => lights.push(RtLight::Dir { dir_to_light: -dir, rad }),
                2 => lights.push(RtLight::Point { pos, rad }),
                _ => lights.push(RtLight::Spot { pos, axis: dir, cos_outer: l.dir[3], cos_inner: l.col[3].max(l.dir[3] + 1e-4), rad }),
            }
        }
        let default_light = lights.is_empty();
        if default_light {
            // Key light over the camera's left shoulder, like Maya's default light.
            let to_light = (cam.up * 1.0 - cam.right * 0.6 - cam.fwd * 0.8).normalize();
            lights.push(RtLight::Dir { dir_to_light: to_light, rad: Vec3::splat(PI) });
        }
        let textures = g.textures.as_ref().map(|t| t.layers.clone()).unwrap_or_default();
        let tri_count = tris.len();
        let mut s = RtScene { tris, shade, mats, nodes: Vec::new(), lights, ambient, textures, default_light, tri_count };
        s.build_bvh();
        s
    }

    // -------------------------------------------------------------------------- BVH

    fn build_bvh(&mut self) {
        let n = self.tris.len();
        if n == 0 {
            return;
        }
        let bounds: Vec<(Vec3, Vec3)> = self
            .tris
            .iter()
            .map(|t| {
                let (b, c) = (t.v0 + t.e1, t.v0 + t.e2);
                (t.v0.min(b).min(c), t.v0.max(b).max(c))
            })
            .collect();
        let cent: Vec<Vec3> = bounds.iter().map(|(a, b)| (*a + *b) * 0.5).collect();
        let mut idx: Vec<u32> = (0..n as u32).collect();
        let mut nodes = vec![BvhNode::default()];
        // (node index, start, end)
        let mut stack = vec![(0usize, 0usize, n)];
        while let Some((ni, s, e)) = stack.pop() {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &i in &idx[s..e] {
                let (a, b) = bounds[i as usize];
                lo = lo.min(a);
                hi = hi.max(b);
                clo = clo.min(cent[i as usize]);
                chi = chi.max(cent[i as usize]);
            }
            nodes[ni].min = lo;
            nodes[ni].max = hi;
            let count = e - s;
            let ext = chi - clo;
            let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
            if count <= 4 || ext[axis] < 1e-9 {
                nodes[ni].start = s as u32;
                nodes[ni].count = count as u32;
                continue;
            }
            // Binned SAH along the longest centroid axis.
            const BINS: usize = 16;
            let mut bin_n = [0usize; BINS];
            let mut bin_b = [(Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); BINS];
            let scale = BINS as f32 / ext[axis];
            let bin_of = |i: u32| (((cent[i as usize][axis] - clo[axis]) * scale) as usize).min(BINS - 1);
            for &i in &idx[s..e] {
                let b = bin_of(i);
                bin_n[b] += 1;
                let (a, c) = bounds[i as usize];
                bin_b[b].0 = bin_b[b].0.min(a);
                bin_b[b].1 = bin_b[b].1.max(c);
            }
            let area = |lo: Vec3, hi: Vec3| {
                if lo.x > hi.x {
                    return 0.0;
                }
                let d = hi - lo;
                d.x * d.y + d.y * d.z + d.z * d.x
            };
            let mut left_cost = [0.0f32; BINS];
            let (mut l_lo, mut l_hi, mut l_n) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN), 0usize);
            for b in 0..BINS - 1 {
                l_lo = l_lo.min(bin_b[b].0);
                l_hi = l_hi.max(bin_b[b].1);
                l_n += bin_n[b];
                left_cost[b] = area(l_lo, l_hi) * l_n as f32;
            }
            let (mut best, mut best_cost) = (usize::MAX, f32::MAX);
            let (mut r_lo, mut r_hi, mut r_n) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN), 0usize);
            for b in (1..BINS).rev() {
                r_lo = r_lo.min(bin_b[b].0);
                r_hi = r_hi.max(bin_b[b].1);
                r_n += bin_n[b];
                let c = left_cost[b - 1] + area(r_lo, r_hi) * r_n as f32;
                if c < best_cost {
                    best_cost = c;
                    best = b;
                }
            }
            let leaf_cost = area(lo, hi) * count as f32;
            if best == usize::MAX || (best_cost >= leaf_cost && count <= 8) {
                nodes[ni].start = s as u32;
                nodes[ni].count = count as u32;
                continue;
            }
            // Partition.
            let mut i = s;
            let mut j = e;
            while i < j {
                if bin_of(idx[i]) < best {
                    i += 1;
                } else {
                    j -= 1;
                    idx.swap(i, j);
                }
            }
            let mut mid = i;
            if mid == s || mid == e {
                mid = (s + e) / 2;
            }
            let left = nodes.len();
            nodes.push(BvhNode::default());
            nodes.push(BvhNode::default());
            nodes[ni].start = left as u32;
            nodes[ni].count = 0;
            stack.push((left, s, mid));
            stack.push((left + 1, mid, e));
        }
        // Reorder triangles so leaves are contiguous.
        self.tris = idx.iter().map(|i| self.tris[*i as usize]).collect();
        self.shade = idx.iter().map(|i| self.shade[*i as usize]).collect();
        self.nodes = nodes;
    }

    /// Closest hit: (t, triangle, u, v).
    fn intersect(&self, o: Vec3, d: Vec3, tmax: f32) -> Option<(f32, usize, f32, f32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<(f32, usize, f32, f32)> = None;
        let mut tbest = tmax;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if !slab(node.min, node.max, o, inv, tbest) {
                continue;
            }
            if node.count > 0 {
                for i in node.start as usize..(node.start + node.count) as usize {
                    if let Some((t, u, v)) = tri_hit(&self.tris[i], o, d, tbest) {
                        tbest = t;
                        best = Some((t, i, u, v));
                    }
                }
            } else if sp + 2 <= stack.len() {
                // Visit the nearer child first.
                let (l, r) = (node.start, node.start + 1);
                let ln = &self.nodes[l as usize];
                let rn = &self.nodes[r as usize];
                let dl = slab_dist(ln.min, ln.max, o, inv);
                let dr = slab_dist(rn.min, rn.max, o, inv);
                if dl < dr {
                    stack[sp] = r;
                    stack[sp + 1] = l;
                } else {
                    stack[sp] = l;
                    stack[sp + 1] = r;
                }
                sp += 2;
            }
        }
        best
    }

    /// Transmission along a shadow ray (1 = unblocked; transparent surfaces let light through).
    fn transmission(&self, mut o: Vec3, d: Vec3, mut dist: f32) -> f32 {
        let mut tr = 1.0;
        for _ in 0..8 {
            let Some((t, i, _, _)) = self.intersect(o, d, dist) else { return tr };
            let m = &self.mats[self.shade[i].mat as usize];
            tr *= m.transp;
            if tr < 1e-3 {
                return 0.0;
            }
            o += d * (t + EPS * 10.0);
            dist -= t + EPS * 10.0;
            if dist <= 0.0 {
                return tr;
            }
        }
        tr
    }

    fn sample_tex(&self, layer: i32, uv: Vec2) -> Option<Vec3> {
        let img = self.textures.get(usize::try_from(layer).ok()?)?;
        let px = img.mips.first()?;
        let n = TEX_SIZE as i32;
        let x = uv.x.rem_euclid(1.0) * n as f32 - 0.5;
        let y = uv.y.rem_euclid(1.0) * n as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let fetch = |xi: i32, yi: i32| {
            let (xi, yi) = (xi.rem_euclid(n), yi.rem_euclid(n));
            let o = ((yi * n + xi) * 4) as usize;
            Vec3::new(px[o] as f32, px[o + 1] as f32, px[o + 2] as f32) / 255.0
        };
        let (xi, yi) = (x0 as i32, y0 as i32);
        let c = fetch(xi, yi).lerp(fetch(xi + 1, yi), fx).lerp(fetch(xi, yi + 1).lerp(fetch(xi + 1, yi + 1), fx), fy);
        Some(c.powf(2.2))
    }

    fn sky(&self, d: Vec3, env: f32) -> Vec3 {
        let zenith = Vec3::new(0.55, 0.68, 0.9);
        let horizon = Vec3::new(0.85, 0.86, 0.88);
        let ground = Vec3::new(0.28, 0.26, 0.24);
        let c = if d.y >= 0.0 { horizon.lerp(zenith, d.y.sqrt()) } else { horizon.lerp(ground, (-d.y).sqrt().min(1.0)) };
        c * env + self.ambient
    }
}

fn slab(lo: Vec3, hi: Vec3, o: Vec3, inv: Vec3, tmax: f32) -> bool {
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let tn = t0.min(t1).max_element().max(0.0);
    let tf = t0.max(t1).min_element().min(tmax);
    tn <= tf
}

fn slab_dist(lo: Vec3, hi: Vec3, o: Vec3, inv: Vec3) -> f32 {
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let tn = t0.min(t1).max_element().max(0.0);
    let tf = t0.max(t1).min_element();
    if tn <= tf { tn } else { f32::MAX }
}

/// Möller–Trumbore.
fn tri_hit(t: &Tri, o: Vec3, d: Vec3, tmax: f32) -> Option<(f32, f32, f32)> {
    let p = d.cross(t.e2);
    let det = t.e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t.v0;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(t.e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let tt = t.e2.dot(q) * inv;
    (tt > EPS && tt < tmax).then_some((tt, u, v))
}

// ------------------------------------------------------------------------------ sampling

/// PCG32.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut r = Rng(seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
        r.next();
        r
    }
    fn next(&mut self) -> u32 {
        let old = self.0;
        self.0 = old.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let xs = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xs.rotate_right(rot)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

fn basis(n: Vec3) -> (Vec3, Vec3) {
    let s = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    (Vec3::new(1.0 + s * n.x * n.x * a, s * b, -s * n.x), Vec3::new(b, s + n.y * n.y * a, -n.y))
}

fn lum(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

struct Surface {
    n: Vec3,
    base: Vec3,
    f0: Vec3,
    kd: Vec3,
    alpha: f32,
    p_spec: f32,
}

fn ggx_d(nh: f32, a2: f32) -> f32 {
    let d = nh * nh * (a2 - 1.0) + 1.0;
    a2 / (PI * d * d).max(1e-12)
}

fn smith_g1(x: f32, a2: f32) -> f32 {
    2.0 * x / (x + (a2 + (1.0 - a2) * x * x).sqrt()).max(1e-12)
}

fn fresnel(f0: Vec3, c: f32) -> Vec3 {
    f0 + (Vec3::ONE - f0) * (1.0 - c).clamp(0.0, 1.0).powi(5)
}

impl Surface {
    /// BSDF value times cos(wi), and the sampling pdf for wi.
    fn eval(&self, wo: Vec3, wi: Vec3) -> (Vec3, f32) {
        let nwi = self.n.dot(wi);
        let nwo = self.n.dot(wo);
        if nwi <= 0.0 || nwo <= 0.0 {
            return (Vec3::ZERO, 0.0);
        }
        let diff = self.kd * (1.0 / PI) * nwi;
        let pdf_d = nwi / PI;
        let mut f = diff;
        let mut pdf = (1.0 - self.p_spec) * pdf_d;
        if self.p_spec > 0.0 {
            let h = (wi + wo).normalize();
            let nh = self.n.dot(h).max(0.0);
            let vh = wo.dot(h).max(1e-6);
            let a2 = self.alpha * self.alpha;
            let d = ggx_d(nh, a2);
            let g = smith_g1(nwo, a2) * smith_g1(nwi, a2);
            let fr = fresnel(self.f0, vh);
            f += fr * (d * g / (4.0 * nwo));
            pdf += self.p_spec * d * nh / (4.0 * vh);
        }
        (f, pdf)
    }

    fn sample(&self, wo: Vec3, rng: &mut Rng) -> Option<Vec3> {
        let (t, b) = basis(self.n);
        let (u1, u2) = (rng.f(), rng.f());
        if rng.f() < self.p_spec {
            let a2 = self.alpha * self.alpha;
            let cos_t = ((1.0 - u1) / (1.0 + (a2 - 1.0) * u1)).max(0.0).sqrt();
            let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
            let phi = 2.0 * PI * u2;
            let h = t * (sin_t * phi.cos()) + b * (sin_t * phi.sin()) + self.n * cos_t;
            let wi = 2.0 * wo.dot(h) * h - wo;
            (wi.dot(self.n) > 0.0).then_some(wi)
        } else {
            let r = u1.sqrt();
            let phi = 2.0 * PI * u2;
            Some((t * (r * phi.cos()) + b * (r * phi.sin()) + self.n * (1.0 - u1).max(0.0).sqrt()).normalize())
        }
    }
}

/// One path sample: (radiance, alpha coverage).
fn trace(sc: &RtScene, s: &RenderSettings, mut o: Vec3, mut d: Vec3, rng: &mut Rng) -> (Vec3, f32) {
    let mut l = Vec3::ZERO;
    let mut thr = Vec3::ONE;
    let mut alpha = 0.0;
    let mut depth = 0u32;
    let mut passes = 0u32;
    let mut primary = true;
    loop {
        let Some((t, ti, u, v)) = sc.intersect(o, d, f32::MAX) else {
            if primary {
                if s.env_background {
                    l += sc.sky(d, s.env);
                    alpha = 1.0;
                }
            } else {
                let mut c = thr * sc.sky(d, s.env);
                if depth > 0 {
                    let m = c.max_element();
                    if m > s.clamp {
                        c *= s.clamp / m;
                    }
                }
                l += c;
            }
            break;
        };
        if primary {
            alpha = 1.0;
        }
        let sh = &sc.shade[ti];
        let m = sc.mats[sh.mat as usize];
        let p = o + d * t;
        let w = 1.0 - u - v;
        let tri = &sc.tris[ti];
        let mut ng = tri.e1.cross(tri.e2).normalize();
        let mut n = (sh.n[0] * w + sh.n[1] * u + sh.n[2] * v).normalize_or(ng);
        if ng.dot(d) > 0.0 {
            ng = -ng;
        }
        if n.dot(ng) < 0.0 {
            n = -n;
        }
        // Transparency: continue straight through (stochastically).
        if m.transp > 0.0 && passes < 16 && rng.f() < m.transp {
            o = p + d * (EPS * 10.0);
            passes += 1;
            continue;
        }
        primary = false;
        let mut base = m.albedo;
        if sh.has_uv && m.tex >= 0 {
            let uv = sh.uv[0] * w + sh.uv[1] * u + sh.uv[2] * v;
            if let Some(c) = sc.sample_tex(m.tex, uv) {
                base = c;
            }
        }
        if m.emission.max_element() > 0.0 {
            l += thr * m.emission;
        }
        let f0 = Vec3::splat(0.08 * m.spec).lerp(base, m.metal);
        let kd = base * (1.0 - m.metal) * (1.0 - f0.max_element().min(0.9));
        let wo = -d;
        let spec_w = lum(fresnel(f0, n.dot(wo).max(0.0)));
        let diff_w = lum(kd);
        let p_spec = if m.spec <= 0.0 && m.metal <= 0.0 { 0.0 } else { (spec_w / (spec_w + diff_w).max(1e-6)).clamp(0.1, 0.95) };
        let surf = Surface { n, base, f0, kd, alpha: (m.rough * m.rough).max(0.002), p_spec };
        let _ = surf.base;
        let po = p + ng * (EPS * 10.0 * (1.0 + p.abs().max_element() * 1e-3));
        // Next-event estimation for the delta lights.
        for light in &sc.lights {
            let (wi, dist, rad) = match *light {
                RtLight::Dir { dir_to_light, rad } => (dir_to_light, f32::MAX, rad),
                RtLight::Point { pos, rad } => {
                    let dv = pos - po;
                    let dd = dv.length();
                    (dv / dd.max(1e-6), dd, rad)
                }
                RtLight::Spot { pos, axis, cos_outer, cos_inner, rad } => {
                    let dv = pos - po;
                    let dd = dv.length();
                    let wi = dv / dd.max(1e-6);
                    let ca = (-wi).dot(axis);
                    let x = ((ca - cos_outer) / (cos_inner - cos_outer)).clamp(0.0, 1.0);
                    (wi, dd, rad * (x * x * (3.0 - 2.0 * x)))
                }
            };
            if rad.max_element() <= 0.0 || wi.dot(ng) <= 0.0 {
                continue;
            }
            let (f, _) = surf.eval(wo, wi);
            if f.max_element() <= 0.0 {
                continue;
            }
            let tr = sc.transmission(po, wi, dist);
            if tr > 0.0 {
                let mut c = thr * f * rad * tr;
                if depth > 0 {
                    let mx = c.max_element();
                    if mx > s.clamp {
                        c *= s.clamp / mx;
                    }
                }
                l += c;
            }
        }
        if depth >= s.bounces {
            break;
        }
        let Some(wi) = surf.sample(wo, rng) else { break };
        let (f, pdf) = surf.eval(wo, wi);
        if pdf <= 1e-8 || wi.dot(ng) <= 0.0 {
            break;
        }
        thr *= f / pdf;
        depth += 1;
        if depth >= 3 {
            let q = thr.max_element().clamp(0.05, 0.95);
            if rng.f() > q {
                break;
            }
            thr /= q;
        }
        if !thr.is_finite() {
            break;
        }
        o = po;
        d = wi;
    }
    (l, alpha)
}

// ------------------------------------------------------------------------------ jobs

struct Buffers {
    /// Accumulated linear RGB + alpha per pixel.
    accum: Vec<[f32; 4]>,
    /// Samples accumulated per row.
    row_spp: Vec<u32>,
}

struct Shared {
    w: u32,
    h: u32,
    buf: Mutex<Buffers>,
    cancel: AtomicBool,
    /// Rows finished (all passes) — progress.
    rows_done: AtomicU64,
    version: AtomicU64,
    workers_left: AtomicU64,
}

static JOB_ID: AtomicU64 = AtomicU64::new(1);

/// A progressive render running on background threads.
pub struct RenderJob {
    /// Unique per job.
    pub id: u64,
    shared: Arc<Shared>,
    pub settings: RenderSettings,
    pub started: std::time::Instant,
    pub finished: Option<std::time::Duration>,
    pub tri_count: usize,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl RenderJob {
    pub fn start(scene: Arc<RtScene>, cam: RtCamera, settings: RenderSettings) -> RenderJob {
        let (w, h) = (settings.width.clamp(8, 8192), settings.height.clamp(8, 8192));
        let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).clamp(1, 64);
        let shared = Arc::new(Shared {
            w,
            h,
            buf: Mutex::new(Buffers { accum: vec![[0.0; 4]; (w * h) as usize], row_spp: vec![0; h as usize] }),
            cancel: AtomicBool::new(false),
            rows_done: AtomicU64::new(0),
            version: AtomicU64::new(0),
            workers_left: AtomicU64::new(threads as u64),
        });
        let tri_count = scene.tri_count;
        let mut handles = Vec::new();
        for wi in 0..threads {
            let (sh, sc, st) = (shared.clone(), scene.clone(), settings.clone());
            handles.push(std::thread::spawn(move || {
                worker(&sh, &sc, &cam, &st, wi as u32, threads as u32);
                sh.workers_left.fetch_sub(1, Ordering::SeqCst);
                sh.version.fetch_add(1, Ordering::SeqCst);
            }));
        }
        RenderJob { id: JOB_ID.fetch_add(1, Ordering::Relaxed), shared, settings, started: std::time::Instant::now(), finished: None, tri_count, handles }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.shared.w, self.shared.h)
    }

    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_done(&self) -> bool {
        self.shared.workers_left.load(Ordering::SeqCst) == 0
    }

    pub fn was_cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::SeqCst)
    }

    /// 0..1
    pub fn progress(&self) -> f32 {
        let total = self.shared.h as u64 * self.settings.spp.max(1) as u64;
        self.shared.rows_done.load(Ordering::Relaxed) as f32 / total as f32
    }

    /// Changes whenever new samples land.
    pub fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Relaxed)
    }

    /// Minimum samples per pixel accumulated so far.
    pub fn samples(&self) -> u32 {
        self.shared.buf.lock().map(|b| b.row_spp.iter().copied().min().unwrap_or(0)).unwrap_or(0)
    }

    pub fn elapsed(&mut self) -> std::time::Duration {
        if self.finished.is_none() && self.is_done() {
            self.finished = Some(self.started.elapsed());
        }
        self.finished.unwrap_or_else(|| self.started.elapsed())
    }

    /// Wait for completion.
    pub fn join(mut self) -> RenderJob {
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        self.elapsed();
        self
    }

    /// Average linear RGBA per pixel (rows top to bottom).
    pub fn linear(&self) -> Vec<[f32; 4]> {
        let b = self.shared.buf.lock().unwrap();
        let w = self.shared.w as usize;
        b.accum
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let n = b.row_spp[i / w].max(1) as f32;
                [p[0] / n, p[1] / n, p[2] / n, p[3] / n]
            })
            .collect()
    }

    /// Tone-mapped sRGB RGBA8 (rows top to bottom). Rows without samples are transparent.
    pub fn rgba8(&self, exposure: f32, tm: ToneMap) -> Vec<u8> {
        tonemap(&self.linear(), exposure, tm)
    }

    pub fn save_png(&self, path: &std::path::Path, exposure: f32, tm: ToneMap) -> Result<(), String> {
        let (w, h) = self.size();
        let img = image::RgbaImage::from_raw(w, h, self.rgba8(exposure, tm)).ok_or("image size")?;
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(dir);
        }
        img.save(path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

impl Drop for RenderJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn tonemap(lin: &[[f32; 4]], exposure: f32, tm: ToneMap) -> Vec<u8> {
    let k = 2f32.powf(exposure);
    let mut out = Vec::with_capacity(lin.len() * 4);
    for p in lin {
        let a = p[3].clamp(0.0, 1.0);
        for c in &p[..3] {
            let mut x = (c * k).max(0.0);
            if tm == ToneMap::Filmic {
                // Narkowicz ACES fit.
                x = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
            }
            // Colour is premultiplied by coverage; un-premultiply for straight alpha.
            if a > 1e-4 && a < 1.0 {
                x /= a;
            }
            out.push((x.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8);
        }
        out.push((a * 255.0 + 0.5) as u8);
    }
    out
}

fn worker(sh: &Shared, sc: &RtScene, cam: &RtCamera, s: &RenderSettings, wi: u32, n: u32) {
    let (w, h) = (sh.w, sh.h);
    let aspect = w as f32 / h as f32;
    let mut row = vec![[0.0f32; 4]; w as usize];
    for pass in 0..s.spp.max(1) {
        let mut y = wi;
        while y < h {
            if sh.cancel.load(Ordering::Relaxed) {
                return;
            }
            for x in 0..w {
                let mut rng = Rng::new(((pass as u64) << 40) ^ ((y as u64) << 20) ^ x as u64 ^ 0x9E37_79B9_7F4A_7C15);
                let (jx, jy) = if pass == 0 { (0.5, 0.5) } else { (rng.f(), rng.f()) };
                let sx = ((x as f32 + jx) / w as f32) * 2.0 - 1.0;
                let sy = 1.0 - ((y as f32 + jy) / h as f32) * 2.0;
                let (o, d) = cam.ray(sx, sy, aspect);
                let (c, a) = trace(sc, s, o, d, &mut rng);
                let c = if c.is_finite() { c } else { Vec3::ZERO };
                row[x as usize] = [c.x, c.y, c.z, a];
            }
            {
                let mut b = sh.buf.lock().unwrap();
                let base = (y * w) as usize;
                for (dst, src) in b.accum[base..base + w as usize].iter_mut().zip(&row) {
                    for k in 0..4 {
                        dst[k] += src[k];
                    }
                }
                b.row_spp[y as usize] += 1;
            }
            sh.rows_done.fetch_add(1, Ordering::Relaxed);
            sh.version.fetch_add(1, Ordering::Relaxed);
            y += n;
        }
    }
}

/// Render synchronously (batch / MEL `render -o`).
pub fn render_blocking(scene: Arc<RtScene>, cam: RtCamera, settings: RenderSettings) -> RenderJob {
    RenderJob::start(scene, cam, settings).join()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::commands::execute;

    fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * w + x) * 4) as usize;
        [img[o], img[o + 1], img[o + 2], img[o + 3]]
    }

    #[test]
    fn bvh_matches_brute_force() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -n a; polyCube -w 3 -h 0.2 -d 3 -n floor; move 0 -1.2 0 floor; polyTorus -r 2 -sr 0.3 -n t").unwrap();
        let g = app.build_geometry();
        let cam = RtCamera::from_view(&app.views[0].cam);
        let sc = RtScene::build(&g, &cam);
        assert!(sc.tri_count > 500);
        let mut rng = Rng::new(7);
        for _ in 0..300 {
            let o = Vec3::new(rng.f() * 8.0 - 4.0, rng.f() * 8.0 - 4.0, 6.0);
            let d = (Vec3::new(rng.f() * 2.0 - 1.0, rng.f() * 2.0 - 1.0, 0.0) - o).normalize();
            let fast = sc.intersect(o, d, f32::MAX).map(|h| h.0);
            let slow = sc.tris.iter().filter_map(|t| tri_hit(t, o, d, f32::MAX).map(|h| h.0)).fold(None, |a: Option<f32>, t| Some(a.map_or(t, |a| a.min(t))));
            match (fast, slow) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-4),
                (None, None) => {}
                x => panic!("mismatch {x:?}"),
            }
        }
    }

    #[test]
    fn renders_lit_sphere_with_shadow() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(
            &mut app,
            "polyPlane -w 20 -h 20 -n ground; polySphere -r 1 -n ball; move 0 1 0 ball; directionalLight -i 1 -rot -90 0 0 -n sun; \
             camera -p 0 2 8 -aim 0 1 0 -n shot",
        )
        .unwrap();
        let s = RenderSettings { width: 80, height: 60, spp: 8, env: 0.0, ..Default::default() };
        let job = app.render_now(Some("shot"), s.clone()).unwrap();
        let img = job.rgba8(0.0, ToneMap::Srgb);
        // The top of the ball (lit from straight above) is bright; the background is empty.
        let (w, h) = (s.width, s.height);
        let mut top = None;
        for y in 0..h {
            let p = px(&img, w, w / 2, y);
            if p[3] > 0 {
                top = Some((y, p));
                break;
            }
        }
        let (ty, tp) = top.expect("ball visible");
        assert!(ty > 2, "sky at the top");
        assert!(px(&img, w, w / 2, 0)[3] == 0, "transparent background");
        assert!(tp[0] > 150, "lit top {tp:?}");
        // Directly under the ball (shadow) is darker than the open ground beside it.
        let ground_y = h - 3;
        let under = px(&img, w, w / 2, ground_y.min(h - 1));
        let side = px(&img, w, 2, ground_y);
        let _ = (under, side);
        // Sample the shadow just in front of the ball's contact point.
        let col = |x: u32, y: u32| px(&img, w, x, y)[0] as i32;
        let mut darkest = 255;
        for y in h * 2 / 3..h {
            darkest = darkest.min(col(w / 2, y));
        }
        assert!(darkest < col(3, h - 2) - 40, "shadow under the ball: {darkest} vs {}", col(3, h - 2));
    }

    #[test]
    fn mel_render_writes_png() {
        let mut app = App::new();
        app.scene.nodes.clear();
        let dir = std::env::temp_dir().join("rm_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        let out = dir.join("frame.png");
        execute(&mut app, "polyCube -n box; shadingNode -asShader standardSurface -n gold -rgb 1 0.8 0.3; setAttr gold.metalness 1; hyperShade -assign gold box").unwrap();
        let r = execute(&mut app, &format!("render -x 64 -y 48 -spp 4 -o \"{}\"", out.display())).unwrap();
        assert!(r.contains("64"), "{r}");
        let img = image::open(&out).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (64, 48));
        assert!(img.pixels().any(|p| p[3] > 0 && p[0] > p[2]), "gold-ish pixels");
        // Sequence.
        execute(&mut app, "setKeyframe -t 1 -at ty -v 0 box; setKeyframe -t 3 -at ty -v 1 box").unwrap();
        let seq = dir.join("seq/shot");
        execute(&mut app, &format!("render -x 32 -y 24 -spp 1 -seq -st 1 -e 3 -o \"{}\"", seq.display())).unwrap();
        for f in 1..=3 {
            assert!(dir.join(format!("seq/shot.{f:04}.png")).exists());
        }
    }

    #[test]
    #[ignore]
    fn showcase() {
        let out = std::env::var("RT_OUT").unwrap_or_else(|_| "/tmp/showcase.png".into());
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(
            &mut app,
            "polyPlane -w 30 -h 30 -n ground; polySphere -r 1 -sa 48 -sh 32 -n red; move -2.3 1 0 red; \
             polySphere -r 1 -sa 48 -sh 32 -n chrome; move 0 1 0 chrome; polyCube -w 1.6 -h 1.6 -d 1.6 -n box; move 2.3 0.8 0 box; rotate 0 30 0 box; \
             polyTorus -r 0.8 -sr 0.25 -n glow; move 0 0.25 2.2 glow; \
             shadingNode -asShader blinn -n redM -rgb 0.8 0.1 0.08; setAttr redM.specular 0.6; setAttr redM.roughness 0.3; hyperShade -assign redM red; \
             shadingNode -asShader standardSurface -n chromeM -rgb 0.95 0.95 0.95; setAttr chromeM.metalness 1; setAttr chromeM.roughness 0.08; hyperShade -assign chromeM chrome; \
             shadingNode -asShader lambert -n boxM -rgb 1 1 1; setAttr boxM.colorMap checker; hyperShade -assign boxM box; polyAutoProjection box; \
             shadingNode -asShader lambert -n glowM -rgb 0.1 0.1 0.1; setAttr glowM.incandescence 1 0.5 0.1; hyperShade -assign glowM glow; \
             directionalLight -i 1.2 -rot -50 -35 0 -n sun; pointLight -i 0.4 -rgb 0.6 0.7 1 -p -4 4 4 -n fill; \
             camera -p 0 3.5 9 -aim 0 0.8 0 -n shot",
        )
        .unwrap();
        let s = RenderSettings { width: 640, height: 360, spp: 128, env: 0.35, env_background: true, ..Default::default() };
        let mut job = app.render_now(Some("shot"), s.clone()).unwrap();
        eprintln!("rendered in {:.2}s, {} tris", job.elapsed().as_secs_f32(), job.tri_count);
        job.save_png(std::path::Path::new(&out), 0.0, ToneMap::Filmic).unwrap();
    }
}
