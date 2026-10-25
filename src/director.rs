//! Shot director: procedural characters, themed environments, shot cameras, FX presets and
//! `makeShot`, which assembles a whole shot (set, character, animation, FX, lighting, camera)
//! from a few words — "a robot walking through a snowy forest at sunset, tracking shot".

use crate::app::App;
use crate::fx::{EmitterKind, ParticleSettings, Rng};
use crate::mesh::Mesh;
use crate::scene::{Channel, Curve, NodeId, Shape};
use glam::{Mat4, Quat, Vec3};

pub const STYLES: [(&str, &str); 3] = [("human", "mannequin-style person"), ("robot", "boxy robot"), ("alien", "big-headed thin alien")];
pub const THEMES: [(&str, &str); 7] = [
    ("forest", "grass, trees and rocks around a clear path"),
    ("city", "a street lined with buildings"),
    ("desert", "dunes, rocks and cacti"),
    ("beach", "sand, sea and palm trees"),
    ("studio", "a seamless white cyclorama"),
    ("snow", "snowfield with pine trees"),
    ("space", "a rocky moon with a planet and stars"),
];
pub const CAMERAS: [(&str, &str); 5] = [
    ("static", "wide establishing shot"),
    ("track", "follows the subject from the side"),
    ("dolly", "pushes in from the front"),
    ("orbit", "circles the subject"),
    ("crane", "rises up and back"),
];
pub const FX: [(&str, &str); 8] = [
    ("rain", "falling rain"),
    ("snow", "drifting snowflakes"),
    ("sparks", "a burst of sparks"),
    ("fireflies", "glowing fireflies"),
    ("dust", "blowing dust"),
    ("leaves", "falling leaves"),
    ("cape", "a cloth cape pinned to the character's chest"),
    ("flag", "a flag on a pole in the wind"),
];

// ------------------------------------------------------------------------------ character

fn xf(m: Mesh, t: Mat4) -> Mesh {
    Mesh { positions: m.positions.iter().map(|p| t.transform_point3(*p)).collect(), ..m }
}

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

/// A T-posed character mesh (feet on y = 0, facing +Z, arms along ±X) of height `h`, with
/// limbs where Quick Rig puts its joints, so it rigs and deforms cleanly.
pub fn character_mesh(style: &str, h: f32) -> Result<Mesh, String> {
    use crate::mesh::{cube, cylinder, sphere};
    let mut m = Mesh::default();
    // Along +X: the cylinder's top (+Y, radius r1) turns toward the hand.
    let arm = |r0: f32, r1: f32| xf(cylinder(r0 * h, r1 * h, 0.29 * h, 12, 10, true), Mat4::from_rotation_z(-std::f32::consts::FRAC_PI_2));
    match style {
        "human" | "person" | "man" | "woman" => {
            for s in [1.0f32, -1.0] {
                m.append(&xf(cylinder(0.042 * h, 0.06 * h, 0.46 * h, 12, 10, true), at(s * 0.09 * h, 0.28 * h, 0.0)));
                m.append(&xf(cube(0.08 * h, 0.05 * h, 0.17 * h, 1, 1, 2), at(s * 0.09 * h, 0.025 * h, 0.035 * h)));
                m.append(&xf(arm(0.037, 0.03), at(s * 0.245 * h, 0.815 * h, 0.0) * if s < 0.0 { Mat4::from_rotation_y(std::f32::consts::PI) } else { Mat4::IDENTITY }));
                m.append(&xf(sphere(0.034 * h, 10, 8), at(s * 0.415 * h, 0.815 * h, 0.0) * Mat4::from_scale(Vec3::new(1.3, 0.75, 0.6))));
            }
            m.append(&xf(cube(0.25 * h, 0.1 * h, 0.15 * h, 2, 2, 2), at(0.0, 0.52 * h, 0.0)));
            m.append(&xf(cylinder(0.11 * h, 0.135 * h, 0.3 * h, 16, 8, true), at(0.0, 0.69 * h, 0.0) * Mat4::from_scale(Vec3::new(1.0, 1.0, 0.65))));
            m.append(&xf(cylinder(0.035 * h, 0.035 * h, 0.07 * h, 10, 3, false), at(0.0, 0.865 * h, 0.0)));
            m.append(&xf(sphere(0.062 * h, 16, 12), at(0.0, 0.93 * h, 0.005 * h) * Mat4::from_scale(Vec3::new(0.9, 1.12, 1.0))));
        }
        "robot" | "android" | "mech" => {
            for s in [1.0f32, -1.0] {
                m.append(&xf(cube(0.1 * h, 0.44 * h, 0.1 * h, 1, 10, 1), at(s * 0.09 * h, 0.28 * h, 0.0)));
                m.append(&xf(cube(0.12 * h, 0.06 * h, 0.2 * h, 1, 1, 2), at(s * 0.09 * h, 0.03 * h, 0.03 * h)));
                m.append(&xf(cube(0.29 * h, 0.075 * h, 0.075 * h, 10, 1, 1), at(s * 0.245 * h, 0.815 * h, 0.0)));
                m.append(&xf(cube(0.06 * h, 0.06 * h, 0.04 * h, 1, 1, 1), at(s * 0.415 * h, 0.815 * h, 0.0)));
                m.append(&xf(cylinder(0.008 * h, 0.008 * h, 0.06 * h, 6, 1, true), at(s * 0.04 * h, 1.0 * h - 0.03 * h, 0.0)));
            }
            m.append(&xf(cube(0.24 * h, 0.1 * h, 0.16 * h, 2, 2, 2), at(0.0, 0.52 * h, 0.0)));
            m.append(&xf(cube(0.3 * h, 0.3 * h, 0.2 * h, 3, 6, 2), at(0.0, 0.70 * h, 0.0)));
            m.append(&xf(cube(0.15 * h, 0.13 * h, 0.14 * h, 2, 2, 2), at(0.0, 0.915 * h, 0.0)));
        }
        "alien" | "creature" => {
            for s in [1.0f32, -1.0] {
                m.append(&xf(cylinder(0.028 * h, 0.04 * h, 0.46 * h, 10, 10, true), at(s * 0.09 * h, 0.28 * h, 0.0)));
                m.append(&xf(sphere(0.045 * h, 10, 6), at(s * 0.09 * h, 0.03 * h, 0.03 * h) * Mat4::from_scale(Vec3::new(0.9, 0.5, 1.5))));
                m.append(&xf(arm(0.025, 0.018), at(s * 0.245 * h, 0.815 * h, 0.0) * if s < 0.0 { Mat4::from_rotation_y(std::f32::consts::PI) } else { Mat4::IDENTITY }));
                m.append(&xf(sphere(0.03 * h, 10, 6), at(s * 0.42 * h, 0.815 * h, 0.0) * Mat4::from_scale(Vec3::new(1.6, 0.5, 0.9))));
            }
            m.append(&xf(sphere(0.12 * h, 16, 12), at(0.0, 0.62 * h, 0.0) * Mat4::from_scale(Vec3::new(1.0, 1.5, 0.75))));
            m.append(&xf(cylinder(0.025 * h, 0.025 * h, 0.08 * h, 8, 3, false), at(0.0, 0.85 * h, 0.0)));
            m.append(&xf(sphere(0.1 * h, 18, 14), at(0.0, 0.9 * h, 0.0) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.05))));
        }
        _ => return Err(format!("Unknown character style '{style}' ({})", STYLES.iter().map(|(n, d)| format!("{n}: {d}")).collect::<Vec<_>>().join(", "))),
    }
    Ok(m)
}

