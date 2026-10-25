//! wgpu viewport renderer. Each viewport renders the scene into its own offscreen
//! MSAA colour + depth target, which is then blitted into egui's render pass.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use std::collections::HashMap;
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuVertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    /// sRGB colour, alpha = 1 - transparency.
    pub col: [f32; 4],
    /// (specular weight, roughness, metalness, unused)
    pub mat: [f32; 4],
    /// Emission (sRGB) in xyz, w = 1 when the vertex has a UV.
    pub emi: [f32; 4],
    pub uv: [f32; 2],
    /// Texture layers + 1 of the normal, roughness and metalness maps (0 = none);
    /// w = occlusion layer + 1 + 1024 × (emission layer + 1).
    pub maps: [f32; 4],
}

impl GpuVertex {
    /// Default surface parameters (lambert-like).
    pub const DEFAULT_MAT: [f32; 4] = [0.0, 0.8, 0.0, 0.0];
    /// Unlit line vertex.
    pub fn line(pos: glam::Vec3, c: [f32; 3]) -> Self {
        GpuVertex { pos: pos.to_array(), nrm: [0.0; 3], col: [c[0], c[1], c[2], 1.0], mat: [0.0; 4], emi: [0.0; 4], uv: [0.0; 2], maps: [0.0; 4] }
    }
}

/// A light as uploaded to the GPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLight {
    /// xyz world position, w = kind (0 ambient, 1 directional, 2 point, 3 spot).
    pub pos: [f32; 4],
    /// xyz direction the light travels, w = cos(outer cone half angle).
    pub dir: [f32; 4],
    /// rgb (linear) * intensity, w = cos(inner cone half angle).
    pub col: [f32; 4],
}

pub const MAX_LIGHTS: usize = 8;
const SHADOW_SIZE: u32 = 2048;

/// An immutable block of vertices with a unique id (so the renderer can skip re-uploading
/// blocks that did not change between frames).
pub struct Chunk {
    pub id: u64,
    pub data: Vec<GpuVertex>,
    /// World-space bounds of the block (min, max).
    pub bounds: Option<(glam::Vec3, glam::Vec3)>,
    /// Viewport-only geometry (e.g. a smoke preview): the path tracer skips it.
    pub preview: bool,
}

static CHUNK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Chunk {
    pub fn with_bounds(data: Vec<GpuVertex>, bounds: Option<(glam::Vec3, glam::Vec3)>) -> Arc<Chunk> {
        Arc::new(Chunk { id: CHUNK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed), data, bounds, preview: false })
    }

    /// Viewport-only geometry (not rendered by the path tracer).
    pub fn preview(data: Vec<GpuVertex>, bounds: Option<(glam::Vec3, glam::Vec3)>) -> Arc<Chunk> {
        Arc::new(Chunk { id: CHUNK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed), data, bounds, preview: true })
    }
}

/// Geometry shared by all viewports for one UI frame (world space).
#[derive(Default)]
pub struct FrameGeometry {
    pub generation: u64,
    pub tris: Vec<Arc<Chunk>>,
    /// Edges of all meshes (wireframe mode / wireframe-on-shaded).
    pub wire_all: Vec<Arc<Chunk>>,
    /// Edges of selected meshes only, plus locators and always-drawn lines.
    pub wire_sel: Vec<Arc<Chunk>>,
    /// Rig lines drawn on top of everything (joints, bones, IK handles, control curves).
    pub overlay: Vec<GpuVertex>,
    /// Triangles of transparent materials (drawn blended after the opaque ones).
    pub tris_transp: Vec<Arc<Chunk>>,
    /// The node each chunk of `tris` / `tris_transp` came from (ID passes and mattes).
    pub tri_nodes: Vec<crate::scene::NodeId>,
    pub transp_nodes: Vec<crate::scene::NodeId>,
    /// Smoke / fire volumes for the path tracer.
    pub volumes: Vec<Arc<crate::fluid::VolumeGeom>>,
    pub lights: Vec<GpuLight>,
    /// Texture layers (layer 0 = UV checker). Shared between frames while unchanged.
    pub textures: Option<Arc<crate::texture::TextureSet>>,
    /// Area lights for the path tracer: (centre, half-extent X, half-extent Y, emission
    /// direction, linear colour × intensity).
    pub area_lights: Vec<(glam::Vec3, glam::Vec3, glam::Vec3, glam::Vec3, [f32; 3])>,
    /// Camera icons, drawn per view (a view doesn't draw the camera it looks through).
    pub camera_icons: Vec<(crate::scene::NodeId, Vec<GpuVertex>)>,
    /// Shadow-casting light: (light view-projection, index into `lights`).
    pub shadow: Option<(glam::Mat4, usize)>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    /// xyz light direction (toward light), w = line depth offset.
    light: [f32; 4],
    /// xyz eye position, w = flags (1 all lights, 2 shadows, 4 x-ray).
    cam: [f32; 4],
    shadow_vp: [[f32; 4]; 4],
    /// x = light count, y = shadow light index (-1 none), z = x-ray alpha, w = shadow texel.
    info: [f32; 4],
    lights: [GpuLight; MAX_LIGHTS],
}

