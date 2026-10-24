//! Viewport cameras: perspective + the three orthographic views (top/front/side).

use glam::{Mat4, Vec2, Vec3, Vec4};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamKind {
    Persp,
    Top,
    Front,
    Side,
}

impl CamKind {
    pub fn name(self) -> &'static str {
        match self {
            CamKind::Persp => "persp",
            CamKind::Top => "top",
            CamKind::Front => "front",
            CamKind::Side => "side",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub kind: CamKind,
    pub target: Vec3,
    /// Degrees. Only used by the perspective camera.
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// Half height of the orthographic view volume.
    pub ortho_half: f32,
    pub fov_y: f32,
}

impl Camera {
    pub fn new(kind: CamKind) -> Self {
        Camera { kind, target: Vec3::ZERO, yaw: 45.0, pitch: -27.0, distance: 38.0, ortho_half: 12.0, fov_y: 37.8 }
    }

    pub fn is_ortho(&self) -> bool {
        self.kind != CamKind::Persp
    }

    /// Unit vector pointing from the target toward the eye.
    pub fn back_dir(&self) -> Vec3 {
        match self.kind {
            CamKind::Persp => {
                let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
                Vec3::new(y.sin() * p.cos(), -p.sin(), y.cos() * p.cos())
            }
            CamKind::Top => Vec3::Y,
            CamKind::Front => Vec3::Z,
            CamKind::Side => Vec3::X,
        }
    }

    pub fn up_hint(&self) -> Vec3 {
        match self.kind {
            CamKind::Top => -Vec3::Z,
            _ => Vec3::Y,
        }
    }

    pub fn eye(&self) -> Vec3 {
        let d = if self.is_ortho() { 1000.0 } else { self.distance };
        self.target + self.back_dir() * d
    }

    pub fn forward(&self) -> Vec3 {
        -self.back_dir()
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(self.up_hint()).normalize()
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize()
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, self.up_hint())
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        if self.is_ortho() {
            let h = self.ortho_half;
            let w = h * aspect;
            glam::camera::rh::proj::directx::orthographic(-w, w, -h, h, 0.1, 5000.0)
        } else {
            let near = (self.distance * 0.002).clamp(0.005, 1.0);
            glam::camera::rh::proj::directx::perspective(self.fov_y.to_radians(), aspect, near, near * 200_000.0)
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    /// World units per screen pixel at the given world point.
    pub fn world_per_pixel(&self, p: Vec3, viewport_h: f32) -> f32 {
        if self.is_ortho() {
            2.0 * self.ortho_half / viewport_h
        } else {
            let depth = (p - self.eye()).dot(self.forward()).max(0.01);
            2.0 * depth * (self.fov_y.to_radians() / 2.0).tan() / viewport_h
        }
    }

    pub fn tumble(&mut self, dx: f32, dy: f32) {
        if self.is_ortho() {
            return;
        }
        self.yaw -= dx * 0.35;
        self.pitch = (self.pitch - dy * 0.35).clamp(-89.5, 89.5);
    }

    pub fn track(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let wpp = self.world_per_pixel(self.target, viewport_h);
        self.target += (-self.right() * dx + self.up() * dy) * wpp;
    }

    pub fn dolly(&mut self, amount: f32) {
        let f = (-amount * 0.005).exp();
        if self.is_ortho() {
            self.ortho_half = (self.ortho_half * f).clamp(0.01, 10_000.0);
        } else {
            self.distance = (self.distance * f).clamp(0.01, 100_000.0);
        }
    }

    pub fn frame(&mut self, lo: Vec3, hi: Vec3) {
        let c = (lo + hi) * 0.5;
        let r = ((hi - lo).length() * 0.5).max(0.05);
        self.target = c;
        if self.is_ortho() {
            self.ortho_half = r * 1.6;
        } else {
            self.distance = r / (self.fov_y.to_radians() / 2.0).sin() * 1.6;
        }
    }
}

/// Screen-space helper bound to one viewport rectangle (in egui points).
#[derive(Clone, Copy, Debug)]
pub struct Projector {
    pub view_proj: Mat4,
    pub inv: Mat4,
    pub origin: Vec2,
    pub size: Vec2,
}

impl Projector {
    pub fn new(cam: &Camera, origin: Vec2, size: Vec2) -> Self {
        let vp = cam.view_proj(size.x / size.y.max(1.0));
        Projector { view_proj: vp, inv: vp.inverse(), origin, size }
    }

    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let c = self.view_proj * Vec4::new(p.x, p.y, p.z, 1.0);
        if c.w <= 1e-6 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        Some(Vec2::new(
            self.origin.x + (ndc.x * 0.5 + 0.5) * self.size.x,
            self.origin.y + (0.5 - ndc.y * 0.5) * self.size.y,
        ))
    }

    /// World-space ray (origin, direction) through a screen point.
    pub fn ray(&self, s: Vec2) -> (Vec3, Vec3) {
        let nx = (s.x - self.origin.x) / self.size.x * 2.0 - 1.0;
        let ny = 1.0 - (s.y - self.origin.y) / self.size.y * 2.0;
        let a = self.inv * Vec4::new(nx, ny, 0.0, 1.0);
        let b = self.inv * Vec4::new(nx, ny, 1.0, 1.0);
        let a = a.truncate() / a.w;
        let b = b.truncate() / b.w;
        (a, (b - a).normalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_ray_roundtrip() {
        let cam = Camera::new(CamKind::Persp);
        let pr = Projector::new(&cam, Vec2::new(10.0, 20.0), Vec2::new(800.0, 600.0));
        let s = pr.project(Vec3::new(1.0, 2.0, 3.0)).unwrap();
        let (o, d) = pr.ray(s);
        // the point should lie on the ray
        let v = Vec3::new(1.0, 2.0, 3.0) - o;
        assert!(v.cross(d).length() < 1e-2 * v.length());
        let center = pr.project(cam.target).unwrap();
        assert!((center - Vec2::new(410.0, 320.0)).length() < 1e-2);
    }
}