// ------------------------------------------------------------------------------ app actions

pub struct ShotSpec {
    pub theme: String,
    pub subject: Option<String>,
    pub anim: Option<String>,
    pub mood: Option<String>,
    pub camera: String,
    pub fx: Vec<String>,
    pub wind: bool,
    pub start: f32,
    pub end: f32,
    pub clear: bool,
    pub notes: Vec<String>,
}

impl Default for ShotSpec {
    fn default() -> Self {
        ShotSpec { theme: "studio".into(), subject: None, anim: None, mood: None, camera: "track".into(), fx: Vec::new(), wind: false, start: 1.0, end: 120.0, clear: false, notes: Vec::new() }
    }
}

impl App {
    fn add_mesh(&mut self, name: &str, mesh: Mesh, parent: Option<NodeId>) -> NodeId {
        let id = self.scene.add_node(name, Shape::Poly { history: None, mesh, rev: crate::scene::next_rev() }, parent);
        self.apply_name(id, Some(name));
        id
    }

    fn shade(&mut self, ids: &[NodeId], preset: &str, rgb: Option<[f32; 3]>, name: &str) {
        let _ = self.apply_material_preset(preset, rgb, Some(name), ids);
    }

    /// A procedural T-posed character (one mesh), ready for Quick Rig / rigAnimate.
    pub fn create_character(&mut self, style: &str, height: f32, name: Option<&str>, pos: Vec3) -> Result<NodeId, String> {
        let h = height.clamp(0.2, 20.0);
        let style = style.to_lowercase();
        let mut mesh = character_mesh(&style, h)?;
        // Placement baked into the points, so Quick Rig binds at the right spot.
        for p in &mut mesh.positions {
            *p += pos;
        }
        self.checkpoint();
        let base = match style.as_str() {
            "robot" | "android" | "mech" => "robot",
            "alien" | "creature" => "alien",
            _ => "character",
        };
        let id = self.add_mesh(name.unwrap_or(base), mesh, None);
        match base {
            "robot" => self.shade(&[id], "brushed_metal", Some([0.75, 0.78, 0.82]), "robot_metal"),
            "alien" => self.shade(&[id], "plastic", Some([0.35, 0.75, 0.4]), "alien_skin"),
            _ => self.shade(&[id], "skin", None, "character_skin"),
        }
        self.sel = vec![id];
        Ok(id)
    }

