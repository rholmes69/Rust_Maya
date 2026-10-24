//! High-level "recipes" shared by the UI, MEL and the AI: lighting rigs, material presets,
//! rig + procedural animation, and the render check (preview render + automatic critique).

use crate::app::App;
use crate::commands::execute;
use crate::scene::{Channel, NodeId, Shape};
use glam::Vec3;

// ------------------------------------------------------------------------------ lighting

pub const LIGHT_PRESETS: [(&str, &str); 6] = [
    ("studio", "Three-point studio: warm key spot with shadows, cool fill, rim light"),
    ("outdoor", "Daylight: high sun with shadows plus blue sky fill"),
    ("sunset", "Golden hour: low orange sun, purple ambient"),
    ("night", "Moonlight: dim blue moon plus a warm practical lamp"),
    ("product", "Product shot: soft area-light key, fill and top light, bright sky"),
    ("dramatic", "Low-key: one hard side light and a rim, little fill"),
];

/// Spherical direction: azimuth from +Z toward +X (degrees), elevation above the ground.
fn dir(az: f32, el: f32) -> Vec3 {
    let (a, e) = (az.to_radians(), el.to_radians());
    Vec3::new(a.sin() * e.cos(), e.sin(), a.cos() * e.cos())
}

fn fmt_v(v: Vec3) -> String {
    format!("{:.3} {:.3} {:.3}", v.x, v.y, v.z)
}