pub struct ViewCallback {
    pub view_id: usize,
    pub rect: egui::Rect,
    pub view_proj: glam::Mat4,
    pub light_dir: glam::Vec3,
    pub geom: Arc<FrameGeometry>,
    pub grid: Arc<Vec<GpuVertex>>,
    pub draw_shaded: bool,
    pub draw_wire_all: bool,
    pub bg: [f32; 3],
    pub eye: glam::Vec3,
    pub all_lights: bool,
    pub shadows: bool,
    pub xray: bool,
    pub textured: bool,
    pub uv_checker: bool,
}

struct Target {
    size: [u32; 2],
    msaa_view: Option<wgpu::TextureView>,
    resolve_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    blit_bg: wgpu::BindGroup,
    grid_buf: Option<(wgpu::Buffer, u32)>,
}

struct DynBuffer {
    /// Vertex buffer pages, each within the device's `max_buffer_size` (big imports can need
    /// more than one 256 MB buffer): (buffer, capacity in bytes).
    pages: Vec<(wgpu::Buffer, u64)>,
    /// Vertices per full page (a multiple of 6, so triangles and lines never straddle pages).
    page_verts: u64,
    count: u32,
    /// (chunk id, vertex offset) of what is currently in the buffer.
    uploaded: Vec<(u64, u64)>,
}

impl DynBuffer {
    fn new() -> Self {
        DynBuffer { pages: Vec::new(), page_verts: 0, count: 0, uploaded: Vec::new() }
    }

    /// Make room for `total` vertices; returns true if the pages were (re)allocated.
    fn reserve(&mut self, device: &wgpu::Device, total: u64) -> bool {
        let vsize = std::mem::size_of::<GpuVertex>() as u64;
        let max_bytes = device.limits().max_buffer_size.min(1 << 30);
        let page_verts = (max_bytes / vsize / 6 * 6).max(6);
        let pages = total.div_ceil(page_verts) as usize;
        let last_need = (total - (pages as u64 - 1) * page_verts) * vsize;
        let fits = self.page_verts == page_verts
            && self.pages.len() == pages
            && self.pages.last().is_some_and(|(_, cap)| *cap >= last_need)
            && self.pages[..pages - 1].iter().all(|(_, cap)| *cap >= page_verts * vsize);
        if fits {
            return false;
        }
        self.page_verts = page_verts;
        self.pages = (0..pages)
            .map(|i| {
                let size = if i + 1 < pages { page_verts * vsize } else { last_need.next_power_of_two().max(4096).min(page_verts * vsize) };
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rm_vertices"),
                    size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                (buf, size)
            })
            .collect();
        true
    }

    /// Write vertices starting at a global vertex offset (split across pages as needed).
    fn write(&self, queue: &wgpu::Queue, mut off: u64, mut data: &[GpuVertex]) {
        let vsize = std::mem::size_of::<GpuVertex>() as u64;
        while !data.is_empty() {
            let page = (off / self.page_verts) as usize;
            let local = off % self.page_verts;
            let n = ((self.page_verts - local) as usize).min(data.len());
            queue.write_buffer(&self.pages[page].0, local * vsize, bytemuck::cast_slice(&data[..n]));
            data = &data[n..];
            off += n as u64;
        }
    }