    /// A themed set (group "set_<theme>"): ground plus props, scattered with a seed, leaving
    /// a clear lane along Z through the middle for the subject.
    pub fn create_environment(&mut self, theme: &str, size: f32, density: f32, seed: u32) -> Result<NodeId, String> {
        use crate::mesh::{cone, cube, cylinder, plane, sphere};
        let theme = theme.to_lowercase();
        if !THEMES.iter().any(|t| t.0 == theme) {
            return Err(format!("Unknown environment '{theme}' ({})", THEMES.map(|t| t.0).join(", ")));
        }
        let size = size.clamp(4.0, 200.0);
        let density = density.clamp(0.0, 3.0);
        self.checkpoint();
        let grp = self.scene.add_node(&format!("set_{theme}"), Shape::Group, None);
        self.apply_name(grp, Some(&format!("set_{theme}")));
        let mut rng = Rng::new(seed as u64 + 17);
        let half = size * 0.5;
        // Positions off the central lane (|x| > lane), inside the set.
        let spot = |rng: &mut Rng, lane: f32| -> Vec3 {
            loop {
                let p = Vec3::new(rng.s() * half * 0.95, 0.0, rng.s() * half * 0.95);
                if p.x.abs() > lane {
                    return p;
                }
            }
        };
        let count = |base: f32| ((base * density * (size / 20.0).powi(2)).round() as usize).clamp(0, 400);
        let ground = |app: &mut App, w: f32, d: f32, preset: &str, rgb: [f32; 3], name: &str| {
            let id = app.add_mesh(name, plane(w, d, 1, 1), Some(grp));
            app.shade(&[id], preset, Some(rgb), &format!("{name}_mat"));
            id
        };
        match theme.as_str() {
            "forest" | "snow" => {
                let snow = theme == "snow";
                ground(self, size, size, if snow { "plastic" } else { "grass" }, if snow { [0.92, 0.94, 0.97] } else { [0.25, 0.42, 0.15] }, "ground");
                let (mut trunks, mut leaves, mut rocks) = (Mesh::default(), Mesh::default(), Mesh::default());
                for _ in 0..count(if snow { 35.0 } else { 45.0 }) {
                    let p = spot(&mut rng, 1.8);
                    let th = 1.2 + rng.f() * 1.5;
                    trunks.append(&xf(cylinder(0.14, 0.09, th, 8, 1, true), at(p.x, th * 0.5, p.z)));
                    if snow || rng.f() < 0.5 {
                        // Pine: stacked cones.
                        for k in 0..3 {
                            let r = (1.1 - k as f32 * 0.3) * (0.8 + rng.f() * 0.4);
                            leaves.append(&xf(cone(r, 1.3, 10), at(p.x, th + 0.3 + k as f32 * 0.65, p.z)));
                        }
                    } else {
                        let r = 0.9 + rng.f() * 0.6;
                        leaves.append(&xf(sphere(r, 12, 8), at(p.x, th + r * 0.7, p.z) * Mat4::from_scale(Vec3::new(1.0, 0.85, 1.0))));
                    }
                }
                for _ in 0..count(20.0) {
                    let p = spot(&mut rng, 1.2);
                    let r = 0.15 + rng.f() * 0.4;
                    rocks.append(&xf(sphere(r, 8, 6), at(p.x, r * 0.25, p.z) * Mat4::from_scale(Vec3::new(1.0 + rng.f() * 0.5, 0.6, 1.0))));
                }
                let t = self.add_mesh("tree_trunks", trunks, Some(grp));
                self.shade(&[t], "wood", None, "bark");
                let l = self.add_mesh("tree_leaves", leaves, Some(grp));
                self.shade(&[l], if snow { "plastic" } else { "grass" }, Some(if snow { [0.12, 0.3, 0.2] } else { [0.18, 0.45, 0.12] }), "foliage");
                let r = self.add_mesh("rocks", rocks, Some(grp));
                self.shade(&[r], "concrete", Some([0.45, 0.44, 0.42]), "rock");
            }
            "city" => {
                ground(self, size, size, "concrete", [0.32, 0.32, 0.34], "ground");
                let road = self.add_mesh("road", xf(plane(5.0, size, 1, 1), at(0.0, 0.005, 0.0)), Some(grp));
                self.shade(&[road], "rubber", Some([0.08, 0.08, 0.09]), "asphalt");
                let mut bld = Mesh::default();
                let mut z = -half + 2.0;
                while z < half - 2.0 {
                    for side in [-1.0f32, 1.0] {
                        let (w, d, ht) = (3.0 + rng.f() * 3.0, 3.0 + rng.f() * 2.5, 3.0 + rng.f() * 14.0 * density.max(0.3));
                        bld.append(&xf(cube(w, ht, d, 1, (ht / 1.5) as u32 + 1, 1), at(side * (4.5 + w * 0.5 + rng.f()), ht * 0.5, z + d * 0.5)));
                    }
                    z += 3.0 + rng.f() * 3.0;
                }
                let b = self.add_mesh("buildings", bld, Some(grp));
                self.shade(&[b], "concrete", Some([0.62, 0.6, 0.58]), "facade");
            }
            "desert" => {
                // Dunes: a displaced grid.
                let mut g = plane(size, size, 60, 60);
                let k = 6.0 / size;
                for p in &mut g.positions {
                    p.y = ((p.x * k * 1.3).sin() * (p.z * k).cos() * 0.5 + (p.x * k * 3.1 + p.z * k * 2.3).sin() * 0.25 + 0.75) * (p.x.abs() / half).min(1.0).powf(0.6) * 1.2;
                }
                let gid = self.add_mesh("dunes", g, Some(grp));
                self.shade(&[gid], "concrete", Some([0.86, 0.72, 0.48]), "sand");
                let (mut cacti, mut rocks) = (Mesh::default(), Mesh::default());
                for _ in 0..count(18.0) {
                    let p = spot(&mut rng, 2.5);
                    let th = 1.0 + rng.f() * 1.8;
                    cacti.append(&xf(cylinder(0.18, 0.16, th, 10, 2, true), at(p.x, th * 0.5, p.z)));
                    let arm_h = th * (0.4 + rng.f() * 0.3);
                    cacti.append(&xf(cylinder(0.1, 0.1, 0.6, 8, 1, true), at(p.x + 0.3, arm_h + 0.25, p.z)));
                }
                for _ in 0..count(15.0) {
                    let p = spot(&mut rng, 1.5);
                    let r = 0.2 + rng.f() * 0.6;
                    rocks.append(&xf(sphere(r, 8, 6), at(p.x, r * 0.3, p.z) * Mat4::from_scale(Vec3::new(1.3, 0.7, 1.0))));
                }
                let c = self.add_mesh("cacti", cacti, Some(grp));
                self.shade(&[c], "plastic", Some([0.25, 0.5, 0.2]), "cactus");
                let r = self.add_mesh("rocks", rocks, Some(grp));
                self.shade(&[r], "concrete", Some([0.6, 0.42, 0.3]), "red_rock");
            }
            "beach" => {
                let sand = self.add_mesh("sand", xf(plane(size, size * 0.6, 1, 1), at(0.0, 0.0, -size * 0.2)), Some(grp));
                self.shade(&[sand], "concrete", Some([0.9, 0.8, 0.6]), "sand");
                let sea = self.add_mesh("sea", xf(plane(size * 1.5, size, 1, 1), at(0.0, -0.05, size * 0.55)), Some(grp));
                self.shade(&[sea], "water", None, "sea_water");
                let (mut trunks, mut fronds) = (Mesh::default(), Mesh::default());
                for _ in 0..count(10.0) {
                    let mut p = spot(&mut rng, 2.5);
                    p.z = -size * 0.45 + rng.f() * size * 0.5;
                    // A curved trunk: a tapering tube swept along a bent path.
                    let lean = Vec3::new(rng.s(), 0.0, rng.s()).normalize_or(Vec3::X) * (0.6 + rng.f() * 0.8);
                    let th = 3.0 + rng.f() * 2.0;
                    let path = crate::nurbs::Curve::new(vec![p, p + Vec3::Y * th * 0.4 + lean * 0.2, p + Vec3::Y * th * 0.8 + lean * 0.7, p + Vec3::Y * th + lean], 3, false);
                    let prof = crate::nurbs::circle(0.14, 8, Vec3::Y, p);
                    trunks.append(&crate::nurbs::extrude(&prof, Some(&path), 0.0, Vec3::ZERO, 10, 12));
                    let top = p + Vec3::Y * th + lean;
                    for k in 0..7 {
                        let a = k as f32 / 7.0 * std::f32::consts::TAU + rng.f() * 0.3;
                        let q = Quat::from_rotation_y(a) * Quat::from_rotation_x(0.45 + rng.f() * 0.3);
                        let leaf = xf(cube(0.35, 0.03, 1.9, 1, 1, 4), Mat4::from_rotation_translation(q, top) * at(0.0, 0.0, 0.95));
                        fronds.append(&leaf);
                    }
                }
                let t = self.add_mesh("palm_trunks", trunks, Some(grp));
                self.shade(&[t], "wood", Some([0.5, 0.38, 0.25]), "palm_bark");
                let f = self.add_mesh("palm_fronds", fronds, Some(grp));
                self.shade(&[f], "grass", Some([0.2, 0.5, 0.15]), "palm_leaf");
            }
            "studio" => {
                // Cyclorama: an L profile with a smooth cove, extruded across.
                let r = 2.0;
                let w = size.min(30.0);
                let prof = crate::nurbs::Curve::new(vec![Vec3::new(-w * 0.5, 0.0, w * 0.4), Vec3::new(-w * 0.5, 0.0, -w * 0.25), Vec3::new(-w * 0.5, 0.0, -w * 0.25 - r), Vec3::new(-w * 0.5, r, -w * 0.25 - r), Vec3::new(-w * 0.5, w * 0.5, -w * 0.25 - r)], 3, false);
                let mut cyc = crate::nurbs::extrude(&prof, None, w, Vec3::X, 40, 2);
                // The inside of the cove is the visible side: make the floor face up.
                let floor = (0..cyc.faces.len()).find(|f| cyc.face_center(*f).y < 1e-3 && cyc.face_center(*f).z > 0.0);
                if floor.is_some_and(|f| cyc.face_normal(f).y < 0.0) {
                    cyc.reverse_normals();
                }
                let c = self.add_mesh("cyclorama", cyc, Some(grp));
                self.shade(&[c], "plastic", Some([0.92, 0.92, 0.92]), "cyc_white");
            }
            "space" => {
                let mut g = plane(size, size, 50, 50);
                let mut r2 = Rng::new(seed as u64 + 99);
                let craters: Vec<(Vec3, f32)> = (0..12).map(|_| (Vec3::new(r2.s() * half, 0.0, r2.s() * half), 0.8 + r2.f() * 2.0)).collect();
                for p in &mut g.positions {
                    let mut y = 0.0;
                    for (c, r) in &craters {
                        let d = Vec3::new(p.x - c.x, 0.0, p.z - c.z).length() / r;
                        if d < 1.3 {
                            y += if d < 1.0 { -0.35 * (1.0 - d * d) } else { 0.15 * (1.3 - d) / 0.3 };
                        }
                    }
                    p.y = y * (p.x.abs() / 2.0).min(1.0);
                }
                let gid = self.add_mesh("moon_surface", g, Some(grp));
                self.shade(&[gid], "concrete", Some([0.5, 0.5, 0.52]), "regolith");
                let planet = self.add_mesh("planet", xf(sphere(size * 0.35, 48, 32), at(-size * 0.3, size * 0.35, -size * 1.4)), Some(grp));
                self.shade(&[planet], "ceramic", Some([0.35, 0.5, 0.85]), "planet_surface");
            }
            _ => unreachable!(),
        }
        self.sel = vec![grp];
        Ok(grp)
    }