impl App {
    /// Bounds of the visible meshes (or the given objects).
    fn subject_bounds(&self, targets: &[NodeId]) -> (Vec3, f32) {
        let ids: Vec<NodeId> = if targets.is_empty() {
            self.scene.nodes.iter().filter(|n| n.mesh().is_some() && self.scene.is_visible(n.id)).map(|n| n.id).collect()
        } else {
            targets.iter().flat_map(|t| self.scene.subtree(*t)).filter(|id| self.scene.node(*id).is_some_and(|n| n.mesh().is_some())).collect()
        };
        // Ignore huge ground planes when framing the subject: drop meshes much wider than tall
        // if anything else exists.
        let flat = |id: &NodeId| self.world_bounds(&[*id]).is_some_and(|(lo, hi)| (hi.y - lo.y) < 0.02 * (hi - lo).length());
        let solid: Vec<NodeId> = ids.iter().copied().filter(|i| !flat(i)).collect();
        let use_ids = if solid.is_empty() { ids } else { solid };
        match self.world_bounds(&use_ids) {
            Some((lo, hi)) => ((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(0.5)),
            None => (Vec3::new(0.0, 1.0, 0.0), 2.0),
        }
    }

    /// Build a named lighting rig (group `lightRig`) around the subject. Replaces an existing
    /// `lightRig` group. `scale` multiplies every intensity.
    pub fn light_rig(&mut self, preset: &str, targets: &[NodeId], scale: f32) -> Result<String, String> {
        let preset = preset.to_lowercase();
        if !LIGHT_PRESETS.iter().any(|p| p.0 == preset) {
            return Err(format!("Unknown lighting preset '{preset}' ({})", LIGHT_PRESETS.map(|p| p.0).join(", ")));
        }
        let (c, r) = self.subject_bounds(targets);
        let k = scale.max(0.0);
        let d = r * 3.5;
        let aim = fmt_v(c);
        let cone = |dist: f32| (2.0 * (r * 1.4 / dist).atan().to_degrees()).clamp(15.0, 120.0);
        let mut mel: Vec<String> = Vec::new();
        if let Some(old) = self.scene.find_by_name("lightRig") {
            let name = self.scene.node(old).unwrap().name.clone();
            mel.push(format!("delete {name}"));
        }
        let spot = |n: &str, p: Vec3, i: f32, rgb: [f32; 3], sh: bool, pen: f32| {
            let dist = (p - c).length();
            format!(
                "spotLight -n {n} -p {} -aim {aim} -i {:.3} -rgb {} {} {} -ca {:.1} -pen {pen:.1}{}",
                fmt_v(p),
                i * k,
                rgb[0],
                rgb[1],
                rgb[2],
                cone(dist),
                if sh { " -sh on" } else { "" }
            )
        };
        let dirl = |n: &str, from: Vec3, i: f32, rgb: [f32; 3], sh: bool| {
            format!("directionalLight -n {n} -p {} -aim {aim} -i {:.3} -rgb {} {} {}{}", fmt_v(c + from * d), i * k, rgb[0], rgb[1], rgb[2], if sh { " -sh on" } else { "" })
        };
        let point = |n: &str, p: Vec3, i: f32, rgb: [f32; 3]| format!("pointLight -n {n} -p {} -i {:.3} -rgb {} {} {}", fmt_v(p), i * k, rgb[0], rgb[1], rgb[2]);
        // Softbox: an area light 1.6× the subject radius on a side, intensity normalised so the
        // subject receives about `i` (area lights fall off with distance).
        let area = |n: &str, p: Vec3, i: f32, rgb: [f32; 3]| {
            let side = r * 1.6;
            let dist = (p - c).length();
            let norm = dist * dist / (side * side);
            format!("areaLight -n {n} -p {} -aim {aim} -w {side:.3} -h {side:.3} -i {:.3} -rgb {} {} {}", fmt_v(p), i * k * norm, rgb[0], rgb[1], rgb[2])
        };
        let amb = |n: &str, i: f32, rgb: [f32; 3]| format!("ambientLight -n {n} -i {:.3} -rgb {} {} {}", i * k, rgb[0], rgb[1], rgb[2]);
        let (lights, env): (Vec<String>, f32) = match preset.as_str() {
            "studio" => (
                vec![
                    spot("key_light", c + dir(40.0, 35.0) * d, 1.1, [1.0, 0.95, 0.88], true, 12.0),
                    spot("fill_light", c + dir(-55.0, 15.0) * d, 0.35, [0.8, 0.88, 1.0], false, 20.0),
                    dirl("rim_light", dir(160.0, 40.0), 0.7, [1.0, 1.0, 1.0], false),
                    amb("bounce_light", 0.05, [1.0, 1.0, 1.0]),
                ],
                0.2,
            ),
            "outdoor" => (vec![dirl("sun_light", dir(35.0, 55.0), 1.3, [1.0, 0.96, 0.88], true), amb("sky_fill", 0.12, [0.7, 0.8, 1.0])], 0.6),
            "sunset" => (
                vec![dirl("sun_light", dir(70.0, 10.0), 1.2, [1.0, 0.55, 0.3], true), amb("dusk_fill", 0.1, [0.55, 0.45, 0.8]), dirl("rim_light", dir(-120.0, 25.0), 0.25, [0.6, 0.6, 1.0], false)],
                0.25,
            ),
            "night" => (
                vec![dirl("moon_light", dir(-30.0, 45.0), 0.35, [0.55, 0.65, 1.0], true), point("lamp_light", c + Vec3::new(r * 1.2, r * 0.8, r * 0.9), 0.9, [1.0, 0.7, 0.4]), amb("night_fill", 0.04, [0.4, 0.45, 0.7])],
                0.04,
            ),
            "product" => (
                vec![
                    area("key_light", c + dir(35.0, 30.0) * d, 1.0, [1.0, 1.0, 1.0]),
                    area("fill_light", c + dir(-50.0, 20.0) * d, 0.5, [1.0, 1.0, 1.0]),
                    area("top_light", c + Vec3::Y * d, 0.5, [1.0, 1.0, 1.0]),
                    amb("bounce_light", 0.08, [1.0, 1.0, 1.0]),
                ],
                0.5,
            ),
            _ /* dramatic */ => (vec![spot("key_light", c + dir(90.0, 20.0) * d, 1.4, [1.0, 0.9, 0.8], true, 5.0), dirl("rim_light", dir(-150.0, 30.0), 0.8, [0.7, 0.8, 1.0], false)], 0.05),
        };
        let names: Vec<String> = lights.iter().map(|l| l.split_whitespace().nth(2).unwrap_or("").to_string()).collect();
        mel.extend(lights);
        mel.push(format!("select {}", names.join(" ")));
        mel.push("group -n lightRig".into());
        mel.push("select -cl".into());
        mel.push("lighting -all -shadows on".into());
        self.begin_batch();
        let r2: Result<(), String> = mel.iter().try_for_each(|m| execute(self, m).map(|_| ()).map_err(|e| format!("{m}: {e}")));
        self.end_batch();
        r2?;
        self.render.settings.env = env;
        Ok(format!("{preset} light rig: {} (sky {env})", names.join(", ")))
    }
}

// ------------------------------------------------------------------------------ materials

pub struct MatPreset {
    pub name: &'static str,
    pub kind: &'static str,
    pub rgb: [f32; 3],
    pub spec: f32,
    pub rough: f32,
    pub metal: f32,
    pub transp: f32,
    pub emit: f32,
}

const fn mp(name: &'static str, kind: &'static str, rgb: [f32; 3], spec: f32, rough: f32, metal: f32, transp: f32, emit: f32) -> MatPreset {
    MatPreset { name, kind, rgb, spec, rough, metal, transp, emit }
}

pub const MATERIAL_PRESETS: &[MatPreset] = &[
    mp("gold", "standardSurface", [1.0, 0.77, 0.34], 1.0, 0.25, 1.0, 0.0, 0.0),
    mp("silver", "standardSurface", [0.97, 0.96, 0.92], 1.0, 0.2, 1.0, 0.0, 0.0),
    mp("chrome", "standardSurface", [0.95, 0.95, 0.95], 1.0, 0.05, 1.0, 0.0, 0.0),
    mp("copper", "standardSurface", [0.95, 0.64, 0.54], 1.0, 0.3, 1.0, 0.0, 0.0),
    mp("brass", "standardSurface", [0.89, 0.75, 0.42], 1.0, 0.35, 1.0, 0.0, 0.0),
    mp("brushed_metal", "standardSurface", [0.91, 0.92, 0.92], 1.0, 0.5, 1.0, 0.0, 0.0),
    mp("plastic", "blinn", [0.8, 0.1, 0.1], 0.5, 0.3, 0.0, 0.0, 0.0),
    mp("rubber", "blinn", [0.05, 0.05, 0.05], 0.2, 0.9, 0.0, 0.0, 0.0),
    mp("glass", "standardSurface", [0.92, 0.96, 1.0], 1.0, 0.02, 0.0, 0.85, 0.0),
    mp("water", "standardSurface", [0.25, 0.45, 0.55], 1.0, 0.05, 0.0, 0.6, 0.0),
    mp("wood", "blinn", [0.55, 0.35, 0.18], 0.2, 0.6, 0.0, 0.0, 0.0),
    mp("concrete", "lambert", [0.55, 0.55, 0.52], 0.05, 0.95, 0.0, 0.0, 0.0),
    mp("marble", "blinn", [0.92, 0.91, 0.88], 0.5, 0.15, 0.0, 0.0, 0.0),
    mp("ceramic", "blinn", [0.95, 0.95, 0.93], 0.6, 0.1, 0.0, 0.0, 0.0),
    mp("car_paint", "standardSurface", [0.6, 0.02, 0.03], 1.0, 0.15, 0.3, 0.0, 0.0),
    mp("skin", "blinn", [0.8, 0.57, 0.47], 0.3, 0.5, 0.0, 0.0, 0.0),
    mp("fabric", "lambert", [0.3, 0.35, 0.6], 0.0, 1.0, 0.0, 0.0, 0.0),
    mp("neon", "lambert", [0.1, 0.9, 1.0], 0.0, 0.5, 0.0, 0.0, 1.0),
    mp("grass", "lambert", [0.22, 0.45, 0.12], 0.05, 0.9, 0.0, 0.0, 0.0),
];

pub fn material_preset(name: &str) -> Option<&'static MatPreset> {
    let n = name.to_lowercase().replace([' ', '-'], "_");
    let alias = match n.as_str() {
        "steel" | "aluminium" | "aluminum" | "metal" => "brushed_metal",
        "mirror" => "chrome",
        "glossy_plastic" => "plastic",
        "stone" | "cement" => "concrete",
        "porcelain" => "ceramic",
        "emissive" | "glow" | "light" => "neon",
        "cloth" => "fabric",
        "paint" => "car_paint",
        other => other,
    };
    MATERIAL_PRESETS.iter().find(|p| p.name == alias)
}