    fn upload_chunks(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, chunks: &[Arc<Chunk>]) {
        let total: u64 = chunks.iter().map(|c| c.data.len() as u64).sum();
        self.count = total as u32;
        if total == 0 {
            return;
        }
        if self.reserve(device, total) {
            self.uploaded.clear();
        }
        let mut off = 0u64;
        let mut now = Vec::with_capacity(chunks.len());
        for (i, c) in chunks.iter().enumerate() {
            if !c.data.is_empty() && self.uploaded.get(i) != Some(&(c.id, off)) {
                self.write(queue, off, &c.data);
            }
            now.push((c.id, off));
            off += c.data.len() as u64;
        }
        self.uploaded = now;
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[GpuVertex]) {
        self.count = data.len() as u32;
        if data.is_empty() {
            return;
        }
        self.reserve(device, data.len() as u64);
        self.write(queue, 0, data);
    }

    /// Draw all vertices (one draw per page).
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let mut left = self.count as u64;
        for (buf, _) in &self.pages {
            if left == 0 {
                break;
            }
            let n = left.min(self.page_verts);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..n as u32, 0..1);
            left -= n;
        }
    }
}

pub struct Renderer {
    tri_pipeline: wgpu::RenderPipeline,
    tri_blend_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_view: wgpu::TextureView,
    shadow_bg: wgpu::BindGroup,
    shadow_layout: wgpu::BindGroupLayout,
    cmp_sampler: wgpu::Sampler,
    tex_sampler: wgpu::Sampler,
    tex_view: wgpu::TextureView,
    tex_key: u64,
    shadow_uniform_buf: wgpu::Buffer,
    shadow_uniform_bg: wgpu::BindGroup,
    shadow_gen: u64,
    tris_transp: DynBuffer,
    line_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    samples: u32,
    targets: HashMap<usize, Target>,
    uploaded_gen: u64,
    tris: DynBuffer,
    wire_all: DynBuffer,
    wire_sel: DynBuffer,
    overlay: DynBuffer,
}

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADER: &str = r#"
struct Light { pos: vec4<f32>, dir: vec4<f32>, col: vec4<f32> };
struct U {
    vp: mat4x4<f32>,
    light: vec4<f32>,
    cam: vec4<f32>,
    shadow_vp: mat4x4<f32>,
    info: vec4<f32>,
    lights: array<Light, 8>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var shadow_tex: texture_depth_2d;
@group(1) @binding(1) var shadow_samp: sampler_comparison;
@group(1) @binding(2) var tex_arr: texture_2d_array<f32>;
@group(1) @binding(3) var tex_samp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>, @location(1) nrm: vec3<f32>, @location(2) col: vec4<f32>,
    @location(3) mat: vec4<f32>, @location(4) emi: vec4<f32>, @location(5) uv: vec2<f32>,
    @location(6) maps: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>, @location(0) nrm: vec3<f32>, @location(1) col: vec4<f32>,
    @location(2) wpos: vec3<f32>, @location(3) mat: vec4<f32>, @location(4) emi: vec4<f32>,
    @location(5) uv: vec2<f32>, @location(6) maps: vec4<f32>,
};

@vertex fn vs_tri(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.vp * vec4<f32>(v.pos, 1.0);
    o.nrm = v.nrm;
    o.col = v.col;
    o.wpos = v.pos;
    o.mat = v.mat;
    o.emi = v.emi;
    o.uv = v.uv;
    o.maps = v.maps;
    return o;
}

@vertex fn vs_shadow(v: VIn) -> @builtin(position) vec4<f32> {
    return u.vp * vec4<f32>(v.pos, 1.0);
}