    /// Where the subject is (rig root / character mesh centre) at a frame.
    fn subject_point(&mut self, target: Option<NodeId>, f: f32) -> (Vec3, f32) {
        self.set_time(f);
        crate::rig::evaluate(&mut self.scene);
        if let (Some(root), Some(cog)) = (self.scene.find_by_name("root_ctrl"), self.scene.find_by_name("cog_ctrl")) {
            let r = crate::rig::world_pos(&self.scene, root);
            let c = crate::rig::world_pos(&self.scene, cog);
            let h = ((c.y - r.y) / 0.53).max(0.2);
            return (Vec3::new(c.x, r.y + 0.55 * h, c.z), h);
        }
        match target.and_then(|t| self.world_bounds(&[t])) {
            Some((lo, hi)) => ((lo + hi) * 0.5, (hi - lo).y.max((hi - lo).max_element() * 0.5).max(0.5)),
            None => (Vec3::new(0.0, 1.0, 0.0), 2.0),
        }
    }

    /// A keyed camera for the shot. `kind`: static, track, dolly, orbit, crane.
    pub fn create_shot_camera(&mut self, kind: &str, target: Option<NodeId>, start: f32, end: f32, name: Option<&str>) -> Result<NodeId, String> {
        let kind = kind.to_lowercase();
        if !CAMERAS.iter().any(|c| c.0 == kind) {
            return Err(format!("Unknown camera move '{kind}' ({})", CAMERAS.map(|c| c.0).join(", ")));
        }
        let t0 = self.scene.time;
        let end = end.max(start + 1.0);
        // Sample the subject first (this changes the time).
        let n = (((end - start) / 6.0).ceil() as usize).clamp(2, 60);
        let frames: Vec<f32> = (0..=n).map(|i| start + (end - start) * i as f32 / n as f32).collect();
        let samples: Vec<(Vec3, f32)> = frames.iter().map(|f| self.subject_point(target, *f)).collect();
        self.set_time(t0);
        crate::rig::evaluate(&mut self.scene);
        let h = samples.iter().map(|s| s.1).fold(0.0, f32::max).max(0.5);
        let first = samples[0].0;
        let last = samples[samples.len() - 1].0;
        let mid = (first + last) * 0.5;
        let travel = first.distance(last);
        self.checkpoint();
        let id = self.create_camera(Some(name.unwrap_or("shotCam")), 35.0);
        let mut curves: Vec<Curve> = (0..6).map(|_| Curve::default()).collect();
        let mut prev = Vec3::ZERO;
        for (i, (f, (p, _))) in frames.iter().zip(&samples).enumerate() {
            let u = i as f32 / n as f32;
            let (eye, look) = match kind.as_str() {
                "static" => (mid + Vec3::new(3.2 * h + travel * 0.55, 1.1 * h, 1.2 * h + travel * 0.35), mid),
                "track" => (*p + Vec3::new(3.0 * h, 0.35 * h, 0.9 * h), *p + Vec3::Z * 0.25 * h),
                "dolly" => (*p + Vec3::new(0.5 * h, 0.15 * h, 5.5 * h + (2.2 * h - 5.5 * h) * u), *p),
                "orbit" => {
                    let a = (20.0 + 140.0 * u).to_radians();
                    (*p + Vec3::new(a.sin() * 3.2 * h, 0.6 * h, a.cos() * 3.2 * h), *p)
                }
                _ => (*p + Vec3::new(1.4 * h, 0.1 * h + 2.6 * h * u, 2.6 * h + 2.5 * h * u), *p),
            };
            let mut e = crate::look::aim_euler(look - eye);
            if i > 0 {
                for k in 0..3 {
                    while e[k] - prev[k] > 180.0 {
                        e[k] -= 360.0;
                    }
                    while e[k] - prev[k] < -180.0 {
                        e[k] += 360.0;
                    }
                }
            }
            prev = e;
            for (c, v) in curves.iter_mut().zip([eye.x, eye.y, eye.z, e.x, e.y, e.z]) {
                c.set_key(*f, v);
            }
            if kind == "static" {
                break;
            }
        }
        let chans = [Channel::TranslateX, Channel::TranslateY, Channel::TranslateZ, Channel::RotateX, Channel::RotateY, Channel::RotateZ];
        if let Some(nd) = self.scene.node_mut(id) {
            for (ch, c) in chans.into_iter().zip(curves) {
                nd.set(ch, c.keys[0].value);
                if c.keys.len() > 1 {
                    nd.anim.insert(ch, c);
                }
            }
        }
        self.scene.evaluate();
        self.render.camera = crate::render_view::RenderCam::Node(id);
        self.sel = vec![id];
        Ok(id)
    }