impl App {
    /// Create (or reuse) a preset material and assign it to the objects (default: selection).
    pub fn apply_material_preset(&mut self, preset: &str, rgb: Option<[f32; 3]>, name: Option<&str>, targets: &[NodeId]) -> Result<String, String> {
        let p = material_preset(preset).ok_or_else(|| format!("Unknown material preset '{preset}' ({})", MATERIAL_PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))?;
        let objs: Vec<NodeId> = if targets.is_empty() { self.sel.clone() } else { targets.to_vec() };
        let objs: Vec<NodeId> = objs.into_iter().flat_map(|t| self.scene.subtree(t)).filter(|id| self.scene.node(*id).is_some_and(|n| n.mesh().is_some())).collect();
        if objs.is_empty() {
            return Err("Material preset: select the objects to shade.".into());
        }
        let c = rgb.unwrap_or(p.rgb);
        let mat_name = name.map(String::from).unwrap_or_else(|| if rgb.is_some() { format!("{}_custom_mat", p.name) } else { format!("{}_mat", p.name) });
        let names: Vec<String> = objs.iter().filter_map(|id| self.scene.node(*id)).map(|n| n.name.clone()).collect();
        let mut mel = Vec::new();
        if self.material_by_name(&mat_name).is_none() {
            mel.push(format!("shadingNode -asShader {} -n {mat_name} -rgb {} {} {}", p.kind, c[0], c[1], c[2]));
            mel.push(format!("setAttr {mat_name}.specular {}", p.spec));
            mel.push(format!("setAttr {mat_name}.roughness {}", p.rough));
            mel.push(format!("setAttr {mat_name}.metalness {}", p.metal));
            if p.transp > 0.0 {
                mel.push(format!("setAttr {mat_name}.transparency {}", p.transp));
            }
            if p.emit > 0.0 {
                mel.push(format!("setAttr {mat_name}.incandescence {} {} {}", c[0] * p.emit, c[1] * p.emit, c[2] * p.emit));
            }
        }
        mel.push(format!("hyperShade -assign {mat_name} {}", names.join(" ")));
        self.begin_batch();
        let r: Result<(), String> = mel.iter().try_for_each(|m| execute(self, m).map(|_| ()).map_err(|e| format!("{m}: {e}")));
        self.end_batch();
        r?;
        Ok(format!("{mat_name} → {}", names.join(", ")))
    }
}