fn shadow_factor(p: vec3<f32>, n: vec3<f32>) -> f32 {
    let c = u.shadow_vp * vec4<f32>(p + n * 0.02, 1.0);
    let ndc = c.xyz / c.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 || ndc.z < 0.0) {
        return 1.0;
    }
    let t = u.info.w;
    var s = 0.0;
    for (var x = -1; x <= 1; x = x + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            s = s + textureSampleCompareLevel(shadow_tex, shadow_samp, uv + vec2<f32>(f32(x), f32(y)) * t, ndc.z - 0.0015);
        }
    }
    return s / 9.0;
}

fn spec_term(n: vec3<f32>, l: vec3<f32>, v: vec3<f32>, rough: f32) -> f32 {
    let r = clamp(rough, 0.03, 1.0);
    let shin = clamp(2.0 / (r * r * r * r + 0.0001) - 2.0, 1.0, 4096.0);
    let h = normalize(l + v);
    return pow(max(dot(n, h), 0.0), shin) * (shin + 8.0) / 25.13;
}

struct MapSamples {
    nrm: vec4<f32>, rough: vec4<f32>, metal: vec4<f32>, ao: vec4<f32>, emis: vec4<f32>,
    dp1: vec3<f32>, dp2: vec3<f32>, duv1: vec2<f32>, duv2: vec2<f32>,
};

fn shade(i: VOut, ff: bool, texel: vec4<f32>, ms: MapSamples) -> vec4<f32> {
    var n = normalize(i.nrm);
    if (!ff) { n = -n; }
    let flags = u32(u.cam.w + 0.5);
    let v = normalize(u.cam.xyz - i.wpos);
    var base = pow(i.col.rgb, vec3<f32>(2.2));
    let has_uv = i.emi.w > 0.5;
    let maps_on = has_uv && (flags & 8u) != 0u && (flags & 16u) == 0u;
    var rough = i.mat.y;
    var metal = i.mat.z;
    if (maps_on && i.maps.x > 0.5) {
        // Cotangent frame from screen-space derivatives (no stored tangents needed).
        let dp2perp = cross(ms.dp2, n);
        let dp1perp = cross(n, ms.dp1);
        let t = dp2perp * ms.duv1.x + dp1perp * ms.duv2.x;
        let b = dp2perp * ms.duv1.y + dp1perp * ms.duv2.y;
        let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
        let tsn = pow(ms.nrm.rgb, vec3<f32>(1.0 / 2.2)) * 2.0 - 1.0;
        n = normalize(t * inv * tsn.x + b * inv * tsn.y + n * max(tsn.z, 0.05));
    }
    if (maps_on && i.maps.y > 0.5) { rough = rough * pow(ms.rough.r, 1.0 / 2.2); }
    if (maps_on && i.maps.z > 0.5) { metal = metal * pow(ms.metal.r, 1.0 / 2.2); }
    let packed = u32(i.maps.w + 0.5);
    var ao = 1.0;
    if (maps_on && (packed % 1024u) > 0u) { ao = pow(ms.ao.r, 1.0 / 2.2); }
    var emis_tex = vec3<f32>(1.0);
    if (maps_on && (packed / 1024u) > 0u) { emis_tex = ms.emis.rgb; }
    let layer = i32(i.mat.w + 0.5) - 1;
    if (has_uv && (flags & 16u) != 0u) {
        base = texel.rgb;
    } else if (has_uv && layer >= 0 && (flags & 8u) != 0u) {
        base = texel.rgb;
    }
    let spec_w = i.mat.x;
    
    let dcol = base * (1.0 - metal);
    let scol = mix(vec3<f32>(spec_w), base * max(spec_w, 0.5), metal);
    var lin = vec3<f32>(0.0);
    let count = i32(u.info.x + 0.5);
    if ((flags & 1u) != 0u && count > 0) {
        let sh = i32(u.info.y + 0.5);
        for (var k = 0; k < count; k = k + 1) {
            let L = u.lights[k];
            let kind = i32(L.pos.w + 0.5);
            if (kind == 0) {
                lin = lin + dcol * L.col.rgb * ao;
                continue;
            }
            var l = -L.dir.xyz;
            var att = 1.0;
            if (kind >= 2) {
                let d = L.pos.xyz - i.wpos;
                l = normalize(d);
                if (kind == 3) {
                    let ca = dot(-l, normalize(L.dir.xyz));
                    att = smoothstep(L.dir.w, L.col.w, ca);
                }
            }
            if ((flags & 2u) != 0u && k == sh && u.info.y >= 0.0) {
                att = att * shadow_factor(i.wpos, n);
            }
            let ndl = max(dot(n, l), 0.0);
            lin = lin + (dcol * ndl + scol * spec_term(n, l, v, rough) * ndl) * L.col.rgb * att;
        }
        lin = lin + dcol * 0.03 * ao;
    } else {
        let l = normalize(u.light.xyz);
        let diff = max(dot(n, l), 0.0);
        let fill = max(dot(n, normalize(vec3<f32>(-0.3, 0.6, -0.4))), 0.0) * 0.15;
        lin = dcol * (0.12 * ao + 0.88 * diff + fill * ao) + scol * spec_term(n, l, v, rough) * diff * 0.6 + base * metal * 0.15;
    }
    // Cheap hemispherical environment reflection so metals and glossy surfaces read.
    let rv = reflect(-v, n);
    let env = mix(vec3<f32>(0.04, 0.04, 0.045), vec3<f32>(0.45, 0.47, 0.5), clamp(rv.y * 0.5 + 0.5, 0.0, 1.0));
    let fres = 0.04 + 0.96 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    let refl = mix(vec3<f32>(fres * spec_w), base, metal) * (1.0 - rough * 0.6);
    lin = lin + env * refl * ao;
    lin = lin + pow(i.emi.rgb, vec3<f32>(2.2)) * emis_tex;
    var a = i.col.a;
    if ((flags & 4u) != 0u) { a = min(a, u.info.z); }
    return vec4<f32>(pow(lin, vec3<f32>(1.0 / 2.2)), a);
}