    /// FX preset by name (see `FX`). `area` = set size; `subject` = character mesh.
    pub fn create_fx_preset(&mut self, name: &str, area: f32, subject: Option<NodeId>, wind: bool) -> Result<NodeId, String> {
        let a = area.clamp(2.0, 100.0);
        let windv = if wind { Vec3::new(3.0, 0.0, 0.5) } else { Vec3::ZERO };
        let vol = |app: &mut App, s: ParticleSettings, y: f32, sy: f32, nm: &str, half: f32| -> Result<NodeId, String> {
            let id = app.create_emitter(s, Some(Vec3::new(0.0, y, 0.0)), Some(nm))?;
            if let Some(n) = app.scene.node_mut(id) {
                n.scale = Vec3::new(half * 2.0, sy, half * 2.0);
            }
            Ok(id)
        };
        let base = ParticleSettings { kind: EmitterKind::Volume, speed: 0.0, speed_random: 0.0, spread: 0.0, ..Default::default() };
        match name {
            "rain" => vol(self, ParticleSettings { rate: (a * a * 4.0).min(4000.0), gravity: Vec3::new(0.0, -22.0, 0.0), drag: 0.0, wind: windv, lifespan: 1.0, lifespan_random: 0.1, radius: 0.012, color: [0.65, 0.72, 0.85], color_end: [0.65, 0.72, 0.85], bounce: 0.0, friction: 1.0, ..base }, 9.0, 0.2, "rain", a * 0.4),
            "snow" => vol(self, ParticleSettings { rate: (a * a * 0.6).min(1500.0), gravity: Vec3::new(0.0, -1.2, 0.0), drag: 1.2, wind: windv + Vec3::new(0.3, 0.0, 0.0), turbulence: 1.2, turbulence_freq: 0.6, lifespan: 12.0, radius: 0.03, color: [1.0; 3], color_end: [0.92, 0.95, 1.0], bounce: 0.0, friction: 1.0, ..base }, 7.0, 0.2, "snow", a * 0.45),
            "leaves" => vol(self, ParticleSettings { rate: (a * a * 0.15).min(600.0), gravity: Vec3::new(0.0, -1.5, 0.0), drag: 1.5, wind: windv + Vec3::new(0.6, 0.0, 0.2), turbulence: 2.0, turbulence_freq: 0.5, lifespan: 10.0, radius: 0.05, color: [0.95, 0.55, 0.12], color_end: [0.55, 0.25, 0.08], bounce: 0.0, friction: 1.0, ..base }, 6.0, 0.3, "leaves", a * 0.4),
            "dust" => vol(self, ParticleSettings { rate: (a * a * 0.5).min(1200.0), gravity: Vec3::new(0.0, -0.15, 0.0), drag: 1.0, wind: Vec3::new(2.5, 0.0, 0.4) + windv, turbulence: 1.0, turbulence_freq: 0.8, lifespan: 6.0, radius: 0.02, color: [0.85, 0.72, 0.52], color_end: [0.7, 0.6, 0.45], ..base }, 0.8, 1.2, "dust", a * 0.4),
            "fireflies" => vol(self, ParticleSettings { rate: 25.0, gravity: Vec3::ZERO, drag: 1.0, turbulence: 1.5, turbulence_freq: 0.7, lifespan: 6.0, radius: 0.03, color: [1.0, 0.95, 0.35], color_end: [0.6, 1.0, 0.25], ground: false, ..base }, 1.2, 1.6, "fireflies", a * 0.25),
            "sparks" => {
                let p = subject.and_then(|s| self.world_bounds(&[s])).map(|(lo, hi)| Vec3::new((lo.x + hi.x) * 0.5 + 0.8, 0.05, (lo.z + hi.z) * 0.5)).unwrap_or(Vec3::new(1.0, 0.05, 0.0));
                let s = ParticleSettings { kind: EmitterKind::Directional, dir: Vec3::Y, spread: 35.0, rate: 300.0, speed: 4.5, speed_random: 0.4, lifespan: 1.4, lifespan_random: 0.4, drag: 0.2, wind: windv, radius: 0.012, color: [1.0, 0.9, 0.4], color_end: [1.0, 0.25, 0.05], bounce: 0.45, ..Default::default() };
                self.create_emitter(s, Some(p), Some("sparks"))
            }
            "cape" => {
                let ch = subject.ok_or("cape: the shot needs a character")?;
                let chest = self.scene.find_by_name("chest").ok_or("cape: rig the character first (rigAnimate / quickRig)")?;
                let cp = crate::rig::world_pos(&self.scene, chest);
                let (lo, hi) = self.world_bounds(&[ch]).ok_or("cape: no character bounds")?;
                let h = (hi.y - lo.y).max(0.3);
                let (w, len) = (0.32 * h, 0.55 * h);
                let mut m = crate::mesh::plane(w, len, 8, 12);
                // Hang it vertically behind the back, top edge at the shoulders.
                let back = lo.z.max(cp.z - 0.12 * h) - 0.025 * h;
                let rot = Mat4::from_translation(Vec3::new(cp.x, cp.y + 0.02 * h - len * 0.5, back)) * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2);
                m = xf(m, rot);
                self.checkpoint();
                let id = self.add_mesh("cape", m, None);
                self.shade(&[id], "fabric", Some([0.7, 0.08, 0.1]), "cape_cloth");
                self.create_cloth(&[id], |s| {
                    s.thickness = 0.012 * h;
                    s.bend = 0.05;
                    s.wind = windv;
                })?;
                let pts = self.scene.node(id).unwrap().mesh().unwrap().positions.clone();
                let top = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
                let pins: Vec<u32> = pts.iter().enumerate().filter(|(_, p)| (p.y - top).abs() < 1e-4).map(|(i, _)| i as u32).collect();
                self.pin_cloth(id, &pins, Some(chest), true)?;
                self.connect_collider(ch, id, false)?;
                Ok(id)
            }
            "flag" => {
                use crate::mesh::{cylinder, plane};
                let base = Vec3::new(a * 0.18 + 2.0, 0.0, -a * 0.1);
                let pole = self.add_mesh("flag_pole", xf(cylinder(0.04, 0.04, 3.2, 10, 1, true), at(base.x, 1.6, base.z)), None);
                self.shade(&[pole], "chrome", None, "pole_metal");
                let fl = xf(plane(1.4, 0.9, 14, 9), Mat4::from_translation(Vec3::new(base.x + 0.74, 2.7, base.z)) * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2));
                let id = self.add_mesh("flag", fl, None);
                self.shade(&[id], "fabric", Some([0.85, 0.12, 0.12]), "flag_cloth");
                self.create_cloth(&[id], |s| {
                    s.wind = Vec3::new(5.0, 0.0, 1.5) + windv;
                    s.bend = 0.05;
                })?;
                let pts = self.scene.node(id).unwrap().mesh().unwrap().positions.clone();
                let left = pts.iter().map(|p| p.x).fold(f32::MAX, f32::min);
                let pins: Vec<u32> = pts.iter().enumerate().filter(|(_, p)| (p.x - left).abs() < 1e-4).map(|(i, _)| i as u32).collect();
                self.pin_cloth(id, &pins, None, true)?;
                Ok(id)
            }
            _ => Err(format!("Unknown FX '{name}' ({})", FX.map(|f| f.0).join(", "))),
        }
    }

    /// Assemble a whole shot. Returns a summary.
    pub fn make_shot(&mut self, spec: &ShotSpec) -> Result<String, String> {
        self.begin_batch();
        let r = (|| -> Result<String, String> {
            if spec.clear {
                self.checkpoint();
                self.scene.nodes.clear();
                self.sel.clear();
                self.comp_sel.clear();
            }
            let (s, e) = (spec.start, spec.end.max(spec.start + 1.0));
            self.scene.play_start = s;
            self.scene.play_end = e;
            self.scene.anim_start = self.scene.anim_start.min(s);
            self.scene.anim_end = self.scene.anim_end.max(e);
            self.set_time(s);
            let size = 24.0;
            let mut parts = Vec::new();
            self.create_environment(&spec.theme, size, 1.0, 7)?;
            parts.push(format!("{} set", spec.theme));
            let mut subject = None;
            if let Some(style) = &spec.subject {
                let travel = matches!(spec.anim.as_deref(), Some("walk") | Some("run"));
                let z0 = if travel { -size * 0.2 } else { 0.0 };
                let id = self.create_character(style, 1.8, None, Vec3::new(0.0, 0.0, z0))?;
                subject = Some(id);
                parts.push(format!("{} character", style));
                let anim = spec.anim.clone().unwrap_or_else(|| "idle".into());
                self.sel = vec![id];
                self.rig_and_animate(&anim, Some(id), s, e, travel)?;
                parts.push(format!("{anim} animation"));
            }
            for fx in &spec.fx {
                match self.create_fx_preset(fx, size, subject, spec.wind) {
                    Ok(_) => parts.push(fx.clone()),
                    Err(err) => parts.push(format!("({fx} skipped: {err})")),
                }
            }
            let mood = spec.mood.clone().unwrap_or_else(|| if spec.theme == "studio" { "studio".into() } else { "outdoor".into() });
            let targets: Vec<NodeId> = subject.into_iter().collect();
            self.light_rig(&mood, &targets, 1.0)?;
            parts.push(format!("{mood} lighting"));
            self.render.settings.env = match mood.as_str() {
                "night" => 0.06,
                "dramatic" => 0.15,
                "sunset" => 0.45,
                "studio" | "product" => 0.6,
                _ => 0.8,
            };
            self.render.settings.env_background = spec.theme != "studio";
            let cam = self.create_shot_camera(&spec.camera, subject, s, e, Some("shotCam"))?;
            parts.push(format!("{} camera", spec.camera));
            let _ = crate::commands::execute(self, "lookThru shotCam");
            let mid = (s + (e - s) * 0.5).round();
            self.set_time(mid);
            crate::rig::evaluate(&mut self.scene);
            self.sel = subject.into_iter().chain(std::iter::once(cam)).take(1).collect();
            let mut msg = format!("Shot: {} · frames {s}-{e} · camera shotCam", parts.join(", "));
            if !spec.notes.is_empty() {
                msg.push_str(&format!(" · note: {}", spec.notes.join("; ")));
            }
            Ok(msg)
        })();
        self.end_batch();
        r
    }
}

