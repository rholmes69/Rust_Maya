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
    /// Edge-aware denoising of the displayed / saved image (albedo + normal guided).
    pub denoise: bool,
    /// Render region (x0, y0, x1, y1 in 0..1, top-left origin); None = whole frame.
    pub region: Option<[f32; 4]>,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings { width: 960, height: 540, spp: 64, bounces: 4, env: 0.25, exposure: 0.0, tonemap: ToneMap::Srgb, env_background: false, clamp: 4.0, denoise: true, region: None }
    }
}

impl RenderSettings {
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.width, self.height, self.spp, self.bounces, self.env.to_bits(), self.env_background, self.clamp.to_bits()).hash(&mut h);
        self.region.map(|r| r.map(f32::to_bits)).hash(&mut h);
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
    /// Thin-lens aperture radius (0 = pinhole) and focus distance.
    pub lens_radius: f32,
    pub focus: f32,
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
            lens_radius: 0.0,
            focus: 1.0,
        }
    }

    /// Camera node: position and orientation from its world matrix (looks down local -Z).
    pub fn from_node(m: glam::Mat4, cs: &crate::look::CameraShape) -> RtCamera {
        let fov_y_deg = cs.fov_y();
        let eye = m.transform_point3(Vec3::ZERO);
        let fwd = m.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
        let up0 = m.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
        let right = fwd.cross(up0).normalize_or(Vec3::X);
        let up = right.cross(fwd).normalize();
        // Focal length in mm, scene unit = 1 m: aperture radius = f / (2 N).
        let lens_radius = if cs.dof { cs.focal * 0.001 / (2.0 * cs.fstop.max(0.5)) * cs.blur_scale } else { 0.0 };
        RtCamera { eye, fwd, right, up, tan_half: (fov_y_deg.to_radians() * 0.5).tan(), ortho_half: None, lens_radius, focus: cs.focus_distance.max(1e-3) }
    }

    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in [self.eye, self.fwd, self.up] {
            v.to_array().map(f32::to_bits).hash(&mut h);
        }
        (self.tan_half.to_bits(), self.ortho_half.map(f32::to_bits), self.lens_radius.to_bits(), self.focus.to_bits()).hash(&mut h);
        h.finish()
    }

    /// Ray through normalized screen coords (sx, sy in -1..1, y up); `lens` is a uniform
    /// sample in [0,1)² for the thin-lens aperture.
    fn ray(&self, sx: f32, sy: f32, aspect: f32, lens: (f32, f32)) -> (Vec3, Vec3) {
        match self.ortho_half {
            Some(h) => {
                let o = self.eye + self.right * (sx * h * aspect) + self.up * (sy * h);
                (o, self.fwd)
            }
            None => {
                let d = self.fwd + self.right * (sx * self.tan_half * aspect) + self.up * (sy * self.tan_half);
                if self.lens_radius <= 0.0 {
                    return (self.eye, d.normalize());
                }
                // Point on the focus plane (perpendicular to the view axis), seen from a
                // point on the lens disk.
                let target = self.eye + d * self.focus;
                let r = self.lens_radius * lens.0.sqrt();
                let a = std::f32::consts::TAU * lens.1;
                let o = self.eye + self.right * (r * a.cos()) + self.up * (r * a.sin());
                (o, (target - o).normalize())
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
    /// Normal / roughness / metalness map layers, -1 = none.
    tex_n: i32,
    tex_r: i32,
    tex_m: i32,
    tex_ao: i32,
    tex_e: i32,
}

#[derive(Clone, Copy)]
struct Tri {
    v0: Vec3,
    e1: Vec3,
    e2: Vec3,
}

#[derive(Clone, Copy)]
struct TriShade {
    /// Object index (1-based, per mesh chunk) for the object-ID pass.
    obj: u32,
    n: [Vec3; 3],
    /// Per-triangle UV tangent / bitangent (dP/du, dP/dv) for normal mapping.
    tu: Vec3,
    tv: Vec3,
    uv: [Vec2; 3],
    has_uv: bool,
    mat: u32,
}

#[derive(Clone, Copy, Debug)]
enum RtLight {
    Dir { dir_to_light: Vec3, rad: Vec3 },
    Point { pos: Vec3, rad: Vec3 },
    Spot { pos: Vec3, axis: Vec3, cos_outer: f32, cos_inner: f32, rad: Vec3 },
    /// Rectangle centre ± ex ± ey, emitting along `n` (one-sided).
    Area { c: Vec3, ex: Vec3, ey: Vec3, n: Vec3, area: f32, rad: Vec3 },
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
        let mut mat_ids: std::collections::HashMap<[u32; 15], u32> = Default::default();
        for (ci, chunk) in g.tris.iter().chain(&g.tris_transp).enumerate() {
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
                    v.maps[0].to_bits(),
                    v.maps[1].to_bits(),
                    v.maps[2].to_bits(),
                    v.maps[3].to_bits(),
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
                        tex_n: (v.maps[0] + 0.5) as i32 - 1,
                        tex_r: (v.maps[1] + 0.5) as i32 - 1,
                        tex_m: (v.maps[2] + 0.5) as i32 - 1,
                        tex_ao: ((v.maps[3] + 0.5) as u32 % 1024) as i32 - 1,
                        tex_e: ((v.maps[3] + 0.5) as u32 / 1024) as i32 - 1,
                    });
                    mats.len() as u32 - 1
                });
                let fallback = e1.cross(e2).normalize();
                let n = |i: usize| Vec3::from(t[i].nrm).try_normalize().unwrap_or(fallback);
                tris.push(Tri { v0: a, e1, e2 });
                let (uv0, uv1, uv2) = (Vec2::from(t[0].uv), Vec2::from(t[1].uv), Vec2::from(t[2].uv));
                let (d1, d2) = (uv1 - uv0, uv2 - uv0);
                let det = d1.x * d2.y - d2.x * d1.y;
                let (tu, tv) = if det.abs() > 1e-12 {
                    let r = 1.0 / det;
                    ((e1 * d2.y - e2 * d1.y) * r, (e2 * d1.x - e1 * d2.x) * r)
                } else {
                    (Vec3::ZERO, Vec3::ZERO)
                };
                shade.push(TriShade {
                    obj: ci as u32 + 1,
                    n: [n(0), n(1), n(2)],
                    tu,
                    tv,
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
        // Area lights: the GpuLight entry is the viewport approximation; replace it with the
        // real rectangle (match by centre).
        for (c, ex, ey, dir, col) in &g.area_lights {
            if let Some(i) = lights.iter().position(|l| matches!(l, RtLight::Spot { pos, .. } if pos.distance(*c) < 1e-5)) {
                lights.remove(i);
            }
            let area = 4.0 * ex.cross(*ey).length();
            if area > 1e-8 {
                lights.push(RtLight::Area { c: *c, ex: *ex, ey: *ey, n: dir.normalize_or(Vec3::NEG_Z), area, rad: Vec3::from(*col) * PI });
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

    /// Raw (non-colour) texture value, bilinear, 0..1.
    fn sample_raw(&self, layer: i32, uv: Vec2) -> Option<Vec3> {
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
        Some(fetch(xi, yi).lerp(fetch(xi + 1, yi), fx).lerp(fetch(xi, yi + 1).lerp(fetch(xi + 1, yi + 1), fx), fy))
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

/// First-hit features for the denoiser.
#[derive(Clone, Copy, Default)]
struct Aov {
    albedo: Vec3,
    normal: Vec3,
    /// Mirror-likeness of the first hit (reflections there are kept sharp).
    gloss: f32,
    /// Distance from the camera to the first opaque hit (0 = none).
    depth: f32,
    obj: u32,
}

/// One path sample: (radiance, alpha coverage, first-hit albedo / normal).
fn trace(sc: &RtScene, s: &RenderSettings, mut o: Vec3, mut d: Vec3, rng: &mut Rng) -> (Vec3, f32, Aov) {
    let mut aov = Aov::default();
    let mut l = Vec3::ZERO;
    let mut thr = Vec3::ONE;
    let mut alpha = 0.0;
    let mut depth = 0u32;
    let mut passes = 0u32;
    let mut primary = true;
    let mut dist_acc = 0.0f32;
    loop {
        let Some((t, ti, u, v)) = sc.intersect(o, d, f32::MAX) else {
            if primary {
                if s.env_background {
                    let c = sc.sky(d, s.env);
                    l += c;
                    alpha = 1.0;
                    aov.albedo = c.min(Vec3::ONE);
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
        let mut m = sc.mats[sh.mat as usize];
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
            dist_acc += t;
            o = p + d * (EPS * 10.0);
            passes += 1;
            continue;
        }
        let mut base = m.albedo;
        let mut ao = 1.0f32;
        if sh.has_uv {
            let uv = sh.uv[0] * w + sh.uv[1] * u + sh.uv[2] * v;
            if m.tex >= 0
                && let Some(c) = sc.sample_tex(m.tex, uv)
            {
                base = c;
            }
            if m.tex_r >= 0
                && let Some(r) = sc.sample_raw(m.tex_r, uv)
            {
                m.rough = (m.rough * r.x).clamp(0.03, 1.0);
            }
            if m.tex_m >= 0
                && let Some(r) = sc.sample_raw(m.tex_m, uv)
            {
                m.metal = (m.metal * r.x).clamp(0.0, 1.0);
            }
            if m.tex_e >= 0
                && let Some(c) = sc.sample_tex(m.tex_e, uv)
            {
                m.emission *= c;
            }
            if m.tex_ao >= 0
                && let Some(r) = sc.sample_raw(m.tex_ao, uv)
            {
                ao = r.x;
            }
            if m.tex_n >= 0
                && sh.tu.length_squared() > 0.0
                && let Some(r) = sc.sample_raw(m.tex_n, uv)
            {
                let tsn = r * 2.0 - Vec3::ONE;
                // Gram-Schmidt the UV tangent against the shading normal; the bitangent keeps
                // the UV handedness.
                let t = (sh.tu - n * n.dot(sh.tu)).normalize_or_zero();
                let sign = if n.cross(t).dot(sh.tv) < 0.0 { -1.0 } else { 1.0 };
                let b = n.cross(t) * sign;
                let nm = (t * tsn.x + b * tsn.y + n * tsn.z.max(0.05)).normalize_or(n);
                if nm.dot(ng) > 0.02 {
                    n = nm;
                }
            }
        }
        if primary {
            aov.albedo = (base + m.emission).min(Vec3::ONE).max(Vec3::splat(0.0));
            aov.normal = n;
            aov.gloss = (1.0 - m.rough) * m.metal.max(m.spec * 0.6);
            aov.obj = sh.obj;
            aov.depth = dist_acc + t;
        }
        primary = false;
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
                RtLight::Area { c, ex, ey, n, area, rad } => {
                    let pl = c + ex * (2.0 * rng.f() - 1.0) + ey * (2.0 * rng.f() - 1.0);
                    let dv = pl - po;
                    let d2 = dv.length_squared().max(1e-8);
                    let dd = d2.sqrt();
                    let wi = dv / dd;
                    let cos_l = n.dot(-wi);
                    if cos_l <= 0.0 {
                        continue;
                    }
                    // Uniform area sampling: L · cosθ_light · A / d².
                    (wi, dd * (1.0 - 1e-4), rad * (cos_l * area / d2))
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
        // Baked occlusion darkens the light arriving from the hemisphere (indirect only).
        thr *= f / pdf * ao;
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
    (l, alpha, aov)
}

// ------------------------------------------------------------------------------ jobs

struct Buffers {
    /// Accumulated linear RGB + alpha per pixel.
    accum: Vec<[f32; 4]>,
    /// Accumulated first-hit albedo (3), normal (3), luminance² (1), gloss (1); then depth
    /// and object id from the first (unjittered) pass.
    aov: Vec<[f32; 10]>,
    /// Samples accumulated per row.
    row_spp: Vec<u32>,
}

struct Shared {
    w: u32,
    h: u32,
    /// Pixel rectangle being rendered [x0, y0, x1, y1).
    region: [u32; 4],
    /// Image shown outside the region (the previous render).
    base: Option<Vec<[f32; 4]>>,
    buf: Mutex<Buffers>,
    cancel: AtomicBool,
    /// Rows finished (all passes) — progress.
    rows_done: AtomicU64,
    version: AtomicU64,
    workers_left: AtomicU64,
}

static JOB_ID: AtomicU64 = AtomicU64::new(1);

/// Render passes (AOVs) the Render View can show and save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Beauty,
    Albedo,
    Normal,
    Depth,
    ObjectId,
}

impl Pass {
    pub const ALL: [Pass; 5] = [Pass::Beauty, Pass::Albedo, Pass::Normal, Pass::Depth, Pass::ObjectId];
    pub fn name(self) -> &'static str {
        match self {
            Pass::Beauty => "Beauty",
            Pass::Albedo => "Albedo",
            Pass::Normal => "Normal",
            Pass::Depth => "Depth",
            Pass::ObjectId => "Object ID",
        }
    }
}

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
        Self::start_with_base(scene, cam, settings, None)
    }

    /// Start a render; with a region, `base` (same size) fills the pixels outside it.
    pub fn start_with_base(scene: Arc<RtScene>, cam: RtCamera, settings: RenderSettings, base: Option<Vec<[f32; 4]>>) -> RenderJob {
        let (w, h) = (settings.width.clamp(8, 8192), settings.height.clamp(8, 8192));
        let region = match settings.region {
            Some([a, b, c, d]) => {
                let px = |v: f32, n: u32| ((v.clamp(0.0, 1.0) * n as f32).round() as u32).min(n);
                let (x0, x1) = (px(a.min(c), w), px(a.max(c), w));
                let (y0, y1) = (px(b.min(d), h), px(b.max(d), h));
                if x1 > x0 && y1 > y0 { [x0, y0, x1, y1] } else { [0, 0, w, h] }
            }
            None => [0, 0, w, h],
        };
        let base = base.filter(|b| b.len() == (w * h) as usize && region != [0, 0, w, h]);
        let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).clamp(1, 64);
        let shared = Arc::new(Shared {
            w,
            h,
            region,
            base,
            buf: Mutex::new(Buffers { accum: vec![[0.0; 4]; (w * h) as usize], aov: vec![[0.0; 10]; (w * h) as usize], row_spp: vec![0; h as usize] }),
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
        let [_, y0, _, y1] = self.shared.region;
        let total = (y1 - y0) as u64 * self.settings.spp.max(1) as u64;
        self.shared.rows_done.load(Ordering::Relaxed) as f32 / total as f32
    }

    /// Changes whenever new samples land.
    pub fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Relaxed)
    }

    /// Minimum samples per pixel accumulated so far.
    pub fn samples(&self) -> u32 {
        let [_, y0, _, y1] = self.shared.region;
        self.shared.buf.lock().map(|b| b.row_spp[y0 as usize..y1 as usize].iter().copied().min().unwrap_or(0)).unwrap_or(0)
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
                if !self.in_region(i) {
                    return self.shared.base.as_ref().map(|b| b[i]).unwrap_or([0.0; 4]);
                }
                let n = b.row_spp[i / w].max(1) as f32;
                [p[0] / n, p[1] / n, p[2] / n, p[3] / n]
            })
            .collect()
    }

    fn in_region(&self, i: usize) -> bool {
        let w = self.shared.w as usize;
        let (x, y) = ((i % w) as u32, (i / w) as u32);
        let [x0, y0, x1, y1] = self.shared.region;
        x >= x0 && x < x1 && y >= y0 && y < y1
    }

    /// Linear image, denoised when the job's settings ask for it.
    pub fn final_linear(&self) -> Vec<[f32; 4]> {
        if !self.settings.denoise {
            return self.linear();
        }
        let (w, h) = self.size();
        let (col, alb, nrm, var, gloss) = {
            let b = self.shared.buf.lock().unwrap();
            let wu = w as usize;
            let mut col = Vec::with_capacity(b.accum.len());
            let mut alb = Vec::with_capacity(b.accum.len());
            let mut nrm = Vec::with_capacity(b.accum.len());
            let mut var = Vec::with_capacity(b.accum.len());
            let mut gloss = Vec::with_capacity(b.accum.len());
            for (i, (p, a)) in b.accum.iter().zip(&b.aov).enumerate() {
                let n = b.row_spp[i / wu].max(1) as f32;
                let c = if self.in_region(i) { [p[0] / n, p[1] / n, p[2] / n, p[3] / n] } else { self.shared.base.as_ref().map(|b| b[i]).unwrap_or([0.0; 4]) };
                let mean_l = lum(Vec3::new(c[0], c[1], c[2]));
                // Variance of the pixel mean.
                let v = ((a[6] / n - mean_l * mean_l).max(0.0)) / n;
                col.push(c);
                alb.push(Vec3::new(a[0], a[1], a[2]) / n);
                nrm.push(Vec3::new(a[3], a[4], a[5]) / n);
                var.push(v);
                gloss.push(a[7] / n);
            }
            (col, alb, nrm, var, gloss)
        };
        let out = denoise(w as usize, h as usize, &col, &alb, &nrm, &var);
        // Mirror-like surfaces keep their (sharp) reflections; outside a region, the base.
        out.iter()
            .zip(&col)
            .zip(&gloss)
            .enumerate()
            .map(|(i, ((d, r), g))| {
                if !self.in_region(i) {
                    return *r;
                }
                let t = ((g - 0.6) / 0.3).clamp(0.0, 1.0);
                let k = t * t * (3.0 - 2.0 * t) * 0.75;
                [d[0] + (r[0] - d[0]) * k, d[1] + (r[1] - d[1]) * k, d[2] + (r[2] - d[2]) * k, d[3]]
            })
            .collect()
    }

    /// An 8-bit image of a render pass (beauty is tone-mapped and denoised as configured).
    pub fn pass_rgba8(&self, pass: Pass, exposure: f32, tm: ToneMap) -> Vec<u8> {
        if pass == Pass::Beauty {
            return self.rgba8(exposure, tm);
        }
        let b = self.shared.buf.lock().unwrap();
        let wu = self.shared.w as usize;
        let enc = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8;
        let raw = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        // Depth range over hit pixels.
        let (mut dmin, mut dmax) = (f32::MAX, 0.0f32);
        if pass == Pass::Depth {
            for a in &b.aov {
                if a[8] > 0.0 {
                    dmin = dmin.min(a[8]);
                    dmax = dmax.max(a[8]);
                }
            }
        }
        let mut out = Vec::with_capacity(b.aov.len() * 4);
        for (i, (a, p)) in b.aov.iter().zip(&b.accum).enumerate() {
            let n = b.row_spp[i / wu].max(1) as f32;
            let cov = (p[3] / n).clamp(0.0, 1.0);
            let inside = self.in_region(i) && b.row_spp[i / wu] > 0;
            if !inside {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let px: [u8; 4] = match pass {
                Pass::Albedo => [enc(a[0] / n), enc(a[1] / n), enc(a[2] / n), raw(cov)],
                Pass::Normal => {
                    let v = Vec3::new(a[3], a[4], a[5]).normalize_or_zero();
                    if v == Vec3::ZERO { [0, 0, 0, 0] } else { [raw(v.x * 0.5 + 0.5), raw(v.y * 0.5 + 0.5), raw(v.z * 0.5 + 0.5), 255] }
                }
                Pass::Depth => {
                    if a[8] <= 0.0 {
                        [0, 0, 0, 0]
                    } else {
                        // Brighter = closer, normalised to the visible depth range.
                        let t = if dmax > dmin { 1.0 - (a[8] - dmin) / (dmax - dmin) } else { 1.0 };
                        let v = raw(0.1 + 0.9 * t);
                        [v, v, v, 255]
                    }
                }
                Pass::ObjectId => {
                    let id = a[9] as u32;
                    if id == 0 {
                        [0, 0, 0, 0]
                    } else {
                        let h = id.wrapping_mul(2654435761).rotate_left(7) ^ 0x5bd1e995;
                        let c = |k: u32| 60 + ((h >> k) & 0xff) as u8 % 196;
                        [c(0), c(8), c(16), 255]
                    }
                }
                Pass::Beauty => unreachable!(),
            };
            out.extend_from_slice(&px);
        }
        out
    }

    /// Save every pass next to `path` as name_beauty.png, name_albedo.png, …
    pub fn save_passes(&self, path: &std::path::Path, exposure: f32, tm: ToneMap) -> Result<Vec<String>, String> {
        let (w, h) = self.size();
        let stem = path.with_extension("");
        let mut out = Vec::new();
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(dir);
        }
        for p in Pass::ALL {
            let file = format!("{}_{}.png", stem.display(), p.name().to_lowercase().replace(' ', ""));
            let img = image::RgbaImage::from_raw(w, h, self.pass_rgba8(p, exposure, tm)).ok_or("image size")?;
            img.save(&file).map_err(|e| format!("{file}: {e}"))?;
            out.push(file);
        }
        Ok(out)
    }

    /// Tone-mapped sRGB RGBA8 (rows top to bottom). Rows without samples are transparent.
    pub fn rgba8(&self, exposure: f32, tm: ToneMap) -> Vec<u8> {
        tonemap(&self.final_linear(), exposure, tm)
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

/// Edge-avoiding à-trous wavelet denoiser (SVGF-style, single frame): the colour is divided
/// by the first-hit albedo, filtered in 5 passes of a 5×5 B3-spline kernel with growing
/// stride, guided by normals, albedo and the estimated per-pixel luminance variance, and
/// multiplied back. Texture detail and geometric edges survive; Monte Carlo noise doesn't.
pub fn denoise(w: usize, h: usize, col: &[[f32; 4]], alb: &[Vec3], nrm: &[Vec3], var: &[f32]) -> Vec<[f32; 4]> {
    let n = w * h;
    if n == 0 || col.len() != n {
        return col.to_vec();
    }
    let demod = |c: [f32; 4], a: Vec3| Vec3::new(c[0], c[1], c[2]) / a.max(Vec3::splat(0.15));
    let mut irr: Vec<Vec3> = (0..n).map(|i| demod(col[i], alb[i])).collect();
    let mut v: Vec<f32> = var.iter().zip(alb).map(|(v, a)| v / a.max(Vec3::splat(0.15)).dot(Vec3::splat(1.0 / 3.0)).powi(2)).collect();
    let nn: Vec<Vec3> = nrm.iter().map(|x| x.normalize_or_zero()).collect();
    let cover: Vec<bool> = col.iter().map(|c| c[3] > 0.01).collect();
    const H: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
    let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).clamp(1, 32);
    for it in 0..5 {
        let step = 1i32 << it;
        let (src, sv) = (&irr, &v);
        // 3×3 blurred variance for a stable edge-stopping sigma.
        let rows: Vec<(Vec<Vec3>, Vec<f32>)> = std::thread::scope(|scope| {
            let per = h.div_ceil(threads);
            let hs: Vec<_> = (0..threads)
                .map(|t| {
                    let (nn, cover, alb) = (&nn, &cover, alb);
                    scope.spawn(move || {
                        let mut out_c = Vec::new();
                        let mut out_v = Vec::new();
                        for y in (t * per)..((t + 1) * per).min(h) {
                            for x in 0..w {
                                let i = y * w + x;
                                if !cover[i] {
                                    out_c.push(src[i]);
                                    out_v.push(sv[i]);
                                    continue;
                                }
                                let mut vb = 0.0;
                                let mut vn = 0.0;
                                for dy in -1i32..=1 {
                                    for dx in -1i32..=1 {
                                        let (qx, qy) = (x as i32 + dx, y as i32 + dy);
                                        if qx >= 0 && qy >= 0 && (qx as usize) < w && (qy as usize) < h {
                                            vb += sv[qy as usize * w + qx as usize];
                                            vn += 1.0;
                                        }
                                    }
                                }
                                let sigma_l = 4.0 * (vb / vn).max(0.0).sqrt() + 1e-4;
                                let (cp, np, ap) = (src[i], nn[i], alb[i]);
                                let lp = lum(cp);
                                let (mut sum, mut wsum, mut vsum) = (Vec3::ZERO, 0.0f32, 0.0f32);
                                for (ky, hy) in H.iter().enumerate() {
                                    for (kx, hx) in H.iter().enumerate() {
                                        let qx = x as i32 + (kx as i32 - 2) * step;
                                        let qy = y as i32 + (ky as i32 - 2) * step;
                                        if qx < 0 || qy < 0 || qx as usize >= w || qy as usize >= h {
                                            continue;
                                        }
                                        let q = qy as usize * w + qx as usize;
                                        if !cover[q] {
                                            continue;
                                        }
                                        let wn = np.dot(nn[q]).max(0.0).powi(64);
                                        let da = (ap - alb[q]).length_squared();
                                        let wa = (-da / 0.02).exp();
                                        let wl = (-(lp - lum(src[q])).abs() / sigma_l).exp();
                                        let wgt = hx * hy * wn * wa * wl;
                                        sum += src[q] * wgt;
                                        wsum += wgt;
                                        vsum += wgt * wgt * sv[q];
                                    }
                                }
                                if wsum > 1e-8 {
                                    out_c.push(sum / wsum);
                                    out_v.push(vsum / (wsum * wsum));
                                } else {
                                    out_c.push(cp);
                                    out_v.push(sv[i]);
                                }
                            }
                        }
                        (out_c, out_v)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut nc = Vec::with_capacity(n);
        let mut nv = Vec::with_capacity(n);
        for (c, vv) in rows {
            nc.extend(c);
            nv.extend(vv);
        }
        irr = nc;
        v = nv;
    }
    (0..n)
        .map(|i| {
            if !cover[i] {
                return col[i];
            }
            let c = irr[i] * alb[i].max(Vec3::splat(0.15));
            [c.x, c.y, c.z, col[i][3]]
        })
        .collect()
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
    let [rx0, ry0, rx1, ry1] = sh.region;
    let (x0, x1) = (rx0 as usize, rx1 as usize);
    let aspect = w as f32 / h as f32;
    let mut row = vec![[0.0f32; 4]; w as usize];
    let mut row_aov = vec![[0.0f32; 10]; w as usize];
    for pass in 0..s.spp.max(1) {
        let mut y = ry0 + wi;
        while y < ry1.min(h) {
            if sh.cancel.load(Ordering::Relaxed) {
                return;
            }
            for x in rx0..rx1 {
                let mut rng = Rng::new(((pass as u64) << 40) ^ ((y as u64) << 20) ^ x as u64 ^ 0x9E37_79B9_7F4A_7C15);
                let (jx, jy) = if pass == 0 { (0.5, 0.5) } else { (rng.f(), rng.f()) };
                let sx = ((x as f32 + jx) / w as f32) * 2.0 - 1.0;
                let sy = 1.0 - ((y as f32 + jy) / h as f32) * 2.0;
                let lens = if pass == 0 { (0.0, 0.0) } else { (rng.f(), rng.f()) };
                let (o, d) = cam.ray(sx, sy, aspect, lens);
                let (c, a, aov) = trace(sc, s, o, d, &mut rng);
                let c = if c.is_finite() { c } else { Vec3::ZERO };
                row[x as usize] = [c.x, c.y, c.z, a];
                let lm = lum(c);
                row_aov[x as usize] = [aov.albedo.x, aov.albedo.y, aov.albedo.z, aov.normal.x, aov.normal.y, aov.normal.z, lm * lm, aov.gloss, aov.depth, aov.obj as f32];
            }
            {
                let mut b = sh.buf.lock().unwrap();
                let base = (y * w) as usize;
                for (dst, src) in b.accum[base..base + w as usize].iter_mut().zip(&row).skip(x0).take(x1 - x0) {
                    for k in 0..4 {
                        dst[k] += src[k];
                    }
                }
                for (dst, src) in b.aov[base..base + w as usize].iter_mut().zip(&row_aov).skip(x0).take(x1 - x0) {
                    for k in 0..8 {
                        dst[k] += src[k];
                    }
                    if pass == 0 {
                        dst[8] = src[8];
                        dst[9] = src[9];
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
    fn denoiser_reduces_error_and_keeps_edges() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 10 -h 10 -n ground; polyCube -n box; move 0 0.5 0 box; setColor 0.9 0.2 0.1 box; pointLight -i 1.2 -p 2 3 2; camera -p 0 3 5 -aim 0 0.3 0 -n cam").unwrap();
        let base = RenderSettings { width: 64, height: 40, env: 0.4, ..Default::default() };
        let reference = app.render_now(Some("cam"), RenderSettings { spp: 256, denoise: false, ..base.clone() }).unwrap().linear();
        let mut noisy = app.render_now(Some("cam"), RenderSettings { spp: 4, ..base.clone() }).unwrap();
        // Measure away from geometric edges (anti-aliasing differs at 4 vs 256 spp).
        let w = 64usize;
        let smooth: Vec<usize> = (0..reference.len())
            .filter(|&i| {
                let (x, y) = (i % w, i / w);
                x > 0 && y > 0 && x < w - 1 && y < 39 && [i - 1, i + 1, i - w, i + w].iter().all(|&q| (0..4).all(|k| (reference[q][k] - reference[i][k]).abs() < 0.08))
            })
            .collect();
        assert!(smooth.len() > 1000);
        let rmse = |img: &[[f32; 4]]| (smooth.iter().map(|&i| (0..3).map(|k| (img[i][k] - reference[i][k]).powi(2)).sum::<f32>()).sum::<f32>() / smooth.len() as f32).sqrt();
        noisy.settings.denoise = false;
        let e_raw = rmse(&noisy.final_linear());
        noisy.settings.denoise = true;
        let den = noisy.final_linear();
        let e_dn = rmse(&den);
        eprintln!("rmse raw {e_raw:.4} denoised {e_dn:.4}");
        assert!(e_dn < e_raw * 0.6, "raw {e_raw} denoised {e_dn}");
        // Coverage (alpha) untouched.
        assert!(den.iter().zip(&noisy.linear()).all(|(a, b)| (a[3] - b[3]).abs() < 1e-6));
    }

    #[test]
    fn area_light_soft_shadows() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 20 -h 20 -n ground; polyCube -w 1 -h 2 -d 1 -n post; move 0 1 0 post; areaLight -w 3 -h 3 -i 2 -p 0 4 0 -aim 0 0 0 -n soft; camera -p 0 14 0.001 -aim 0 0 0 -n top").unwrap();
        // Top view: the light's back faces the camera but lights are invisible to camera rays.
        let s = RenderSettings { width: 80, height: 80, spp: 64, env: 0.0, denoise: false, ..Default::default() };
        let img = app.render_now(Some("top"), s.clone()).unwrap().linear();
        let at = |x: usize, y: usize| lum(Vec3::new(img[y * 80 + x][0], img[y * 80 + x][1], img[y * 80 + x][2]));
        // Find the brightness profile along a row through the post's shadow region... the post
        // blocks light directly below the area light, so measure along +X from the post edge.
        let row: Vec<f32> = (40..80).map(|x| at(x, 40)).collect();
        eprintln!("profile {:?}", row.iter().map(|v| (v * 100.0).round() / 100.0).collect::<Vec<_>>());
        // The first pixels are the lit top of the post; the ground starts where the value drops.
        let start = row.windows(2).position(|w| w[1] < w[0] * 0.5).expect("post edge") + 1;
        let ground = &row[start..];
        let (peak_i, peak) = ground.iter().copied().enumerate().fold((0, 0.0f32), |a, (i, v)| if v > a.1 { (i, v) } else { a });
        let far = *ground.last().unwrap();
        assert!(peak > far * 2.0, "area light falls off with distance: peak {peak} far {far}");
        // Soft shadow: right beside the post the ground is partly occluded and brightens
        // gradually over several pixels toward the peak.
        assert!(peak_i >= 4, "penumbra spans pixels (peak at {peak_i})");
        assert!(ground[0] < peak * 0.75, "contact shadow darker than open ground");
        // USD exports a RectLight.
        let (bytes, _) = crate::export::usd::export(&app, &crate::export::ExportOptions::from_app(&app, false)).unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("def RectLight \"softShape\""));
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
        if let Ok(fd) = std::env::var("RT_DOF") {
            execute(&mut app, &format!("setAttr shot.depthOfField 1; setAttr shot.fStop 2; setAttr shot.dofBlurScale 60; setAttr shot.focusDistance {fd}")).unwrap();
        }
        if let Ok(rig) = std::env::var("RT_RIG") {
            execute(&mut app, &format!("delete sun fill; lightRig -preset {rig}")).unwrap();
        }
        let spp = std::env::var("RT_SPP").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        let s = RenderSettings { width: 640, height: 360, spp, env: 0.35, env_background: true, ..Default::default() };
        let mut job = app.render_now(Some("shot"), s.clone()).unwrap();
        eprintln!("rendered in {:.2}s, {} tris", job.elapsed().as_secs_f32(), job.tri_count);
        let t = std::time::Instant::now();
        job.save_png(std::path::Path::new(&out), 0.0, ToneMap::Filmic).unwrap();
        eprintln!("denoise+save {:.2}s", t.elapsed().as_secs_f32());
        job.settings.denoise = false;
        job.save_png(std::path::Path::new(&out.replace(".png", "_raw.png")), 0.0, ToneMap::Filmic).unwrap();
        if std::env::var("RT_PASSES").is_ok() {
            job.settings.denoise = true;
            job.save_passes(std::path::Path::new(&out.replace(".png", "_p.png")), 0.0, ToneMap::Filmic).unwrap();
        }
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    use crate::app::App;
    use crate::commands::execute;

    fn solid_png(path: &std::path::Path, rgb: [u8; 3]) {
        image::RgbImage::from_pixel(8, 8, image::Rgb(rgb)).save(path).unwrap();
    }

    #[test]
    fn normal_roughness_metalness_maps() {
        let dir = std::env::temp_dir().join("rm_maps_test");
        let _ = std::fs::create_dir_all(&dir);
        let tilt = dir.join("tilt_n.png");
        // Tangent-space normal tilted ~60° toward +U.
        solid_png(&tilt, [238, 128, 192]);
        let black = dir.join("black.png");
        solid_png(&black, [0, 0, 0]);
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 4 -h 4 -n p; shadingNode -asShader lambert -n m -rgb 1 1 1; hyperShade -assign m p; directionalLight -i 1 -rot -90 0 0; camera -p 0 5 0.001 -aim 0 0 0 -n top").unwrap();
        let s = RenderSettings { width: 16, height: 16, spp: 4, env: 0.0, denoise: false, ..Default::default() };
        let centre = |app: &mut App| {
            let img = app.render_now(Some("top"), s.clone()).unwrap().linear();
            img[8 * 16 + 8][1]
        };
        let flat = centre(&mut app);
        execute(&mut app, &format!("setAttr m.normalMap \"{}\"", tilt.display())).unwrap();
        let tilted = centre(&mut app);
        assert!(tilted < flat * 0.8 && tilted > flat * 0.2, "tilted normal gets less light: {tilted} vs {flat}");
        // glTF export carries the normal map and a packed metallic-roughness texture; re-import restores both.
        execute(&mut app, &format!("setAttr m.roughnessMap \"{}\"; setAttr m.metalnessMap \"{}\"", black.display(), black.display())).unwrap();
        let glb = dir.join("maps.glb");
        execute(&mut app, &format!("file -export \"{}\"", glb.display())).unwrap();
        let (doc, _) = crate::export::gltf::parse_glb(&std::fs::read(&glb).unwrap());
        let mat = &doc["materials"][0];
        assert!(mat["normalTexture"]["index"].is_u64());
        assert!(mat["pbrMetallicRoughness"]["metallicRoughnessTexture"]["index"].is_u64());
        // Occlusion + emission maps: emission makes the plane glow even without lights.
        let grey = dir.join("grey.png");
        solid_png(&grey, [128, 128, 128]);
        let orange = dir.join("orange.png");
        solid_png(&orange, [255, 128, 0]);
        execute(&mut app, &format!("setAttr m.occlusionMap \"{}\"; setAttr m.emissionMap \"{}\"; setAttr m.incandescence 1 1 1", grey.display(), orange.display())).unwrap();
        let glow = app.render_now(Some("top"), s.clone()).unwrap().linear()[8 * 16 + 8];
        assert!(glow[0] > 0.9 && glow[2] < glow[0] * 0.5, "orange emission: {glow:?}");
        execute(&mut app, &format!("file -export \"{}\"", glb.display())).unwrap();
        let (doc, _) = crate::export::gltf::parse_glb(&std::fs::read(&glb).unwrap());
        let mat = &doc["materials"][0];
        assert!(mat["occlusionTexture"]["index"].is_u64() && mat["emissiveTexture"]["index"].is_u64());
        assert!(doc["meshes"][0]["primitives"][0]["attributes"]["TANGENT"].is_u64(), "tangents for normal maps");
        let mut app2 = App::new();
        app2.scene.nodes.clear();
        execute(&mut app2, &format!("file -import \"{}\"", glb.display())).unwrap();
        let m2 = app2.scene.materials.iter().find(|m| m.name == "m").unwrap();
        assert!(m2.normal_map.is_some() && m2.roughness_map.is_some() && m2.metalness_map.is_some());
        assert!(m2.occlusion_map.is_some() && m2.emission_map.is_some());
        // USD connects the maps.
        let (u, _) = crate::export::usd::export(&app, &crate::export::ExportOptions::from_app(&app, false)).unwrap();
        let u = String::from_utf8(u).unwrap();
        assert!(u.contains("normal3f inputs:normal.connect") && u.contains("inputs:roughness.connect") && u.contains("inputs:metallic.connect"));
        assert!(u.contains("token inputs:sourceColorSpace = \"raw\""));
        assert!(u.contains("float inputs:occlusion.connect") && u.contains("color3f inputs:emissiveColor.connect"));
        execute(&mut app, "areaLight -n soft -w 3 -h 1").unwrap();
        let (u2, _) = crate::export::usd::export(&app, &crate::export::ExportOptions::from_app(&app, false)).unwrap();
        std::fs::write(dir.join("maps.usda"), u2).unwrap();
    }
}

#[cfg(test)]
mod pass_tests {
    use super::*;
    use crate::app::App;
    use crate::commands::execute;

    #[test]
    fn passes_region_and_dof() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -n near; move -1.2 1 2 near; polyCube -w 2 -h 2 -d 2 -n far; move 1.5 1 -4 far; polyPlane -w 30 -h 30 -n ground; directionalLight -rot -60 30 0; camera -p 0 2 8 -aim 0 1 0 -n cam").unwrap();
        let s = RenderSettings { width: 64, height: 40, spp: 2, env: 0.3, ..Default::default() };
        let job = app.render_now(Some("cam"), s.clone()).unwrap();
        let px = |img: &[u8], x: u32, y: u32| { let o = ((y * 64 + x) * 4) as usize; [img[o], img[o + 1], img[o + 2], img[o + 3]] };
        // Object ID: at least three distinct objects.
        let ids = job.pass_rgba8(Pass::ObjectId, 0.0, ToneMap::Srgb);
        let mut colors: Vec<[u8; 4]> = ids.chunks_exact(4).filter(|c| c[3] > 0).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        colors.sort();
        colors.dedup();
        assert!(colors.len() >= 3, "{} ids", colors.len());
        // Depth: the near sphere is brighter than the far cube.
        let depth = job.pass_rgba8(Pass::Depth, 0.0, ToneMap::Srgb);
        let world_to_px = |p: Vec3| {
            let c = RtCamera::from_node(app.scene.world_matrix(app.scene.find_by_name("cam").unwrap()), &crate::look::CameraShape::default());
            let d = p - c.eye;
            let z = d.dot(c.fwd);
            let x = d.dot(c.right) / (z * c.tan_half * 1.6);
            let y = d.dot(c.up) / (z * c.tan_half);
            (((x + 1.0) * 0.5 * 64.0) as u32, ((1.0 - y) * 0.5 * 40.0) as u32)
        };
        let (nx, ny) = world_to_px(Vec3::new(-1.2, 1.0, 2.0));
        let (fx, fy) = world_to_px(Vec3::new(1.5, 1.0, -4.0));
        assert!(px(&depth, nx, ny)[0] > px(&depth, fx, fy)[0] + 30, "near {:?} far {:?}", px(&depth, nx, ny), px(&depth, fx, fy));
        // Normal pass: the sphere's centre faces the camera (+Z-ish → blue high).
        let nrm = job.pass_rgba8(Pass::Normal, 0.0, ToneMap::Srgb);
        assert!(px(&nrm, nx, ny)[2] > 200, "{:?}", px(&nrm, nx, ny));
        // Passes on disk.
        let dir = std::env::temp_dir().join("rm_passes");
        let files = job.save_passes(&dir.join("shot.png"), 0.0, ToneMap::Srgb).unwrap();
        assert_eq!(files.len(), 5);
        assert!(files.iter().all(|f| std::path::Path::new(f).exists()));
        // Region: only the region is re-rendered; outside comes from the base image.
        let base = job.final_linear();
        let mut rs = s.clone();
        rs.region = Some([0.25, 0.25, 0.5, 0.75]);
        rs.env = 2.0; // much brighter sky inside the region
        let c = RtCamera::from_node(app.scene.world_matrix(app.scene.find_by_name("cam").unwrap()), &crate::look::CameraShape::default());
        let sc = std::sync::Arc::new(RtScene::build(&app.build_geometry(), &c));
        let rj = RenderJob::start_with_base(sc, c, rs, Some(base.clone())).join();
        let out = rj.final_linear();
        assert_eq!(out[0], base[0], "outside the region unchanged");
        let i_in = (20 * 64 + 24) as usize;
        assert!(lum(Vec3::new(out[i_in][0], out[i_in][1], out[i_in][2])) > lum(Vec3::new(base[i_in][0], base[i_in][1], base[i_in][2])), "inside re-rendered");
        // Depth of field: focused on the near sphere, the far cube's edges get soft.
        let edge_energy = |img: &[[f32; 4]], x0: u32, x1: u32, y0: u32, y1: u32| {
            let mut e = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * 64 + x) as usize;
                    e += (lum(Vec3::new(img[i][0], img[i][1], img[i][2])) - lum(Vec3::new(img[i + 1][0], img[i + 1][1], img[i + 1][2]))).abs();
                }
            }
            e
        };
        let sharp = app.render_now(Some("cam"), RenderSettings { spp: 64, denoise: false, ..s.clone() }).unwrap().linear();
        execute(&mut app, "setAttr cam.depthOfField 1; setAttr cam.fStop 1.4; setAttr cam.dofBlurScale 150; setAttr cam.focusDistance 6.6").unwrap();
        let blurred = app.render_now(Some("cam"), RenderSettings { spp: 64, denoise: false, ..s.clone() }).unwrap().linear();
        let (fx0, fx1) = (fx.saturating_sub(8), (fx + 8).min(62));
        let (fy0, fy1) = (fy.saturating_sub(6), (fy + 6).min(39));
        let e_sharp = edge_energy(&sharp, fx0, fx1, fy0, fy1);
        let e_blur = edge_energy(&blurred, fx0, fx1, fy0, fy1);
        eprintln!("far edge energy sharp {e_sharp:.3} dof {e_blur:.3}");
        assert!(e_blur < e_sharp * 0.8, "far cube is defocused");
    }
}