@fragment fn fs_tri(i: VOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    // Sample in uniform control flow (implicit derivatives); layer 0 = UV checker.
    let flags = u32(u.cam.w + 0.5);
    var layer = max(i32(i.mat.w + 0.5) - 1, 0);
    if ((flags & 16u) != 0u) { layer = 0; }
    let texel = textureSample(tex_arr, tex_samp, i.uv, layer);
    var ms: MapSamples;
    ms.nrm = textureSample(tex_arr, tex_samp, i.uv, max(i32(i.maps.x + 0.5) - 1, 0));
    ms.rough = textureSample(tex_arr, tex_samp, i.uv, max(i32(i.maps.y + 0.5) - 1, 0));
    ms.metal = textureSample(tex_arr, tex_samp, i.uv, max(i32(i.maps.z + 0.5) - 1, 0));
    let packed = u32(i.maps.w + 0.5);
    ms.ao = textureSample(tex_arr, tex_samp, i.uv, max(i32(packed % 1024u) - 1, 0));
    ms.emis = textureSample(tex_arr, tex_samp, i.uv, max(i32(packed / 1024u) - 1, 0));
    ms.dp1 = dpdx(i.wpos);
    ms.dp2 = dpdy(i.wpos);
    ms.duv1 = dpdx(i.uv);
    ms.duv2 = dpdy(i.uv);
    return shade(i, ff, texel, ms);
}

@vertex fn vs_line(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.vp * vec4<f32>(v.pos, 1.0);
    o.clip.z = o.clip.z - u.light.w * o.clip.w;
    o.nrm = v.nrm;
    o.col = v.col;
    o.wpos = v.pos;
    o.mat = v.mat;
    o.emi = v.emi;
    o.uv = v.uv;
    o.maps = v.maps;
    return o;
}

@fragment fn fs_line(i: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(i.col.rgb, 1.0);
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct BOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_blit(@builtin(vertex_index) i: u32) -> BOut {
    var o: BOut;
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    o.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment fn fs_blit_gamma(i: BOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(tex, samp, i.uv).rgb, 1.0);
}

@fragment fn fs_blit_linear(i: BOut) -> @location(0) vec4<f32> {
    return vec4<f32>(pow(textureSample(tex, samp, i.uv).rgb, vec3<f32>(2.2)), 1.0);
}
"#;

fn material_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shadow: &wgpu::TextureView,
    cmp: &wgpu::Sampler,
    tex: &wgpu::TextureView,
    samp: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rm_material_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(shadow) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(cmp) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(tex) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(samp) },
        ],
    })
}