// ------------------------------------------------------------------------------ text → shot

/// Does this request ask for a whole shot?
pub fn is_shot_request(text: &str) -> bool {
    let t = text.to_lowercase();
    ["make a shot", "create a shot", "build a shot", "make shot", "shot of", "shot:", "film a", "film of", "make a scene of", "create a scene of", "build a scene of", "make a movie", "make a film", "make a clip", "direct a shot"].iter().any(|k| t.contains(k))
}

/// Pick shot parameters from keywords.
pub fn parse_shot(text: &str) -> ShotSpec {
    let t = text.to_lowercase().replace([',', '.', '!', '?', ';', ':', '-'], " ");
    let w: Vec<&str> = t.split_whitespace().collect();
    let has = |k: &str| w.iter().any(|x| *x == k || (k.len() > 4 && x.starts_with(k)));
    let mut s = ShotSpec::default();
    let theme = [
        (&["forest", "woods", "jungle", "trees", "woodland"][..], "forest"),
        (&["city", "street", "urban", "downtown", "town", "buildings"][..], "city"),
        (&["desert", "dunes", "sahara", "sand"][..], "desert"),
        (&["beach", "ocean", "sea", "coast", "island", "shore"][..], "beach"),
        (&["space", "moon", "planet", "asteroid", "mars", "lunar"][..], "space"),
        (&["arctic", "winter", "tundra", "snowfield"][..], "snow"),
        (&["studio", "stage", "cyclorama", "turntable"][..], "studio"),
    ];
    let mut explicit_theme = false;
    for (keys, th) in theme {
        if keys.iter().any(|k| has(k)) {
            s.theme = th.into();
            explicit_theme = true;
            break;
        }
    }
    let snowy = has("snow") || has("snowy") || has("snowing") || has("blizzard");
    if snowy {
        if !explicit_theme {
            s.theme = "snow".into();
        }
        s.fx.push("snow".into());
    }
    s.subject = [
        (&["robot", "android", "mech", "droid", "cyborg"][..], "robot"),
        (&["alien", "creature", "martian", "extraterrestrial"][..], "alien"),
        (&["man", "woman", "person", "human", "guy", "girl", "boy", "character", "hero", "hiker", "explorer", "dancer", "astronaut", "runner", "someone", "kid"][..], "human"),
    ]
    .iter()
    .find(|(k, _)| k.iter().any(|x| has(x)))
    .map(|(_, v)| v.to_string());
    s.anim = [
        (&["walk", "walking", "walks", "stroll", "strolling", "wander", "wandering", "hike", "hiking"][..], "walk"),
        (&["run", "running", "runs", "sprint", "sprinting", "jog", "jogging", "chase"][..], "run"),
        (&["jump", "jumping", "jumps", "leap", "leaping", "hop"][..], "jump"),
        (&["wave", "waving", "waves", "greet", "greeting", "hello"][..], "wave"),
        (&["dance", "dancing", "dances", "party"][..], "dance"),
        (&["idle", "standing", "stands", "waiting", "breathing", "stand"][..], "idle"),
    ]
    .iter()
    .find(|(k, _)| k.iter().any(|x| has(x)))
    .map(|(_, v)| v.to_string());
    if s.anim.is_some() && s.subject.is_none() {
        s.subject = Some("human".into());
    }
    s.mood = [
        (&["sunset", "dusk", "golden", "sundown", "evening"][..], "sunset"),
        (&["night", "midnight", "moonlight", "moonlit", "dark", "nighttime"][..], "night"),
        (&["dramatic", "noir", "moody", "cinematic", "spooky", "horror"][..], "dramatic"),
        (&["product", "packshot", "showcase"][..], "product"),
        (&["day", "daylight", "sunny", "noon", "morning", "bright", "afternoon"][..], "outdoor"),
    ]
    .iter()
    .find(|(k, _)| k.iter().any(|x| has(x)))
    .map(|(_, v)| v.to_string());
    if s.mood.is_none() && s.theme == "studio" {
        s.mood = Some("studio".into());
    }
    s.camera = [
        (&["orbit", "orbiting", "circling", "around", "360", "turntable"][..], "orbit"),
        (&["dolly", "push", "pushing", "approach", "closer", "zoom"][..], "dolly"),
        (&["crane", "rising", "aerial", "drone", "overhead", "birds"][..], "crane"),
        (&["static", "wide", "establishing", "locked", "still"][..], "static"),
        (&["track", "tracking", "follow", "following", "follows", "side"][..], "track"),
    ]
    .iter()
    .find(|(k, _)| k.iter().any(|x| has(x)))
    .map(|(_, v)| v.to_string())
    .unwrap_or_else(|| if s.subject.is_some() { "track".into() } else { "orbit".into() });
    for (keys, fx) in [
        (&["rain", "raining", "rainy", "storm", "stormy", "drizzle"][..], "rain"),
        (&["sparks", "sparkling", "welding", "fire", "fireworks"][..], "sparks"),
        (&["fireflies", "firefly", "magic", "magical", "glowing"][..], "fireflies"),
        (&["dust", "dusty", "sandstorm", "haze", "hazy", "foggy", "fog", "mist", "misty"][..], "dust"),
        (&["leaves", "autumn", "fall", "falling"][..], "leaves"),
        (&["cape", "cloak", "superhero"][..], "cape"),
        (&["flag", "banner"][..], "flag"),
    ] {
        if keys.iter().any(|k| has(k)) && !s.fx.iter().any(|f| f == fx) {
            if fx == "leaves" && has("snow") {
                continue;
            }
            s.fx.push(fx.into());
        }
    }
    if s.fx.iter().any(|f| f == "cape") && s.subject.is_none() {
        s.subject = Some("human".into());
    }
    if has("fog") || has("foggy") || has("mist") || has("misty") {
        s.notes.push("volumetric fog isn't supported; used drifting dust instead".into());
    }
    s.wind = has("wind") || has("windy") || has("breeze") || has("blowing") || has("stormy") || has("storm");
    // Duration: "5 seconds", "10 sec", "200 frames".
    for (i, x) in w.iter().enumerate() {
        if let Ok(n) = x.parse::<f32>() {
            match w.get(i + 1).copied() {
                Some(u) if u.starts_with("sec") || u == "s" => s.end = s.start + (n * 24.0).round().max(12.0) - 1.0,
                Some(u) if u.starts_with("frame") => s.end = s.start + n.max(12.0) - 1.0,
                _ => {}
            }
        }
    }
    s.clear = has("new") || has("scratch") || has("fresh") || has("clear");
    s
}