// ------------------------------------------------------------------------- rig + animate

pub const ANIMS: [(&str, &str); 6] = [
    ("walk", "Walk cycle (32 frames per cycle)"),
    ("run", "Run cycle (18 frames per cycle)"),
    ("jump", "Crouch, jump and land (30 frames)"),
    ("wave", "Wave the left hand"),
    ("idle", "Breathing idle"),
    ("dance", "Side-step dance with arms"),
];

/// A key to set: (control, channel, frame, value).
type Key = (NodeId, Channel, f32, f32);

impl App {
    /// Quick-rig the mesh if there is no rig yet, then key a procedural animation on the
    /// Quick Rig controls. `travel` moves the character forward (walk/run).
    pub fn rig_and_animate(&mut self, anim: &str, mesh: Option<NodeId>, start: f32, end: f32, travel: bool) -> Result<String, String> {
        let anim = anim.to_lowercase();
        if !ANIMS.iter().any(|a| a.0 == anim) {
            return Err(format!("Unknown animation '{anim}' ({})", ANIMS.map(|a| a.0).join(", ")));
        }
        self.begin_batch();
        let r = (|| {
            let mut msg = String::new();
            if !self.scene.name_exists("cog_ctrl") {
                let mesh = mesh.or_else(|| self.sel.iter().copied().find(|id| self.scene.node(*id).is_some_and(|n| n.mesh().is_some() && n.skin.is_none())));
                msg = self.quick_rig(mesh)?;
                msg.push_str("; ");
            }
            let n = self.key_rig_animation(&anim, start, end.max(start + 1.0), travel)?;
            msg.push_str(&format!("{anim}: {n} keys on frames {start}-{end}"));
            Ok(msg)
        })();
        self.end_batch();
        r
    }