/// A 2D texture array (TEX_SIZE², full mips) holding `layers`; with no layers a 1×1 white
/// placeholder with 2 layers (GL needs >1 layer to treat it as an array).
fn make_texture_array(device: &wgpu::Device, layers: &[Arc<crate::texture::TexImage>]) -> wgpu::TextureView {
    let (size, mips, n) = if layers.is_empty() { (1, 1, 2) } else { (crate::texture::TEX_SIZE, layers[0].mips.len() as u32, (layers.len() as u32).max(2)) };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rm_textures"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: n },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
}

fn upload_texture_array(device: &wgpu::Device, queue: &wgpu::Queue, layers: &[Arc<crate::texture::TexImage>]) -> wgpu::TextureView {
    let size = crate::texture::TEX_SIZE;
    let n = (layers.len() as u32).max(2);
    let mips = layers.first().map(|l| l.mips.len() as u32).unwrap_or(1);
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rm_textures"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: n },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (li, img) in layers.iter().enumerate() {
        for (m, data) in img.mips.iter().enumerate() {
            let s = (size >> m).max(1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: m as u32, origin: wgpu::Origin3d { x: 0, y: 0, z: li as u32 }, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * s), rows_per_image: Some(s) },
                wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
            );
        }
    }
    tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
}

impl Renderer {
    pub fn install(rs: &egui_wgpu::RenderState) {
        let r = Renderer::new(&rs.device, &rs.adapter, rs.target_format);
        rs.renderer.write().callback_resources.insert(r);
    }

    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter, target_format: wgpu::TextureFormat) -> Self {
        let samples = {
            let f = adapter.get_texture_format_features(COLOR_FORMAT);
            let d = adapter.get_texture_format_features(DEPTH_FORMAT);
            if f.flags.sample_count_supported(4) && d.flags.sample_count_supported(4) { 4 } else { 1 }
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rm_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rm_uniform_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rm_blit_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rm_shadow_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rm_scene_layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&shadow_layout)],
            immediate_size: 0,
        });
        let shadow_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rm_shadow_pl"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });
        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rm_blit_pl"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let vlayout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GpuVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x2, 6 => Float32x4],
        };
        let make_scene_pipeline = |label: &str,
                                   vs: &str,
                                   fs: &str,
                                   topology: wgpu::PrimitiveTopology,
                                   bias: wgpu::DepthBiasState,
                                   on_top: bool,
                                   blended: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    buffers: &[Some(vlayout.clone())],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!on_top && !blended),
                    depth_compare: Some(if on_top { wgpu::CompareFunction::Always } else { wgpu::CompareFunction::LessEqual }),
                    stencil: Default::default(),
                    bias,
                }),
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: blended.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri_pipeline = make_scene_pipeline(
            "rm_tris",
            "vs_tri",
            "fs_tri",
            wgpu::PrimitiveTopology::TriangleList,
            wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 },
            false,
            false,
        );
        let tri_blend_pipeline = make_scene_pipeline(
            "rm_tris_blend",
            "vs_tri",
            "fs_tri",
            wgpu::PrimitiveTopology::TriangleList,
            wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 },
            false,
            true,
        );
        let line_pipeline = make_scene_pipeline(
            "rm_lines",
            "vs_line",
            "fs_line",
            wgpu::PrimitiveTopology::LineList,
            Default::default(),
            false,
            false,
        );
        let overlay_pipeline = make_scene_pipeline("rm_overlay", "vs_line", "fs_line", wgpu::PrimitiveTopology::LineList, Default::default(), true, false);
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rm_shadow"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_shadow"),
                buffers: &[Some(vlayout.clone())],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 4, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let shadow_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("rm_shadow_map"),
                size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let cmp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rm_shadow_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let tex_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rm_tex_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 1,
            ..Default::default()
        });
        let tex_view = make_texture_array(device, &[]);
        let shadow_bg = material_bind_group(device, &shadow_layout, &shadow_view, &cmp_sampler, &tex_view, &tex_sampler);
        let shadow_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rm_shadow_uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rm_shadow_uniform_bg"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: shadow_uniform_buf.as_entire_binding() }],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rm_blit"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_blit"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if target_format.is_srgb() { "fs_blit_linear" } else { "fs_blit_gamma" }),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rm_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Renderer {
            tri_pipeline,
            tri_blend_pipeline,
            shadow_pipeline,
            shadow_view,
            shadow_bg,
            shadow_layout,
            cmp_sampler,
            tex_sampler,
            tex_view,
            tex_key: 0,
            shadow_uniform_buf,
            shadow_uniform_bg,
            shadow_gen: 0,
            tris_transp: DynBuffer::new(),
            line_pipeline,
            overlay_pipeline,
            blit_pipeline,
            uniform_layout,
            blit_layout,
            sampler,
            samples,
            targets: HashMap::new(),
            uploaded_gen: 0,
            tris: DynBuffer::new(),
            wire_all: DynBuffer::new(),
            wire_sel: DynBuffer::new(),
            overlay: DynBuffer::new(),
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, id: usize, size: [u32; 2]) {
        if self.targets.get(&id).is_some_and(|t| t.size == size) {
            return;
        }
        let ext = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let tex = |label: &str, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: ext,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let resolve_view = tex(
            "rm_resolve",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let msaa_view = (self.samples > 1).then(|| {
            tex("rm_msaa", COLOR_FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT)
        });
        let depth_view = tex("rm_depth", DEPTH_FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (uniform_buf, uniform_bg, grid_buf) = match self.targets.remove(&id) {
            Some(old) => (old.uniform_buf, old.uniform_bg, old.grid_buf),
            None => {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rm_uniforms"),
                    size: std::mem::size_of::<Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("rm_uniform_bg"),
                    layout: &self.uniform_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
                });
                (buf, bg, None)
            }
        };
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rm_blit_bg"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolve_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.targets.insert(
            id,
            Target { size, msaa_view, resolve_view, depth_view, uniform_buf, uniform_bg, blit_bg, grid_buf },
        );
    }
}

