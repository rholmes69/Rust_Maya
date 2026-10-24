//! Look development: shared materials (Hypershade), lights and cameras as scene nodes.

use crate::app::App;
use crate::scene::{NodeId, Shape};
use glam::{Mat3, Quat, Vec3};
use serde::{Deserialize, Serialize};

// --------------------------------------------------------------------------- materials

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatKind {
    Lambert,
    Blinn,
    Phong,
    StandardSurface,
}

impl MatKind {
    pub const ALL: [MatKind; 4] = [MatKind::Lambert, MatKind::Blinn, MatKind::Phong, MatKind::StandardSurface];
    pub fn name(self) -> &'static str {
        match self {
            MatKind::Lambert => "lambert",
            MatKind::Blinn => "blinn",
            MatKind::Phong => "phong",
            MatKind::StandardSurface => "standardSurface",
        }
    }
    pub fn parse(s: &str) -> Option<MatKind> {
        match s.to_ascii_lowercase().as_str() {
            "lambert" => Some(MatKind::Lambert),
            "blinn" => Some(MatKind::Blinn),
            "phong" | "phonge" => Some(MatKind::Phong),
            "standardsurface" | "aistandardsurface" | "standard" | "pbr" => Some(MatKind::StandardSurface),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub id: u32,
    pub name: String,
    pub kind: MatKind,
    /// Base colour (sRGB).
    pub color: [f32; 3],
    /// Specular weight 0..1.
    pub specular: f32,
    /// Roughness 0..1 (Blinn eccentricity / Phong cosine power are mapped onto it).
    pub roughness: f32,
    pub metalness: f32,
    /// Incandescence / emission colour (sRGB, added on top of lighting).
    pub emission: [f32; 3],
    /// 0 = opaque, 1 = fully transparent.
    pub transparency: f32,
    /// File texture connected to the colour ("checker" = built-in UV checker).
    pub color_map: Option<String>,
    /// place2dTexture repeatUV.
    pub repeat_uv: [f32; 2],
}

impl Default for Material {
    fn default() -> Self {
        Material { id: 0, name: "lambert2".into(), kind: MatKind::Lambert, color: [0.5, 0.5, 0.5], specular: 0.0, roughness: 0.5, metalness: 0.0, emission: [0.0; 3], transparency: 0.0, color_map: None, repeat_uv: [1.0, 1.0] }
    }
}

impl Material {
    pub fn new(kind: MatKind) -> Self {
        let (specular, roughness) = match kind {
            MatKind::Lambert => (0.0, 0.8),
            MatKind::Blinn => (0.5, 0.3),
            MatKind::Phong => (0.6, 0.15),
            MatKind::StandardSurface => (1.0, 0.2),
        };
        Material { kind, specular, roughness, color: if kind == MatKind::StandardSurface { [0.8, 0.8, 0.8] } else { [0.5, 0.5, 0.5] }, ..Default::default() }
    }

    /// Set an attribute by Maya-style name. Returns false for unknown attributes.
    pub fn set_attr(&mut self, attr: &str, v: &[f32]) -> Result<(), String> {
        let rgb = |v: &[f32]| -> Result<[f32; 3], String> {
            match v {
                [r, g, b, ..] => Ok([*r, *g, *b]),
                [x] => Ok([*x; 3]),
                _ => Err("expected r g b".into()),
            }
        };
        let one = |v: &[f32]| v.first().copied().ok_or_else(|| "missing value".to_string());
        match attr {
            "color" | "baseColor" | "c" | "diffuseColor" => self.color = rgb(v)?,
            "colorR" => self.color[0] = one(v)?,
            "colorG" => self.color[1] = one(v)?,
            "colorB" => self.color[2] = one(v)?,
            "specular" | "specularWeight" | "reflectivity" | "specularRollOff" => self.specular = one(v)?.clamp(0.0, 1.0),
            "specularColor" => self.specular = (rgb(v)?.iter().sum::<f32>() / 3.0).clamp(0.0, 1.0),
            "roughness" | "specularRoughness" => self.roughness = one(v)?.clamp(0.0, 1.0),
            "eccentricity" => self.roughness = one(v)?.clamp(0.0, 1.0),
            "cosinePower" => self.roughness = (1.0 - (one(v)? / 100.0)).clamp(0.02, 1.0),
            "metalness" | "metallic" => self.metalness = one(v)?.clamp(0.0, 1.0),
            "repeatUV" | "repeat" => {
                self.repeat_uv = match v {
                    [u, w, ..] => [*u, *w],
                    [x] => [*x, *x],
                    _ => return Err("expected u v".into()),
                }
            }
            "repeatU" => self.repeat_uv[0] = one(v)?,
            "repeatV" => self.repeat_uv[1] = one(v)?,
            "incandescence" | "emission" | "emissionColor" => self.emission = rgb(v)?,
            "transparency" | "opacity" => {
                let t = match v {
                    [r, g, b, ..] => (r + g + b) / 3.0,
                    [x] => *x,
                    _ => return Err("missing value".into()),
                };
                self.transparency = if attr == "opacity" { 1.0 - t } else { t }.clamp(0.0, 1.0);
            }
            _ => return Err(format!("Unknown material attribute '{attr}' (color, colorMap, repeatUV, specular, roughness, metalness, incandescence, transparency)")),
        }
        Ok(())
    }

    pub fn get_attr(&self, attr: &str) -> Option<String> {
        let c = |c: [f32; 3]| format!("{} {} {}", c[0], c[1], c[2]);
        Some(match attr {
            "color" | "baseColor" | "c" => c(self.color),
            "specular" | "specularWeight" => self.specular.to_string(),
            "roughness" | "specularRoughness" | "eccentricity" => self.roughness.to_string(),
            "metalness" => self.metalness.to_string(),
            "incandescence" | "emission" => c(self.emission),
            "transparency" => self.transparency.to_string(),
            "colorMap" | "fileTextureName" => self.color_map.clone().unwrap_or_default(),
            "repeatUV" => format!("{} {}", self.repeat_uv[0], self.repeat_uv[1]),
            _ => return None,
        })
    }
}

// ------------------------------------------------------------------------------ lights

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKind {
    Ambient,
    Directional,
    Point,
    Spot,
}

impl LightKind {
    pub fn name(self) -> &'static str {
        match self {
            LightKind::Ambient => "ambientLight",
            LightKind::Directional => "directionalLight",
            LightKind::Point => "pointLight",
            LightKind::Spot => "spotLight",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    pub kind: LightKind,
    pub color: [f32; 3],
    pub intensity: f32,
    /// Spot cone angle (degrees, full angle).
    pub cone: f32,
    /// Spot penumbra (degrees, added outside the cone).
    pub penumbra: f32,
    /// Depth-map shadows.
    pub shadows: bool,
}

impl Default for Light {
    fn default() -> Self {
        Light { kind: LightKind::Directional, color: [1.0; 3], intensity: 1.0, cone: 40.0, penumbra: 5.0, shadows: false }
    }
}

impl Light {
    pub fn set_attr(&mut self, attr: &str, v: &[f32]) -> Result<(), String> {
        let one = || v.first().copied().ok_or_else(|| "missing value".to_string());
        match attr {
            "intensity" | "i" => self.intensity = one()?.max(0.0),
            "color" | "rgb" => {
                self.color = match v {
                    [r, g, b, ..] => [*r, *g, *b],
                    [x] => [*x; 3],
                    _ => return Err("expected r g b".into()),
                }
            }
            "coneAngle" | "cone" => self.cone = one()?.clamp(1.0, 179.0),
            "penumbraAngle" | "penumbra" => self.penumbra = one()?.clamp(0.0, 45.0),
            "useDepthMapShadows" | "shadows" | "castShadows" | "useRayTraceShadows" => self.shadows = one()? > 0.5,
            _ => return Err(format!("Unknown light attribute '{attr}' (intensity, color, coneAngle, penumbraAngle, useDepthMapShadows)")),
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------------- cameras

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraShape {
    /// Focal length in mm (35 mm film back 36 x 24 mm, like Maya's default camera).
    pub focal: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for CameraShape {
    fn default() -> Self {
        CameraShape { focal: 35.0, near: 0.1, far: 10_000.0 }
    }
}

impl CameraShape {
    /// Vertical field of view in degrees.
    pub fn fov_y(&self) -> f32 {
        2.0 * (12.0 / self.focal.max(1.0)).atan().to_degrees()
    }
    pub fn set_attr(&mut self, attr: &str, v: &[f32]) -> Result<(), String> {
        let x = v.first().copied().ok_or("missing value")?;
        match attr {
            "focalLength" | "fl" => self.focal = x.clamp(2.5, 3500.0),
            "nearClipPlane" | "ncp" => self.near = x.max(1e-4),
            "farClipPlane" | "fcp" => self.far = x.max(self.near + 1e-3),
            "fieldOfView" | "fov" => self.focal = 12.0 / (x.to_radians() / 2.0).tan(),
            _ => return Err(format!("Unknown camera attribute '{attr}' (focalLength, nearClipPlane, farClipPlane)")),
        }
        Ok(())
    }
}

/// Rotation (Euler XYZ degrees) that points local -Z along `dir` with +Y up.
pub fn aim_euler(dir: Vec3) -> Vec3 {
    let f = dir.normalize_or(Vec3::NEG_Z);
    let up = if f.cross(Vec3::Y).length() < 1e-4 { Vec3::Z } else { Vec3::Y };
    let z = -f;
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    crate::rig::quat_euler(Quat::from_mat3(&Mat3::from_cols(x, y, z)))
}

/// Line-segment icon for a light / camera in local space.
pub fn icon_lines(shape: &Shape) -> Vec<(Vec3, Vec3)> {
    let mut l = Vec::new();
    let circle = |l: &mut Vec<(Vec3, Vec3)>, r: f32, z: f32, n: usize| {
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            l.push((Vec3::new(a0.cos() * r, a0.sin() * r, z), Vec3::new(a1.cos() * r, a1.sin() * r, z)));
        }
    };
    match shape {
        Shape::Light(li) => match li.kind {
            LightKind::Directional => {
                // Three parallel arrows along -Z.
                for x in [-0.4f32, 0.0, 0.4] {
                    let a = Vec3::new(x, 0.0, 0.5);
                    let b = Vec3::new(x, 0.0, -0.9);
                    l.push((a, b));
                    l.push((b, b + Vec3::new(0.12, 0.0, 0.25)));
                    l.push((b, b + Vec3::new(-0.12, 0.0, 0.25)));
                }
            }
            LightKind::Point => {
                for d in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 0.0).normalize(), Vec3::new(1.0, -1.0, 0.0).normalize(), Vec3::new(0.0, 1.0, 1.0).normalize(), Vec3::new(0.0, 1.0, -1.0).normalize()] {
                    l.push((-d * 0.45, d * 0.45));
                }
                circle(&mut l, 0.18, 0.0, 10);
            }
            LightKind::Spot => {
                let half = (li.cone * 0.5).to_radians();
                let len = 1.2;
                let r = len * half.tan();
                circle(&mut l, r, -len, 16);
                for i in 0..4 {
                    let a = i as f32 * std::f32::consts::FRAC_PI_2;
                    l.push((Vec3::ZERO, Vec3::new(a.cos() * r, a.sin() * r, -len)));
                }
                l.push((Vec3::ZERO, Vec3::new(0.0, 0.0, -len * 1.5)));
            }
            LightKind::Ambient => {
                circle(&mut l, 0.4, 0.0, 16);
                l.push((Vec3::new(-0.4, 0.0, 0.0), Vec3::new(0.4, 0.0, 0.0)));
                l.push((Vec3::new(0.0, -0.4, 0.0), Vec3::new(0.0, 0.4, 0.0)));
            }
        },
        Shape::Camera(_) => {
            // Body box + lens cone looking down -Z.
            let (w, h, d) = (0.35, 0.3, 0.5);
            let c = [
                Vec3::new(-w, -h, -d), Vec3::new(w, -h, -d), Vec3::new(w, h, -d), Vec3::new(-w, h, -d),
                Vec3::new(-w, -h, d), Vec3::new(w, -h, d), Vec3::new(w, h, d), Vec3::new(-w, h, d),
            ];
            for i in 0..4 {
                l.push((c[i], c[(i + 1) % 4]));
                l.push((c[i + 4], c[(i + 1) % 4 + 4]));
                l.push((c[i], c[i + 4]));
            }
            let lz = -d - 0.4;
            let lens = [Vec3::new(-0.25, -0.2, lz), Vec3::new(0.25, -0.2, lz), Vec3::new(0.25, 0.2, lz), Vec3::new(-0.25, 0.2, lz)];
            for i in 0..4 {
                l.push((lens[i], lens[(i + 1) % 4]));
                l.push((lens[i], Vec3::new(lens[i].x * 0.4, lens[i].y * 0.4, -d)));
            }
            // "Up" triangle on top, like Maya's camera icon.
            l.push((Vec3::new(-0.15, h, 0.0), Vec3::new(0.0, h + 0.2, 0.0)));
            l.push((Vec3::new(0.15, h, 0.0), Vec3::new(0.0, h + 0.2, 0.0)));
        }
        _ => {}
    }
    l
}

fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    [c[0].max(0.0).powf(2.2), c[1].max(0.0).powf(2.2), c[2].max(0.0).powf(2.2)]
}

impl App {
    /// Gather visible scene lights (and the shadow-casting one) for the renderer.
    pub fn collect_lights(&self, g: &mut crate::render::FrameGeometry) {
        use crate::render::{GpuLight, MAX_LIGHTS};
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (a, b) in g.tris.iter().chain(&g.tris_transp).filter_map(|c| c.bounds) {
            lo = lo.min(a);
            hi = hi.max(b);
        }
        let have_bounds = lo.x <= hi.x;
        for n in &self.scene.nodes {
            let Shape::Light(li) = &n.shape else { continue };
            if !self.scene.is_visible(n.id) || g.lights.len() >= MAX_LIGHTS {
                continue;
            }
            let m = self.scene.world_matrix(n.id);
            let pos = m.transform_point3(Vec3::ZERO);
            let dir = m.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Y);
            let lc = srgb_to_linear(li.color);
            let half = (li.cone * 0.5).to_radians();
            let outer = (half + li.penumbra.to_radians()).min(1.55);
            let kind = match li.kind {
                LightKind::Ambient => 0.0,
                LightKind::Directional => 1.0,
                LightKind::Point => 2.0,
                LightKind::Spot => 3.0,
            };
            let idx = g.lights.len();
            g.lights.push(GpuLight {
                pos: [pos.x, pos.y, pos.z, kind],
                dir: [dir.x, dir.y, dir.z, outer.cos()],
                col: [lc[0] * li.intensity, lc[1] * li.intensity, lc[2] * li.intensity, half.cos()],
            });
            if li.shadows && g.shadow.is_none() && have_bounds {
                let center = (lo + hi) * 0.5;
                let radius = ((hi - lo).length() * 0.5).max(0.5);
                let up = if dir.cross(Vec3::Y).length() < 1e-3 { Vec3::Z } else { Vec3::Y };
                match li.kind {
                    LightKind::Directional => {
                        let eye = center - dir * radius * 2.0;
                        let view = glam::camera::rh::view::look_at_mat4(eye, center, up);
                        let proj = glam::camera::rh::proj::directx::orthographic(-radius, radius, -radius, radius, 0.01, radius * 4.0);
                        g.shadow = Some((proj * view, idx));
                    }
                    LightKind::Spot => {
                        let far = (pos - center).length() + radius * 1.5;
                        let view = glam::camera::rh::view::look_at_mat4(pos, pos + dir, up);
                        let proj = glam::camera::rh::proj::directx::perspective((outer * 2.0).min(3.0), 1.0, (far * 0.001).max(0.02), far);
                        g.shadow = Some((proj * view, idx));
                    }
                    _ => {}
                }
            }
        }
    }

    // ---------------------------------------------------------------- materials

    pub fn material(&self, id: u32) -> Option<&crate::look::Material> {
        self.scene.materials.iter().find(|m| m.id == id)
    }
    pub fn material_mut(&mut self, id: u32) -> Option<&mut crate::look::Material> {
        self.scene.materials.iter_mut().find(|m| m.id == id)
    }
    pub fn material_by_name(&self, name: &str) -> Option<u32> {
        let n = name.strip_suffix("SG").filter(|b| self.scene.materials.iter().any(|m| m.name == *b)).unwrap_or(name);
        self.scene.materials.iter().find(|m| m.name == n).map(|m| m.id)
    }

    /// Create a material (undoable). Names are kept unique among materials and nodes.
    pub fn create_material(&mut self, kind: MatKind, name: Option<&str>) -> u32 {
        self.checkpoint();
        let id = self.scene.materials.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        let base = name.map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| format!("{}1", kind.name()));
        let mut nm = base.clone();
        let stem = base.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        let mut i = 1;
        while self.scene.materials.iter().any(|m| m.name == nm) || self.scene.name_exists(&nm) {
            i += 1;
            nm = format!("{stem}{i}");
        }
        let mut m = Material::new(kind);
        m.id = id;
        m.name = nm;
        self.scene.materials.push(m);
        id
    }

    /// Assign a material to objects (None = back to the default per-object lambert).
    pub fn assign_material(&mut self, mat: Option<u32>, ids: &[NodeId]) -> usize {
        self.checkpoint();
        let mut n = 0;
        for &id in ids {
            let mut stack = vec![id];
            // Assigning to a group assigns to all meshes below it, like Maya.
            while let Some(x) = stack.pop() {
                stack.extend(self.scene.children(Some(x)));
                if let Some(node) = self.scene.node_mut(x)
                    && node.mesh().is_some()
                {
                    node.material = mat;
                    n += 1;
                }
            }
        }
        n
    }

    pub fn delete_material(&mut self, id: u32) {
        self.checkpoint();
        self.scene.materials.retain(|m| m.id != id);
        for n in &mut self.scene.nodes {
            if n.material == Some(id) {
                n.material = None;
            }
        }
    }

    pub fn duplicate_material(&mut self, id: u32) -> Option<u32> {
        let src = self.material(id)?.clone();
        let nid = self.create_material(src.kind, Some(&src.name));
        let keep_name = self.material(nid)?.name.clone();
        let m = self.material_mut(nid)?;
        *m = Material { id: nid, name: keep_name, ..src };
        Some(nid)
    }

    pub fn rename_material(&mut self, id: u32, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.scene.materials.iter().any(|m| m.name == name && m.id != id) {
            return;
        }
        self.checkpoint();
        if let Some(m) = self.material_mut(id) {
            m.name = name.to_string();
        }
    }

    /// Objects using a material.
    pub fn material_users(&self, id: u32) -> Vec<NodeId> {
        self.scene.nodes.iter().filter(|n| n.material == Some(id)).map(|n| n.id).collect()
    }

    /// Effective surface colour of a node (material colour if assigned).
    pub fn surface_color(&self, id: NodeId) -> [f32; 3] {
        let Some(n) = self.scene.node(id) else { return [0.5; 3] };
        n.material.and_then(|m| self.material(m)).map(|m| m.color).unwrap_or(n.color)
    }

    // ------------------------------------------------------------- lights/cams

    pub fn create_light(&mut self, kind: LightKind, name: Option<&str>) -> NodeId {
        self.checkpoint();
        let id = self.scene.add_node(kind.name(), Shape::Light(Light { kind, ..Default::default() }), None);
        self.apply_name(id, name);
        if let Some(n) = self.scene.node_mut(id) {
            match kind {
                // Maya creates lights at the origin; lift point/spot lights so they are useful.
                LightKind::Directional => n.rotate = Vec3::new(-45.0, 30.0, 0.0),
                LightKind::Point => n.translate = Vec3::new(0.0, 5.0, 0.0),
                LightKind::Spot => {
                    n.translate = Vec3::new(0.0, 8.0, 6.0);
                    n.rotate = aim_euler(-n.translate);
                }
                LightKind::Ambient => n.translate = Vec3::new(0.0, 3.0, 0.0),
            }
        }
        self.sel = vec![id];
        id
    }

    pub fn create_camera(&mut self, name: Option<&str>, focal: f32) -> NodeId {
        self.checkpoint();
        let id = self.scene.add_node("camera1", Shape::Camera(CameraShape { focal, ..Default::default() }), None);
        self.apply_name(id, name);
        if let Some(n) = self.scene.node_mut(id) {
            n.translate = Vec3::new(0.0, 2.0, 12.0);
            n.rotate = aim_euler(Vec3::new(0.0, -2.0, -12.0));
        }
        self.sel = vec![id];
        id
    }

    /// Aim a node's -Z axis at a world point (lights / cameras).
    pub fn aim_at(&mut self, id: NodeId, target: Vec3) {
        let p = crate::rig::world_pos(&self.scene, id);
        let e = aim_euler(target - p);
        let q = crate::rig::euler_quat(e);
        crate::rig::set_world_rotation(&mut self.scene, id, q);
    }

    pub fn is_camera(&self, id: NodeId) -> bool {
        matches!(self.scene.node(id).map(|n| &n.shape), Some(Shape::Camera(_)))
    }

    /// Look through a camera node in a view (None = back to the view's own camera).
    pub fn look_through(&mut self, view: usize, cam: Option<NodeId>) -> Result<(), String> {
        if let Some(c) = cam
            && !self.is_camera(c)
        {
            return Err("Look Through: select a camera.".into());
        }
        let v = &mut self.views[view];
        v.look_through = cam;
        v.lt_pose = None;
        if cam.is_none() {
            v.cam.fov_y = 37.8;
        }
        self.sync_look_through();
        Ok(())
    }

    /// Keep views that look through camera nodes in sync with them: if the camera node was
    /// moved (gizmo, animation, MEL) the view follows; if the user navigated the view the
    /// camera node follows.
    pub fn sync_look_through(&mut self) {
        for vi in 0..4 {
            let Some(cid) = self.views[vi].look_through else { continue };
            let Some(Shape::Camera(cs)) = self.scene.node(cid).map(|n| n.shape.clone()) else {
                self.views[vi].look_through = None;
                self.views[vi].lt_pose = None;
                continue;
            };
            let m = self.scene.world_matrix(cid);
            let (_, rot, pos) = m.to_scale_rotation_translation();
            let pose = (pos, rot);
            let v = &mut self.views[vi];
            v.cam.fov_y = cs.fov_y();
            let changed_externally = match v.lt_pose {
                None => true,
                Some((p, r)) => p.distance(pos) > 1e-4 || r.angle_between(rot) > 1e-4,
            };
            if changed_externally {
                // View follows the node.
                v.cam.kind = crate::camera::CamKind::Persp;
                let fwd = rot * Vec3::NEG_Z;
                let back = -fwd;
                v.cam.pitch = (-back.y).clamp(-1.0, 1.0).asin().to_degrees();
                v.cam.yaw = back.x.atan2(back.z).to_degrees();
                let d = v.cam.distance.clamp(0.5, 1000.0);
                v.cam.distance = d;
                v.cam.target = pos + fwd * d;
                v.lt_pose = Some(pose);
            } else {
                // Node follows the view (after tumble / track / dolly).
                let eye = v.cam.eye();
                let fwd = v.cam.forward();
                let (lp, lr) = v.lt_pose.unwrap();
                let new_rot = crate::rig::euler_quat(aim_euler(fwd));
                if lp.distance(eye) > 1e-4 || lr.angle_between(new_rot) > 1e-3 {
                    crate::rig::set_world_position(&mut self.scene, cid, eye);
                    crate::rig::set_world_rotation(&mut self.scene, cid, new_rot);
                    let (_, r2, p2) = self.scene.world_matrix(cid).to_scale_rotation_translation();
                    self.views[vi].lt_pose = Some((p2, r2));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn aim_points_minus_z() {
        let d = Vec3::new(1.0, -1.0, -2.0).normalize();
        let q = crate::rig::euler_quat(aim_euler(d));
        assert!((q * Vec3::NEG_Z - d).length() < 1e-4);
    }

    #[test]
    fn mel_materials_lights_cameras() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -n box; polySphere -n ball").unwrap();
        let m = execute(&mut app, "shadingNode -asShader blinn -n red_mat").unwrap();
        assert_eq!(m, "red_mat");
        execute(&mut app, "setAttr red_mat.color -type double3 1 0 0; setAttr red_mat.roughness 0.2; hyperShade -assign red_mat box ball").unwrap();
        let box_ = app.scene.find_by_name("box").unwrap();
        assert_eq!(app.surface_color(box_), [1.0, 0.0, 0.0]);
        assert_eq!(execute(&mut app, "getAttr red_mat.roughness").unwrap(), "0.2");
        // Shared: editing the material recolours both objects.
        execute(&mut app, "setAttr red_mat.color 0 0 1").unwrap();
        let ball = app.scene.find_by_name("ball").unwrap();
        assert_eq!(app.surface_color(ball), [0.0, 0.0, 1.0]);
        execute(&mut app, "sets -e -forceElement red_matSG box").unwrap();
        // setColor goes back to a per-object colour.
        execute(&mut app, "setColor green ball").unwrap();
        assert!(app.scene.node(ball).unwrap().material.is_none());
        execute(&mut app, "undo").unwrap();
        assert!(app.scene.node(ball).unwrap().material.is_some());

        // Lights
        let l = execute(&mut app, "directionalLight -i 2 -rgb 1 0.9 0.8 -n sun -sh on").unwrap();
        assert_eq!(l, "sun");
        let sun = app.scene.find_by_name("sun").unwrap();
        match &app.scene.node(sun).unwrap().shape {
            Shape::Light(li) => assert!(li.shadows && li.intensity == 2.0 && li.kind == LightKind::Directional),
            _ => panic!(),
        }
        execute(&mut app, "spotLight -ca 30 -n key -p 0 6 4 -aim 0 0 0; setAttr key.intensity 3; pointLight -n bulb; ambientLight -i 0.2").unwrap();
        let key = app.scene.find_by_name("key").unwrap();
        let fwd = crate::rig::world_rotation(&app.scene, key) * Vec3::NEG_Z;
        assert!((fwd - Vec3::new(0.0, -6.0, -4.0).normalize()).length() < 1e-3);

        // Camera + look through
        execute(&mut app, "camera -n shotCam -fl 50 -p 0 3 10 -aim 0 0 0; lookThru shotCam").unwrap();
        let cam = app.scene.find_by_name("shotCam").unwrap();
        assert_eq!(app.views[app.active_view].look_through, Some(cam));
        let v = &app.views[app.active_view];
        assert!((v.cam.eye() - Vec3::new(0.0, 3.0, 10.0)).length() < 1e-3, "{}", v.cam.eye());
        // Navigating the view moves the camera node.
        let av = app.active_view;
        app.views[av].cam.tumble(20.0, 0.0);
        app.sync_look_through();
        let p = crate::rig::world_pos(&app.scene, cam);
        assert!((p - app.views[av].cam.eye()).length() < 1e-3 && (p - Vec3::new(0.0, 3.0, 10.0)).length() > 0.1);
        // Moving the camera node moves the view.
        execute(&mut app, "move 5 5 5 shotCam").unwrap();
        app.sync_look_through();
        assert!((app.views[av].cam.eye() - Vec3::new(5.0, 5.0, 5.0)).length() < 1e-3);
        execute(&mut app, "lookThru persp").unwrap();
        assert!(app.views[av].look_through.is_none());
        // Saved scenes keep materials, lights and cameras.
        let json = serde_json::to_string(&app.scene).unwrap();
        let back: crate::scene::Scene = serde_json::from_str(&json).unwrap();
        assert_eq!(back.materials.len(), 1);
        assert!(back.nodes.iter().any(|n| matches!(n.shape, Shape::Camera(_))));
    }
}

#[cfg(test)]
mod shadow_tests {
    use crate::app::App;
    use crate::commands::execute;
    use glam::Vec3;

    #[test]
    fn shadow_matrix_projects_scene() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 14 -h 14 -n ground; polyCube -w 2 -h 2 -d 2 -n box; move -2 1 0; directionalLight -n sun -rot -55 35 0 -sh on").unwrap();
        let g = app.build_geometry();
        let (vp, idx) = g.shadow.expect("shadow light");
        assert_eq!(idx, 0);
        let dir = Vec3::from_slice(&g.lights[0].dir[..3]);
        // A point on top of the box and the ground point it shadows share the same uv; the box is nearer.
        let top = Vec3::new(-2.0, 2.0, 0.0);
        let t = -top.y / dir.y;
        let ground = top + dir * t;
        let a = vp.project_point3(top);
        let b = vp.project_point3(ground);
        eprintln!("dir {dir} top {a} ground {b}");
        assert!((a.truncate() - b.truncate()).length() < 1e-3);
        assert!(a.z < b.z && a.z > 0.0 && b.z < 1.0);
        assert!(a.x.abs() < 1.0 && a.y.abs() < 1.0);
    }
}