    fn key_rig_animation(&mut self, anim: &str, start: f32, end: f32, travel: bool) -> Result<usize, String> {
        let find = |app: &App, n: &str| app.scene.find_by_name(n).ok_or_else(|| format!("rig control '{n}' not found (use quickRig)"));
        let root = find(self, "root_ctrl")?;
        let cog = find(self, "cog_ctrl")?;
        let chest = find(self, "chest_ctrl")?;
        let head = find(self, "head_ctrl")?;
        let lf = find(self, "L_foot_ctrl")?;
        let rf = find(self, "R_foot_ctrl")?;
        let lh = find(self, "L_hand_ctrl")?;
        let rh = find(self, "R_hand_ctrl")?;
        // Character height from the cog (hips at 0.53 H).
        let h = {
            let rw = crate::rig::world_pos(&self.scene, root);
            let cw = crate::rig::world_pos(&self.scene, cog);
            ((cw.y - rw.y) / 0.53).max(0.1)
        };
        self.checkpoint();
        // Rest values (current pose) of every animated control; remove old keys first.
        let ctrls = [root, cog, chest, head, lf, rf, lh, rh];
        for c in ctrls {
            if let Some(n) = self.scene.node_mut(c) {
                n.anim.clear();
            }
        }
        // Evaluate rest after clearing keys.
        let rest = |app: &App, id: NodeId, ch: Channel| app.scene.node(id).map(|n| n.get(ch)).unwrap_or(0.0);
        let mut keys: Vec<Key> = Vec::new();
        let mut key = |id: NodeId, ch: Channel, f: f32, delta: f32, app: &App| keys.push((id, ch, f, rest(app, id, ch) + delta));
        use Channel::*;
        let step = 2.0;
        let frames: Vec<f32> = {
            let mut v = Vec::new();
            let mut f = start;
            while f <= end + 1e-3 {
                v.push(f);
                f += step;
            }
            if (v.last().copied().unwrap_or(start) - end).abs() > 1e-3 {
                v.push(end);
            }
            v
        };
        let tau = std::f32::consts::TAU;
        match anim {
            "walk" | "run" => {
                let run = anim == "run";
                let cycle = if run { 18.0 } else { 32.0 };
                let stride = if run { 0.75 * h } else { 0.42 * h };
                let lift = if run { 0.12 * h } else { 0.06 * h };
                let bob = if run { 0.035 * h } else { 0.02 * h };
                let swing = if run { 0.16 * h } else { 0.1 * h };
                let speed = stride / cycle; // root distance per frame
                let foot = |p: f32| -> (f32, f32) {
                    let p = p.rem_euclid(1.0);
                    let stance = if run { 0.4 } else { 0.55 };
                    // Planted foot: the local offset cancels the root's forward motion.
                    let d = stride * stance;
                    if p < stance {
                        (d / 2.0 - d * (p / stance), 0.0)
                    } else {
                        let s = (p - stance) / (1.0 - stance);
                        let e = s * s * (3.0 - 2.0 * s);
                        (-d / 2.0 + d * e, lift * (std::f32::consts::PI * s).sin())
                    }
                };
                for &f in &frames {
                    let p = (f - start) / cycle;
                    let (lz, ly) = foot(p);
                    let (rz, ry) = foot(p + 0.5);
                    key(lf, TranslateZ, f, lz, self);
                    key(lf, TranslateY, f, ly, self);
                    key(rf, TranslateZ, f, rz, self);
                    key(rf, TranslateY, f, ry, self);
                    // Hips dip at each contact (twice per cycle).
                    key(cog, TranslateY, f, -bob * (1.0 + (2.0 * tau * p).cos()) * 0.5 - if run { 0.04 * h } else { 0.0 }, self);
                    key(cog, RotateY, f, 6.0 * (tau * p).sin(), self);
                    key(cog, RotateX, f, if run { 8.0 } else { 2.0 }, self);
                    key(chest, RotateY, f, -9.0 * (tau * p).sin(), self);
                    key(head, RotateY, f, 3.0 * (tau * p).sin(), self);
                    // Arms swing opposite to the legs.
                    key(lh, TranslateZ, f, -swing * (tau * p).cos(), self);
                    key(rh, TranslateZ, f, swing * (tau * p).cos(), self);
                    key(lh, TranslateY, f, -0.03 * h + 0.015 * h * (2.0 * tau * p).cos(), self);
                    key(rh, TranslateY, f, -0.03 * h + 0.015 * h * (2.0 * tau * p).cos(), self);
                    if travel {
                        key(root, TranslateZ, f, speed * (f - start), self);
                    }
                }
            }
            "jump" => {
                let len = 30.0;
                for &f in &frames {
                    let t = ((f - start) % len) / len;
                    // crouch 0-0.3, air 0.3-0.7 (parabola), land 0.7-0.85, recover.
                    let (dy, air) = if t < 0.3 {
                        let s = t / 0.3;
                        (-0.12 * h * (s * std::f32::consts::FRAC_PI_2).sin(), 0.0)
                    } else if t < 0.7 {
                        let s = (t - 0.3) / 0.4;
                        let y = 4.0 * s * (1.0 - s) * 0.35 * h;
                        (y, y)
                    } else if t < 0.85 {
                        let s = (t - 0.7) / 0.15;
                        (-0.1 * h * (s * std::f32::consts::PI).sin(), 0.0)
                    } else {
                        (0.0, 0.0)
                    };
                    key(cog, TranslateY, f, dy, self);
                    key(cog, RotateX, f, if t < 0.3 { 15.0 * (t / 0.3) } else if t < 0.7 { -5.0 } else { 10.0 * (1.0 - ((t - 0.7) / 0.3).min(1.0)) }, self);
                    key(lf, TranslateY, f, air * 0.85, self);
                    key(rf, TranslateY, f, air * 0.85, self);
                    let arms = if t < 0.3 { -0.1 * h * (t / 0.3) } else if t < 0.7 { 0.35 * h } else { 0.0 };
                    key(lh, TranslateY, f, arms + dy, self);
                    key(rh, TranslateY, f, arms + dy, self);
                    key(lh, TranslateZ, f, if t < 0.3 { -0.08 * h } else { 0.05 * h }, self);
                    key(rh, TranslateZ, f, if t < 0.3 { -0.08 * h } else { 0.05 * h }, self);
                }
            }
            "wave" => {
                for &f in &frames {
                    let t = f - start;
                    let up = (t / 10.0).min(1.0);
                    let e = up * up * (3.0 - 2.0 * up);
                    key(lh, TranslateY, f, e * 0.42 * h, self);
                    key(lh, TranslateX, f, -e * 0.12 * h + e * 0.05 * h * (tau * t / 16.0).sin(), self);
                    key(lh, TranslateZ, f, e * 0.06 * h, self);
                    key(lh, RotateZ, f, e * 25.0 * (tau * t / 16.0).sin(), self);
                    key(head, RotateZ, f, e * 6.0 * (tau * t / 32.0).sin(), self);
                    key(chest, RotateZ, f, -e * 3.0, self);
                }
            }
            "idle" => {
                for &f in &frames {
                    let t = (f - start) / 60.0;
                    key(chest, RotateX, f, -2.0 * (tau * t).sin(), self);
                    key(cog, TranslateY, f, -0.006 * h * (tau * t).sin(), self);
                    key(head, RotateY, f, 4.0 * (tau * t * 0.37).sin(), self);
                    key(lh, TranslateY, f, -0.005 * h * (tau * t).sin(), self);
                    key(rh, TranslateY, f, -0.005 * h * (tau * t).sin(), self);
                }
            }
            _ /* dance */ => {
                let beat = 12.0;
                for &f in &frames {
                    let p = (f - start) / (beat * 2.0);
                    let s = (tau * p).sin();
                    key(cog, TranslateX, f, 0.08 * h * s, self);
                    key(cog, TranslateY, f, -0.03 * h * (1.0 + (2.0 * tau * p).cos()) * 0.5, self);
                    key(cog, RotateZ, f, -5.0 * s, self);
                    key(chest, RotateY, f, 12.0 * s, self);
                    key(head, RotateZ, f, 6.0 * (2.0 * tau * p).sin(), self);
                    key(lf, TranslateX, f, (0.06 * h * s).max(0.0), self);
                    key(rf, TranslateX, f, (0.06 * h * s).min(0.0), self);
                    key(lf, TranslateY, f, 0.04 * h * (tau * p).sin().max(0.0), self);
                    key(rf, TranslateY, f, 0.04 * h * (-(tau * p).sin()).max(0.0), self);
                    key(lh, TranslateY, f, 0.25 * h * (0.5 + 0.5 * (2.0 * tau * p).sin()), self);
                    key(rh, TranslateY, f, 0.25 * h * (0.5 - 0.5 * (2.0 * tau * p).sin()), self);
                    key(lh, TranslateX, f, -0.05 * h * s, self);
                    key(rh, TranslateX, f, -0.05 * h * s, self);
                }
            }
        }
        let count = keys.len();
        for (id, ch, f, v) in keys {
            if let Some(n) = self.scene.node_mut(id) {
                n.anim.entry(ch).or_default().set_key(f, v);
            }
        }
        if self.scene.play_end < end {
            self.scene.play_end = end;
        }
        self.scene.anim_end = self.scene.anim_end.max(end);
        if self.scene.play_start > start {
            self.scene.play_start = start;
        }
        let t = self.scene.time;
        self.set_time(t);
        Ok(count)
    }
}