impl egui_wgpu::CallbackTrait for ViewCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<Renderer>() else { return Vec::new() };
        let ppp = screen.pixels_per_point;
        let size = [
            ((self.rect.width() * ppp).round() as u32).max(1),
            ((self.rect.height() * ppp).round() as u32).max(1),
        ];
        if r.uploaded_gen != self.geom.generation {
            r.tris.upload_chunks(device, queue, &self.geom.tris);
            r.wire_all.upload_chunks(device, queue, &self.geom.wire_all);
            r.wire_sel.upload_chunks(device, queue, &self.geom.wire_sel);
            r.overlay.upload(device, queue, &self.geom.overlay);
            r.tris_transp.upload_chunks(device, queue, &self.geom.tris_transp);
            r.uploaded_gen = self.geom.generation;
        }
        // Texture layers (only when the set of textures changed).
        if let Some(ts) = &self.geom.textures
            && ts.key != r.tex_key
        {
            r.tex_key = ts.key;
            r.tex_view = upload_texture_array(device, queue, &ts.layers);
            r.shadow_bg = material_bind_group(device, &r.shadow_layout, &r.shadow_view, &r.cmp_sampler, &r.tex_view, &r.tex_sampler);
        }
        // Shadow map (once per geometry generation, shared by all views).
        if let Some((svp, _)) = self.geom.shadow
            && r.shadow_gen != self.geom.generation
        {
            r.shadow_gen = self.geom.generation;
            let su = Uniforms {
                view_proj: svp.to_cols_array_2d(),
                light: [0.0; 4],
                cam: [0.0; 4],
                shadow_vp: svp.to_cols_array_2d(),
                info: [0.0; 4],
                lights: [GpuLight::default(); MAX_LIGHTS],
            };
            queue.write_buffer(&r.shadow_uniform_buf, 0, bytemuck::bytes_of(&su));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rm_shadow_pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &r.shadow_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&r.shadow_pipeline);
            pass.set_bind_group(0, &r.shadow_uniform_bg, &[]);
            if r.tris.count > 0 {
                r.tris.draw(&mut pass);
            }
        }
        r.ensure_target(device, self.view_id, size);
        let line_offset = if self.view_proj.w_axis.w == 1.0 { 0.0002 } else { 0.00003 };
        let mut lights = [GpuLight::default(); MAX_LIGHTS];
        let nl = self.geom.lights.len().min(MAX_LIGHTS);
        lights[..nl].copy_from_slice(&self.geom.lights[..nl]);
        let shadows_on = self.shadows && self.all_lights && self.geom.shadow.is_some();
        let flags = (self.all_lights as u32) | ((shadows_on as u32) << 1) | ((self.xray as u32) << 2) | ((self.textured as u32) << 3) | ((self.uv_checker as u32) << 4);
        let uniforms = Uniforms {
            view_proj: self.view_proj.to_cols_array_2d(),
            light: [self.light_dir.x, self.light_dir.y, self.light_dir.z, line_offset],
            cam: [self.eye.x, self.eye.y, self.eye.z, flags as f32],
            shadow_vp: self.geom.shadow.map(|s| s.0).unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
            info: [nl as f32, self.geom.shadow.map(|s| s.1 as f32).unwrap_or(-1.0), 0.3, 1.0 / SHADOW_SIZE as f32],
            lights,
        };
        let t = r.targets.get_mut(&self.view_id).unwrap();
        queue.write_buffer(&t.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        // Grid buffer (small, per view).
        if !self.grid.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&self.grid);
            let need = bytes.len() as u64;
            if t.grid_buf.as_ref().is_none_or(|(b, _)| b.size() < need) {
                t.grid_buf = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("rm_grid"),
                        size: need.next_power_of_two(),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    0,
                ));
            }
            let gb = t.grid_buf.as_mut().unwrap();
            queue.write_buffer(&gb.0, 0, bytes);
            gb.1 = self.grid.len() as u32;
        } else if let Some(gb) = t.grid_buf.as_mut() {
            gb.1 = 0;
        }

        let (view, resolve) = match &t.msaa_view {
            Some(m) => (m, Some(&t.resolve_view)),
            None => (&t.resolve_view, None),
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rm_viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.bg[0] as f64,
                            g: self.bg[1] as f64,
                            b: self.bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &t.uniform_bg, &[]);
            pass.set_bind_group(1, &r.shadow_bg, &[]);
            pass.set_pipeline(&r.line_pipeline);
            if let Some((b, n)) = &t.grid_buf
                && *n > 0 {
                    pass.set_vertex_buffer(0, b.slice(..));
                    pass.draw(0..*n, 0..1);
                }
            if self.draw_shaded {
                let opaque = if self.xray { &r.tri_blend_pipeline } else { &r.tri_pipeline };
                if r.tris.count > 0 {
                    pass.set_pipeline(opaque);
                    r.tris.draw(&mut pass);
                }
                if r.tris_transp.count > 0 {
                    pass.set_pipeline(&r.tri_blend_pipeline);
                    r.tris_transp.draw(&mut pass);
                }
            }
            pass.set_pipeline(&r.line_pipeline);
            let wire = if self.draw_wire_all { &r.wire_all } else { &r.wire_sel };
            if wire.count > 0 {
                wire.draw(&mut pass);
            }
            if r.overlay.count > 0 {
                pass.set_pipeline(&r.overlay_pipeline);
                r.overlay.draw(&mut pass);
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(r) = resources.get::<Renderer>() else { return };
        let Some(t) = r.targets.get(&self.view_id) else { return };
        render_pass.set_pipeline(&r.blit_pipeline);
        render_pass.set_bind_group(0, &t.blit_bg, &[]);
        render_pass.draw(0..3, 0..1);
    }
}