/// The `makeShot` MEL statement for a spec.
pub fn shot_mel(s: &ShotSpec) -> String {
    let mut m = format!("makeShot -theme {}", s.theme);
    if let Some(x) = &s.subject {
        m.push_str(&format!(" -subject {x}"));
    }
    if let Some(x) = &s.anim {
        m.push_str(&format!(" -anim {x}"));
    }
    if let Some(x) = &s.mood {
        m.push_str(&format!(" -mood {x}"));
    }
    m.push_str(&format!(" -camera {}", s.camera));
    for f in &s.fx {
        m.push_str(&format!(" -fx {f}"));
    }
    if s.wind {
        m.push_str(" -wind");
    }
    m.push_str(&format!(" -s {} -e {}", s.start, s.end));
    if s.clear {
        m.push_str(" -clear");
    }
    m
}

// ------------------------------------------------------------------------------ MEL

fn take(t: &mut Vec<String>, flags: &[&str]) -> Option<String> {
    let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
    let v = t.get(i + 1).cloned()?;
    t.drain(i..i + 2);
    Some(v)
}

fn take_vec3(t: &mut Vec<String>, flags: &[&str]) -> Option<Vec3> {
    let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
    let n: Vec<f32> = t.iter().skip(i + 1).take(3).filter_map(|x| x.parse().ok()).collect();
    if n.len() < 3 {
        return None;
    }
    t.drain(i..i + 4);
    Some(Vec3::new(n[0], n[1], n[2]))
}

fn take_all(t: &mut Vec<String>, flags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(v) = take(t, flags) {
        out.push(v);
    }
    out
}

fn flag(t: &mut Vec<String>, flags: &[&str]) -> bool {
    match t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}"))) {
        Some(i) => {
            t.remove(i);
            true
        }
        None => false,
    }
}

pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let mut t: Vec<String> = rest.to_vec();
    let num = |v: Option<String>, d: f32| v.and_then(|x| x.parse::<f32>().ok()).unwrap_or(d);
    let r = match cmd.as_str() {
        "character" => (|| {
            let style = take(&mut t, &["style", "st"]).unwrap_or_else(|| "human".into());
            let h = num(take(&mut t, &["h", "height"]), 1.8);
            let name = take(&mut t, &["n", "name"]);
            let p = take_vec3(&mut t, &["p", "position"]).unwrap_or(Vec3::ZERO);
            let id = app.create_character(&style, h, name.as_deref(), p)?;
            Ok(app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default())
        })(),
        "environment" => (|| {
            let theme = take(&mut t, &["theme", "t"]).or_else(|| t.iter().find(|x| !x.starts_with('-')).cloned()).unwrap_or_else(|| "studio".into());
            let size = num(take(&mut t, &["size", "s"]), 24.0);
            let dens = num(take(&mut t, &["density", "d"]), 1.0);
            let seed = num(take(&mut t, &["seed"]), 7.0) as u32;
            let id = app.create_environment(&theme, size, dens, seed)?;
            Ok(app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default())
        })(),
        "shotCamera" => (|| {
            let kind = take(&mut t, &["type", "t"]).unwrap_or_else(|| "track".into());
            let s = num(take(&mut t, &["s", "start"]), app.scene.play_start);
            let e = num(take(&mut t, &["e", "end"]), app.scene.play_end);
            let name = take(&mut t, &["n", "name"]);
            let target = match take(&mut t, &["target", "tgt"]) {
                Some(n) => Some(app.scene.find_by_name(&n).ok_or_else(|| format!("No object matches name: {n}"))?),
                None => app.sel.first().copied(),
            };
            let id = app.create_shot_camera(&kind, target, s, e, name.as_deref())?;
            Ok(app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default())
        })(),
        "fxPreset" => (|| {
            let name = t.iter().find(|x| !x.starts_with('-')).cloned().ok_or("fxPreset rain|snow|sparks|fireflies|dust|leaves|cape|flag")?;
            let wind = flag(&mut t, &["wind", "w"]);
            let subj = app.sel.iter().copied().find(|i| app.scene.node(*i).is_some_and(|n| n.mesh().is_some()));
            let id = app.create_fx_preset(&name, 24.0, subj, wind)?;
            Ok(app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default())
        })(),
        "makeShot" => (|| {
            let mut s = ShotSpec::default();
            if let Some(v) = take(&mut t, &["theme", "t"]) {
                s.theme = v;
            }
            s.subject = take(&mut t, &["subject", "sub", "character"]).filter(|v| v != "none");
            s.anim = take(&mut t, &["anim", "a"]);
            s.mood = take(&mut t, &["mood", "light", "lighting"]);
            if let Some(v) = take(&mut t, &["camera", "cam"]) {
                s.camera = v;
            }
            s.fx = take_all(&mut t, &["fx"]);
            s.wind = flag(&mut t, &["wind"]);
            s.clear = flag(&mut t, &["clear", "new"]);
            s.start = num(take(&mut t, &["s", "start"]), 1.0);
            s.end = num(take(&mut t, &["e", "end"]), 120.0);
            if s.anim.is_some() && s.subject.is_none() {
                s.subject = Some("human".into());
            }
            app.make_shot(&s)
        })(),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn sentence_to_shot_spec() {
        let s = parse_shot("make a shot of a robot walking through a snowy forest at sunset, tracking shot, 4 seconds");
        assert_eq!((s.theme.as_str(), s.subject.as_deref(), s.anim.as_deref(), s.mood.as_deref(), s.camera.as_str()), ("forest", Some("robot"), Some("walk"), Some("sunset"), "track"));
        assert_eq!(s.fx, vec!["snow"]);
        assert_eq!(s.end, 96.0);
        let s = parse_shot("film an alien dancing on the moon at night with fireflies, orbit camera");
        assert_eq!((s.theme.as_str(), s.subject.as_deref(), s.anim.as_deref(), s.mood.as_deref(), s.camera.as_str()), ("space", Some("alien"), Some("dance"), Some("night"), "orbit"));
        assert_eq!(s.fx, vec!["fireflies"]);
        let s = parse_shot("make a shot: rainy city street, a hero with a cape runs, windy, crane shot");
        assert_eq!((s.theme.as_str(), s.subject.as_deref(), s.anim.as_deref(), s.camera.as_str()), ("city", Some("human"), Some("run"), "crane"));
        assert!(s.fx.contains(&"rain".into()) && s.fx.contains(&"cape".into()) && s.wind);
        assert!(is_shot_request("Make a shot of a beach"));
        assert!(!is_shot_request("make a cube"));
        assert_eq!(shot_mel(&parse_shot("make a shot of a studio")), "makeShot -theme studio -mood studio -camera orbit -s 1 -e 120");
    }

    #[test]
    fn characters_rig_cleanly() {
        for style in ["human", "robot", "alien"] {
            let mut app = App::new();
            app.scene.nodes.clear();
            execute(&mut app, &format!("character -style {style} -h 1.8 -n hero")).unwrap();
            let id = app.scene.find_by_name("hero").unwrap();
            let (lo, hi) = app.world_bounds(&[id]).unwrap();
            assert!(lo.y.abs() < 0.02 && (hi.y - 1.8).abs() < 0.08, "{style}: {lo} {hi}");
            assert!((hi.x - 0.45 * 1.8).abs() < 0.06 && (lo.x + 0.45 * 1.8).abs() < 0.06, "{style} arm reach {lo} {hi}");
            execute(&mut app, "rigAnimate -anim wave -s 1 -e 24 hero").unwrap();
            let skin = app.scene.node(id).unwrap().skin.as_ref().unwrap();
            // Every vertex is weighted, and the hands follow the hand joints.
            assert!(skin.weights.iter().all(|w| (w.iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-3));
            let wrist = app.scene.find_by_name("L_wrist").unwrap();
            let wi = skin.joints.iter().position(|j| *j == wrist).unwrap();
            let mesh = app.scene.node(id).unwrap().mesh().unwrap();
            let hand_v = mesh.positions.iter().enumerate().filter(|(_, p)| p.x > 0.4 * 1.8).map(|(i, _)| i).collect::<Vec<_>>();
            assert!(!hand_v.is_empty());
            let followed = hand_v.iter().filter(|v| skin.weight(**v, wi) > 0.5).count();
            assert!(followed * 2 > hand_v.len(), "{style}: hand vertices bound to the wrist ({followed}/{})", hand_v.len());
        }
    }

    #[test]
    fn make_shot_builds_everything_in_one_undo() {
        let mut app = App::new();
        let before = app.scene.nodes.len();
        let msg = execute(&mut app, "makeShot -theme forest -subject robot -anim walk -mood sunset -camera track -fx snow -fx cape -s 1 -e 48").unwrap();
        assert!(msg.contains("forest set") && msg.contains("robot character") && msg.contains("walk animation") && msg.contains("snow") && msg.contains("cape") && msg.contains("track camera"), "{msg}");
        for n in ["set_forest", "robot", "root_ctrl", "snow", "cape", "shotCam", "lightRig"] {
            assert!(app.scene.find_by_name(n).is_some(), "{n} missing");
        }
        // The camera follows the walking robot.
        let cam = app.scene.find_by_name("shotCam").unwrap();
        let root = app.scene.find_by_name("root_ctrl").unwrap();
        let mut cam_z = Vec::new();
        for f in [1.0, 48.0] {
            app.set_time(f);
            crate::rig::evaluate(&mut app.scene);
            cam_z.push((app.scene.world_matrix(cam).w_axis.z, crate::rig::world_pos(&app.scene, root).z));
        }
        assert!(cam_z[1].1 > cam_z[0].1 + 0.5, "robot walks +Z");
        assert!((cam_z[1].0 - cam_z[0].0) - (cam_z[1].1 - cam_z[0].1) < 0.3, "camera tracks it: {cam_z:?}");
        // The camera looks at the subject: subject in front of the camera (−Z in camera space).
        let inv = app.scene.world_matrix(cam).inverse();
        let local = inv.transform_point3(crate::rig::world_pos(&app.scene, root) + Vec3::Y);
        assert!(local.z < -1.0 && local.x.abs() < local.z.abs() * 0.5, "{local}");
        // The cape hangs from the chest and the snow falls.
        let cape = app.scene.find_by_name("cape").unwrap();
        assert!(app.scene.node(cape).unwrap().cloth.as_ref().unwrap().settings.pin_to == app.scene.find_by_name("chest"));
        assert_eq!(app.render.camera, crate::render_view::RenderCam::Node(cam));
        // One undo removes the whole shot.
        app.undo();
        assert_eq!(app.scene.nodes.len(), before);
    }

    #[test]
    fn every_theme_and_camera_builds() {
        for (th, _) in THEMES {
            let mut app = App::new();
            app.scene.nodes.clear();
            execute(&mut app, &format!("environment -theme {th} -size 20")).unwrap();
            let g = app.scene.find_by_name(&format!("set_{th}")).unwrap();
            let kids = app.scene.subtree(g);
            assert!(kids.len() >= 2, "{th}");
            assert!(kids.iter().all(|k| app.scene.node(*k).unwrap().mesh().is_none_or(|m| !m.faces.is_empty())), "{th} has empty meshes");
        }
        for (cam, _) in CAMERAS {
            let mut app = App::new();
            app.scene.nodes.clear();
            execute(&mut app, &format!("polyCube -n subj; shotCamera -type {cam} -target subj -s 1 -e 48 -n c")).unwrap();
            let c = app.scene.find_by_name("c").unwrap();
            for f in [1.0, 24.0, 48.0] {
                app.set_time(f);
                let inv = app.scene.world_matrix(c).inverse();
                let l = inv.transform_point3(Vec3::ZERO);
                assert!(l.z < 0.0 && (l.x.abs() / -l.z) < 0.6 && (l.y.abs() / -l.z) < 0.6, "{cam} frame {f}: subject at {l}");
            }
        }
    }
}