// ------------------------------------------------------------------------- render check

#[derive(Clone, Debug, Default)]
pub struct RenderCheck {
    pub width: u32,
    pub height: u32,
    /// Tone-mapped RGBA8 preview.
    pub rgba: Vec<u8>,
    pub png: Vec<u8>,
    pub coverage: f32,
    pub mean: f32,
    pub contrast: f32,
    pub clipped: f32,
    pub crushed: f32,
    pub edge: f32,
    pub default_light: bool,
    pub unshaded: usize,
    pub issues: Vec<String>,
    pub fixes: Vec<String>,
}

impl RenderCheck {
    pub fn summary(&self) -> String {
        format!(
            "Preview {}x{}: subject covers {:.0}% of the frame, mean brightness {:.2} (0-1), contrast {:.2}, {:.1}% clipped highlights, {:.1}% crushed blacks, {:.0}% of the frame border is covered.{}{}",
            self.width,
            self.height,
            self.coverage * 100.0,
            self.mean,
            self.contrast,
            self.clipped * 100.0,
            self.crushed * 100.0,
            self.edge * 100.0,
            if self.default_light { " The scene has no lights (default light)." } else { "" },
            if self.unshaded > 0 { format!(" {} mesh(es) use the default grey material.", self.unshaded) } else { String::new() }
        )
    }

    pub fn report(&self) -> String {
        let mut s = self.summary();
        if self.issues.is_empty() {
            s.push_str("\nNo obvious problems found.");
        } else {
            for i in &self.issues {
                s.push_str(&format!("\n• {i}"));
            }
        }
        s
    }
}

impl App {
    /// Render a quick preview and analyse it (exposure, framing, lighting, shading).
    pub fn render_check(&mut self, cam: Option<&str>, w: u32, h: u32, spp: u32) -> Result<RenderCheck, String> {
        let mut s = self.render.settings.clone();
        s.width = w;
        s.height = h;
        s.spp = spp;
        s.env_background = false;
        let job = self.render_now(cam, s.clone())?;
        let lin = job.final_linear();
        let rgba = crate::raytrace::tonemap(&lin, s.exposure, s.tonemap);
        let default_light = !self.scene.nodes.iter().any(|n| matches!(&n.shape, Shape::Light(l) if l.kind != crate::look::LightKind::Ambient) && self.scene.is_visible(n.id));
        let unshaded = self.scene.nodes.iter().filter(|n| n.mesh().is_some() && n.material.is_none() && self.scene.is_visible(n.id)).count();
        let mut rc = RenderCheck { width: w, height: h, default_light, unshaded, ..Default::default() };
        let (mut cov, mut sum, mut sum2, mut clip, mut crush) = (0usize, 0f64, 0f64, 0usize, 0usize);
        let mut edge_cov = 0usize;
        let mut edge_n = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let a = rgba[i * 4 + 3];
                let border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                if border {
                    edge_n += 1;
                }
                if a < 128 {
                    continue;
                }
                cov += 1;
                if border {
                    edge_cov += 1;
                }
                let l = (0.2126 * rgba[i * 4] as f64 + 0.7152 * rgba[i * 4 + 1] as f64 + 0.0722 * rgba[i * 4 + 2] as f64) / 255.0;
                sum += l;
                sum2 += l * l;
                if l > 0.98 {
                    clip += 1;
                }
                if l < 0.03 {
                    crush += 1;
                }
            }
        }
        let n = cov.max(1) as f64;
        rc.coverage = cov as f32 / (w * h) as f32;
        rc.mean = (sum / n) as f32;
        rc.contrast = ((sum2 / n - (sum / n).powi(2)).max(0.0).sqrt()) as f32;
        rc.clipped = clip as f32 / n as f32;
        rc.crushed = crush as f32 / n as f32;
        rc.edge = edge_cov as f32 / edge_n.max(1) as f32;
        let mut found: Vec<(String, Option<&str>)> = Vec::new();
        let mut issue = |t: &str, fix: Option<&'static str>| found.push((t.to_string(), fix));
        let (coverage, edge, mean, clipped, crushed, contrast) = (rc.coverage, rc.edge, rc.mean, rc.clipped, rc.crushed, rc.contrast);
        if coverage < 0.02 {
            issue("Almost nothing is in frame — the camera may be pointing away or too far.", Some("viewFit -all"));
        } else if coverage < 0.08 && edge < 0.5 {
            issue("The subject is small in frame; move the camera closer or frame it.", Some("viewFit -all"));
        }
        if edge > 0.25 && edge < 0.95 && coverage < 0.9 {
            issue("Objects are cut off at the frame edge.", None);
        }
        if coverage >= 0.02 {
            if mean < 0.18 {
                issue("The image is underexposed (too dark).", Some(if default_light { "lightRig -preset studio" } else { "renderSettings -exp 1" }));
            } else if mean > 0.82 || clipped > 0.15 {
                issue("Highlights are blown out (overexposed).", Some("renderSettings -exp -1"));
            }
            if crushed > 0.35 {
                issue("Large areas are pure black — add fill light or a sky.", Some("renderSettings -env 0.4"));
            }
            if contrast < 0.05 && mean > 0.15 {
                issue("Lighting is flat (very low contrast); a key + rim setup would add shape.", Some("lightRig -preset studio"));
            }
        }
        if default_light {
            issue("No scene lights: the render uses the default light.", Some("lightRig -preset studio"));
        }
        let unshaded_msg = format!("{unshaded} mesh(es) have no material (default grey).");
        if unshaded > 0 {
            issue(&unshaded_msg, None);
        }
        for (t, f) in found {
            rc.issues.push(t);
            if let Some(f) = f
                && !rc.fixes.iter().any(|x| x == f)
            {
                rc.fixes.push(f.to_string());
            }
        }
        // Composite over a neutral backdrop for display and for the AI (opaque PNG).
        let rgba: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| {
                let a = p[3] as f32 / 255.0;
                let bg = 34.0;
                [(p[0] as f32 * a + bg * (1.0 - a)) as u8, (p[1] as f32 * a + bg * (1.0 - a)) as u8, (p[2] as f32 * a + bg * (1.0 - a)) as u8, 255]
            })
            .collect();
        let img = image::RgbaImage::from_raw(w, h, rgba.clone()).ok_or("image")?;
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        rc.png = png.into_inner();
        rc.rgba = rgba;
        // Show it in the Render View too.
        self.render.job = Some(job);
        Ok(rc)
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn light_rig_presets_replace_and_undo() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 20 -h 20 -n ground; polySphere -r 1 -n ball; move 0 1 0 ball").unwrap();
        for p in LIGHT_PRESETS {
            let r = execute(&mut app, &format!("lightRig -preset {}", p.0)).unwrap();
            assert!(r.contains(p.0), "{r}");
            assert_eq!(app.scene.nodes.iter().filter(|n| n.name == "lightRig").count(), 1, "rig replaced");
        }
        let lights = app.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Light(_))).count();
        assert_eq!(lights, 2, "dramatic = key + rim");
        // Key light aims at the ball (not the big ground plane).
        let key = app.scene.find_by_name("key_light").unwrap();
        let m = app.scene.world_matrix(key);
        let fwd = m.transform_vector3(Vec3::NEG_Z).normalize();
        let to_ball = (Vec3::new(0.0, 1.0, 0.0) - m.transform_point3(Vec3::ZERO)).normalize();
        assert!(fwd.dot(to_ball) > 0.99);
        execute(&mut app, "undo").unwrap();
        assert!(app.scene.find_by_name("rim_light").is_none(), "undo restores the previous (product) rig");
        assert!(app.scene.find_by_name("top_light").is_some());
    }

    #[test]
    fn material_presets() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -n a; polySphere -n b; select a b; materialPreset -p gold").unwrap();
        let ma = app.scene.node(app.scene.find_by_name("a").unwrap()).unwrap().material.unwrap();
        let mb = app.scene.node(app.scene.find_by_name("b").unwrap()).unwrap().material.unwrap();
        assert_eq!(ma, mb);
        let m = app.material(ma).unwrap();
        assert_eq!(m.name, "gold_mat");
        assert!(m.metalness > 0.99);
        execute(&mut app, "materialPreset -p glass b").unwrap();
        let mb = app.scene.node(app.scene.find_by_name("b").unwrap()).unwrap().material.unwrap();
        assert!(app.material(mb).unwrap().transparency > 0.8);
        execute(&mut app, "materialPreset -p plastic -rgb 0 0 1 a").unwrap();
        let ma = app.scene.node(app.scene.find_by_name("a").unwrap()).unwrap().material.unwrap();
        assert_eq!(app.material(ma).unwrap().color, [0.0, 0.0, 1.0]);
        assert!(execute(&mut app, "materialPreset -p unobtainium a").is_err());
        assert!(execute(&mut app, "materialPreset -list").unwrap().contains("chrome"));
    }

    #[test]
    fn rig_and_walk() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -w 0.6 -h 1.8 -d 0.3 -n body; move 0 0.9 0 body; select body").unwrap();
        let r = execute(&mut app, "rigAnimate -anim walk -s 1 -e 64 -travel on").unwrap();
        assert!(r.contains("Quick Rig") && r.contains("walk"), "{r}");
        let root = app.scene.find_by_name("root_ctrl").unwrap();
        let lf = app.scene.find_by_name("L_foot_ctrl").unwrap();
        let rz = |app: &App, t: f32| app.scene.node(root).unwrap().anim[&Channel::TranslateZ].evaluate(t).unwrap();
        assert!(rz(&app, 64.0) > rz(&app, 1.0) + 0.5, "travels forward");
        // During the left foot's stance its world Z stays (nearly) planted.
        let world_z = |app: &mut App, t: f32| {
            app.set_time(t);
            crate::rig::world_pos(&app.scene, lf).z
        };
        let a = world_z(&mut app, 2.0);
        let b = world_z(&mut app, 12.0);
        assert!((a - b).abs() < 0.03, "planted foot {a} {b}");
        // The foot lifts during swing.
        let mut max_y: f32 = 0.0;
        for f in 18..32 {
            app.set_time(f as f32);
            max_y = max_y.max(crate::rig::world_pos(&app.scene, lf).y);
        }
        assert!(max_y > 0.05, "{max_y}");
        // Re-animate (no new rig) and undo as one step.
        let r = execute(&mut app, "rigAnimate -anim wave -e 48").unwrap();
        assert!(!r.contains("Quick Rig"));
        execute(&mut app, "undo").unwrap();
        assert!(app.scene.node(root).unwrap().anim.contains_key(&Channel::TranslateZ), "walk restored");
    }

    #[test]
    fn render_check_flags_problems() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -n ball; camera -p 0 0 6 -aim 0 0 0 -n shot").unwrap();
        let rc = app.render_check(Some("shot"), 64, 36, 2).unwrap();
        assert!(rc.coverage > 0.1);
        assert!(rc.default_light && rc.unshaded == 1);
        assert!(rc.fixes.iter().any(|f| f.starts_with("lightRig")));
        assert!(rc.png.starts_with(&[0x89, b'P', b'N', b'G']));
        // Looking away: nothing in frame.
        execute(&mut app, "camera -p 0 0 6 -aim 0 0 20 -n away").unwrap();
        let rc = app.render_check(Some("away"), 32, 18, 1).unwrap();
        assert!(rc.coverage < 0.02 && rc.fixes.contains(&"viewFit -all".to_string()));
        let out = execute(&mut app, "renderCheck -cam shot -x 32 -y 18 -spp 1").unwrap();
        assert!(out.contains("Preview 32x18"), "{out}");
    }
}
