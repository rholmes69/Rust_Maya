//! Voice / text posing: "raise the left arm a bit and turn the head right".
//!
//! The `pose` MEL command moves a Quick Rig's controls (hand / foot IK controls, chest,
//! head and cog controls). On an FK-only skeleton it rotates the joints themselves, found by
//! fuzzy names (L_shoulder, leftArm, arm_l, …). `parse_pose` turns plain English into `pose`
//! commands so typed and spoken posing works without a language model.

use crate::app::App;
use crate::rig;
use crate::scene::{NodeId, Shape};
use glam::{Quat, Vec3};

pub const PARTS: [&str; 8] = ["arm", "elbow", "leg", "knee", "foot", "head", "chest", "hips"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Amount {
    Small,
    Normal,
    Large,
    Degrees(f32),
    Distance(f32),
}

impl Amount {
    fn parse(s: &str) -> Result<Amount, String> {
        Ok(match s {
            "small" | "s" | "bit" | "little" => Amount::Small,
            "normal" | "medium" | "m" => Amount::Normal,
            "large" | "l" | "lot" | "big" => Amount::Large,
            _ => {
                let lower = s.to_lowercase();
                if let Some(v) = lower.strip_suffix("cm") {
                    Amount::Distance(v.parse::<f32>().map_err(|_| format!("pose: bad amount '{s}'"))? / 100.0)
                } else if let Some(v) = lower.strip_suffix('m') {
                    Amount::Distance(v.parse().map_err(|_| format!("pose: bad amount '{s}'"))?)
                } else {
                    Amount::Degrees(lower.trim_end_matches("deg").trim_end_matches('°').parse().map_err(|_| format!("pose: bad amount '{s}'"))?)
                }
            }
        })
    }
    fn degrees(self) -> f32 {
        match self {
            Amount::Small => 10.0,
            Amount::Normal => 25.0,
            Amount::Large => 50.0,
            Amount::Degrees(d) => d,
            Amount::Distance(d) => d * 100.0,
        }
    }
    fn distance(self, h: f32) -> f32 {
        match self {
            Amount::Small => 0.03 * h,
            Amount::Normal => 0.08 * h,
            Amount::Large => 0.2 * h,
            Amount::Distance(d) => d,
            Amount::Degrees(d) => d * 0.01 * h,
        }
    }
}

fn find(app: &App, name: &str) -> Option<NodeId> {
    app.scene.find_by_name(name)
}

/// The rig's standing height (root control to the top of the head), for distance amounts.
fn rig_height(app: &App) -> f32 {
    let y = |n: &str| find(app, n).map(|id| rig::world_pos(&app.scene, id).y);
    match (y("head_end").or(y("head")), y("root_ctrl")) {
        (Some(top), Some(base)) if top > base => top - base,
        _ => y("hips").map(|h| h / 0.53).filter(|h| *h > 0.01).unwrap_or(1.8),
    }
}

/// Fuzzy joint lookup for FK rigs: `L_shoulder`, `shoulder_L`, `LeftShoulder`, `left_arm`, …
fn find_joint(app: &App, side: Option<&str>, keys: &[&str]) -> Option<NodeId> {
    let joints: Vec<(NodeId, String)> = app.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Joint { .. })).map(|n| (n.id, n.name.to_lowercase())).collect();
    let sided = |n: &str| -> Option<&'static str> {
        let n = n.trim_start_matches(|c: char| !c.is_alphanumeric());
        if n.starts_with("l_") || n.ends_with("_l") || n.starts_with("left") || n.contains("_left") || n.contains(":left") {
            Some("L")
        } else if n.starts_with("r_") || n.ends_with("_r") || n.starts_with("right") || n.contains("_right") || n.contains(":right") {
            Some("R")
        } else {
            None
        }
    };
    for k in keys {
        for (id, n) in &joints {
            if n.contains(k) && sided(n) == side && !n.contains("end") && !n.contains("twist") {
                if *k == "arm" && (n.contains("fore") || n.contains("lower")) {
                    continue;
                }
                if *k == "leg" && keys.contains(&"knee") && n.contains("up") {
                    continue;
                }
                return Some(*id);
            }
        }
    }
    None
}

fn first_child_joint(app: &App, id: NodeId) -> Option<NodeId> {
    app.scene.nodes.iter().find(|n| n.parent == Some(id) && matches!(n.shape, Shape::Joint { .. })).map(|n| n.id)
}

/// Rotate a node about a world axis through its own pivot.
fn rotate_world(app: &mut App, id: NodeId, axis: Vec3, deg: f32) {
    let q = Quat::from_axis_angle(axis.normalize_or_zero(), deg.to_radians());
    let r = rig::world_rotation(&app.scene, id);
    rig::set_world_rotation(&mut app.scene, id, (q * r).normalize());
}

fn shift_world(app: &mut App, id: NodeId, delta: Vec3) {
    let p = rig::world_pos(&app.scene, id);
    rig::set_world_position(&mut app.scene, id, p + delta);
}

/// Axis that rotates direction `d` toward `toward` (right-hand rule), with a fallback when they're parallel.
fn axis_toward(d: Vec3, toward: Vec3, fallback: Vec3) -> Vec3 {
    let a = d.cross(toward);
    if a.length() > 1e-4 { a.normalize() } else { fallback }
}

/// New root→end distance after changing the middle joint's interior angle by `delta_deg`.
fn bent_length(a: f32, b: f32, d: f32, delta_deg: f32) -> f32 {
    let cos_phi = ((a * a + b * b - d * d) / (2.0 * a * b)).clamp(-1.0, 1.0);
    let phi = cos_phi.acos().to_degrees();
    let phi2 = (phi + delta_deg).clamp(25.0, 179.5).to_radians();
    (a * a + b * b - 2.0 * a * b * phi2.cos()).max(1e-6).sqrt()
}

pub struct PoseRequest {
    pub part: String,
    pub sides: Vec<&'static str>,
    pub action: String,
    pub dir: Option<String>,
    amount: Amount,
    pub key: bool,
}

impl App {
    /// Apply one pose request; returns the nodes it moved.
    fn pose_one(&mut self, req: &PoseRequest, side: &str) -> Result<Vec<NodeId>, String> {
        let h = rig_height(self);
        let deg = req.amount.degrees();
        let dist = req.amount.distance(h);
        let sgn = if side == "R" { -1.0 } else { 1.0 };
        let ctrl = |app: &App, n: &str| find(app, &format!("{side}_{n}_ctrl"));
        let action = req.action.as_str();
        let dir = req.dir.as_deref();
        let bad = |what: &str| Err(format!("pose: can't '{action}' the {what} (try {})", match what {
            "arm" => "raise, lower, forward, back",
            "elbow" | "knee" => "bend, straighten",
            "leg" => "raise, lower, forward, back, out, in",
            "foot" => "lift, lower, step -dir forward|back|left|right",
            "head" => "turn, nod/tilt -dir up|down|left|right",
            "chest" => "lean -dir forward|back|left|right, twist -dir left|right",
            _ => "crouch, stand, turn -dir left|right, shift -dir left|right",
        }));
        let mut moved = Vec::new();
        match req.part.as_str() {
            "arm" | "leg" => {
                let arm = req.part == "arm";
                let (root_k, end_k, ctrl_n): (&[&str], &[&str], &str) =
                    if arm { (&["shoulder", "upperarm", "arm"], &["wrist", "hand"], "hand") } else { (&["hip", "upleg", "thigh", "leg"], &["ankle", "foot"], "foot") };
                let root = find(self, &format!("{side}_{}", root_k[0])).or_else(|| find_joint(self, Some(side), root_k)).ok_or_else(|| format!("pose: no {} {} joint", if side == "L" { "left" } else { "right" }, root_k[0]))?;
                let rp = rig::world_pos(&self.scene, root);
                let (toward, neg) = match (action, dir) {
                    ("raise" | "lift" | "up", _) if arm => (Vec3::Y, false),
                    ("raise" | "lift" | "up" | "kick", _) => (Vec3::Z, false),
                    ("lower" | "down" | "drop", _) => (if arm { Vec3::Y } else { Vec3::Z }, true),
                    ("forward" | "reach", _) | ("swing" | "move", Some("forward")) => (Vec3::Z, false),
                    ("back" | "backward", _) | ("swing" | "move", Some("back")) => (Vec3::Z, true),
                    ("out" | "spread", _) => (Vec3::X * sgn, false),
                    ("in" | "cross", _) => (Vec3::X * sgn, true),
                    _ => return bad(&req.part),
                };
                let angle = if neg { -deg } else { deg };
                if let Some(c) = ctrl(self, ctrl_n) {
                    // IK: swing the hand / foot control around the shoulder / hip.
                    let end = find(self, &format!("{side}_{}", end_k[0])).unwrap_or(c);
                    let ep = rig::world_pos(&self.scene, end);
                    let d = ep - rp;
                    let axis = axis_toward(d, toward, Vec3::X * sgn);
                    let q = Quat::from_axis_angle(axis, angle.to_radians());
                    let delta = rp + q * d - ep;
                    shift_world(self, c, delta);
                    if arm {
                        rotate_world(self, c, axis, angle);
                    }
                    moved.push(c);
                } else {
                    let child = first_child_joint(self, root).ok_or("pose: the limb joint has no child")?;
                    let d = rig::world_pos(&self.scene, child) - rp;
                    let axis = axis_toward(d, toward, Vec3::X * sgn);
                    rotate_world(self, root, axis, angle);
                    moved.push(root);
                }
            }
            "elbow" | "knee" => {
                let elbow = req.part == "elbow";
                let delta = match action {
                    "bend" | "flex" | "fold" | "lift" | "raise" => -deg,
                    "straighten" | "extend" | "stretch" | "lower" => deg,
                    _ => return bad(&req.part),
                };
                let (root_k, mid_k, end_k): (&[&str], &[&str], &[&str]) = if elbow {
                    (&["shoulder", "upperarm", "arm"], &["elbow", "forearm", "lowerarm"], &["wrist", "hand"])
                } else {
                    (&["hip", "upleg", "thigh"], &["knee", "lowerleg", "shin", "calf", "leg"], &["ankle", "foot"])
                };
                let get = |app: &App, k: &[&str]| find(app, &format!("{side}_{}", k[0])).or_else(|| find_joint(app, Some(side), k));
                let (r, m, e) = match (get(self, root_k), get(self, mid_k), get(self, end_k)) {
                    (Some(r), Some(m), Some(e)) => (r, m, e),
                    _ => return Err(format!("pose: no {} chain found", req.part)),
                };
                let (rp, mp, ep) = (rig::world_pos(&self.scene, r), rig::world_pos(&self.scene, m), rig::world_pos(&self.scene, e));
                if let Some(c) = ctrl(self, if elbow { "hand" } else { "foot" }) {
                    // IK: pull the control toward the root so the solver bends the middle joint.
                    let d = ep - rp;
                    let nl = bent_length(mp.distance(rp), ep.distance(mp), d.length(), delta);
                    let target = rp + d.normalize_or_zero() * nl;
                    // Knees bend with the foot lifting forward; elbows fold the hand toward the chest.
                    shift_world(self, c, target - ep);
                    moved.push(c);
                } else {
                    let (u, l) = (mp - rp, ep - mp);
                    let fwd = if elbow { Vec3::Z } else { -Vec3::Z };
                    let mut axis = u.cross(l);
                    if axis.length() < 1e-4 * u.length() * l.length() {
                        axis = axis_toward(l, fwd, Vec3::X);
                    }
                    // Rotating by -delta increases the bend when delta < 0.
                    rotate_world(self, m, axis.normalize(), -delta);
                    moved.push(m);
                }
            }
            "foot" => {
                let c = ctrl(self, "foot");
                let target = c.or_else(|| find(self, &format!("{side}_ankle")).or_else(|| find_joint(self, Some(side), &["ankle", "foot"])));
                let id = target.ok_or("pose: no foot control or ankle joint")?;
                let delta = match (action, dir) {
                    ("lift" | "raise" | "up", _) => Vec3::Y * dist,
                    ("lower" | "down" | "plant", _) => -Vec3::Y * dist,
                    ("step", Some("back")) | ("back" | "backward", _) => -Vec3::Z * dist * 2.5,
                    ("step", Some("left")) => Vec3::X * dist * 1.5,
                    ("step", Some("right")) => -Vec3::X * dist * 1.5,
                    ("step" | "forward", _) => Vec3::Z * dist * 2.5,
                    _ => return bad("foot"),
                };
                if c.is_some() {
                    shift_world(self, id, delta);
                    moved.push(id);
                } else {
                    // FK: approximate by swinging the hip.
                    let hip = find_joint(self, Some(side), &["hip", "upleg", "thigh", "leg"]).ok_or("pose: no hip joint")?;
                    let ang = (delta.length() / (0.45 * h)).atan().to_degrees();
                    let fwd = if delta.z.abs() > delta.x.abs() { Vec3::Z * delta.z.signum() } else { Vec3::Z };
                    rotate_world(self, hip, axis_toward(-Vec3::Y, fwd, Vec3::X), ang);
                    moved.push(hip);
                }
            }
            "head" | "chest" => {
                let head = req.part == "head";
                let id = find(self, if head { "head_ctrl" } else { "chest_ctrl" })
                    .or_else(|| find_joint(self, None, if head { &["head", "neck"] } else { &["chest", "spine2", "spine1", "spine"] }))
                    .ok_or_else(|| format!("pose: no {} control or joint", req.part))?;
                let (axis, ang) = match (action, dir) {
                    ("turn" | "look" | "twist" | "rotate", Some("left")) => (Vec3::Y, deg),
                    ("turn" | "look" | "twist" | "rotate", Some("right")) => (Vec3::Y, -deg),
                    ("nod" | "look" | "tilt" | "lean" | "bow", Some("up" | "back")) | ("raise" | "up", _) => (Vec3::X, -deg),
                    ("nod" | "look" | "tilt" | "lean" | "bow" | "bend", Some("down" | "forward")) | ("nod" | "bow" | "lower" | "down" | "bend", None) => (Vec3::X, deg),
                    ("tilt" | "lean" | "bend", Some("left")) => (Vec3::Z, -deg),
                    ("tilt" | "lean" | "bend", Some("right")) => (Vec3::Z, deg),
                    ("lean" | "tilt", None) => (Vec3::X, deg),
                    ("back", _) => (Vec3::X, -deg),
                    ("forward", _) => (Vec3::X, deg),
                    _ => return bad(&req.part),
                };
                rotate_world(self, id, axis, ang);
                moved.push(id);
            }
            "hips" => {
                let id = find(self, "cog_ctrl").or_else(|| find_joint(self, None, &["hips", "pelvis", "root"])).ok_or("pose: no cog control or hips joint")?;
                match (action, dir) {
                    ("crouch" | "squat" | "duck" | "lower" | "down" | "sit", _) => shift_world(self, id, -Vec3::Y * dist),
                    ("stand" | "rise" | "raise" | "up", _) => {
                        // Raise / lower the hips until the legs are as straight as in the Quick Rig rest pose.
                        let dy = stand_offset(self).ok_or("pose: stand needs hip, knee and ankle joints")?;
                        shift_world(self, id, Vec3::Y * dy);
                    }
                    ("turn" | "twist" | "rotate", Some("left")) => rotate_world(self, id, Vec3::Y, deg),
                    ("turn" | "twist" | "rotate", Some("right")) => rotate_world(self, id, Vec3::Y, -deg),
                    ("shift" | "move" | "sway", Some("left")) => shift_world(self, id, Vec3::X * dist * 0.5),
                    ("shift" | "move" | "sway", Some("right")) => shift_world(self, id, -Vec3::X * dist * 0.5),
                    ("shift" | "move", Some("forward")) | ("forward", _) => shift_world(self, id, Vec3::Z * dist * 0.5),
                    ("shift" | "move", Some("back")) | ("back", _) => shift_world(self, id, -Vec3::Z * dist * 0.5),
                    _ => return bad("hips"),
                }
                moved.push(id);
            }
            p => return Err(format!("pose: unknown part '{p}' (use {})", PARTS.join(", "))),
        }
        Ok(moved)
    }

    pub fn pose(&mut self, req: &PoseRequest) -> Result<String, String> {
        rig::evaluate(&mut self.scene);
        self.checkpoint();
        let mut moved = Vec::new();
        for side in &req.sides {
            // Re-evaluate between sides so world positions reflect the first side's change.
            moved.extend(self.pose_one(req, side)?);
            rig::evaluate(&mut self.scene);
        }
        moved.dedup();
        let names: Vec<String> = moved.iter().filter_map(|id| self.scene.node(*id).map(|n| n.name.clone())).collect();
        // A control that is already animated would snap back on the next frame change: key it.
        let animated = moved.iter().any(|id| self.scene.node(*id).is_some_and(|n| !n.anim.is_empty()));
        let key = req.key || animated;
        if key {
            let prev = std::mem::replace(&mut self.sel, moved.clone());
            let mut chans = crate::scene::Channel::translate().to_vec();
            chans.extend(crate::scene::Channel::rotate());
            self.key_selected(&chans);
            self.sel = prev;
        } else {
            self.auto_key_touch(&moved);
        }
        rig::evaluate(&mut self.scene);
        let note = match (req.key, key) {
            (true, _) => format!(" (keyed at frame {})", self.scene.time),
            (false, true) => format!(" (keyed at frame {} because the rig is animated)", self.scene.time),
            _ => String::new(),
        };
        Ok(format!("posed {}{note}", names.join(", ")))
    }
}

/// Hip→ankle distance over thigh + shin length in the Quick Rig rest pose (knees very slightly bent).
const STAND_RATIO: f32 = 0.99383;

/// Vertical hip offset that makes the legs (average of both sides) as straight as at rest.
fn stand_offset(app: &App) -> Option<f32> {
    let mut sum = 0.0;
    let mut n = 0.0;
    for side in ["L", "R"] {
        let j = |k: &[&str]| find(app, &format!("{side}_{}", k[0])).or_else(|| find_joint(app, Some(side), k));
        let (Some(h), Some(k), Some(a)) = (j(&["hip", "upleg", "thigh"]), j(&["knee", "lowerleg", "shin", "calf", "leg"]), j(&["ankle", "foot"])) else { continue };
        let (hp, kp, ap) = (rig::world_pos(&app.scene, h), rig::world_pos(&app.scene, k), rig::world_pos(&app.scene, a));
        let len = (hp.distance(kp) + kp.distance(ap)) * STAND_RATIO;
        let d = hp - ap;
        let horiz = (d.x * d.x + d.z * d.z).min(len * len);
        sum += ap.y + (len * len - horiz).sqrt() - hp.y;
        n += 1.0;
    }
    (n > 0.0).then(|| sum / n)
}

pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    if cmd != "pose" {
        return None;
    }
    Some((|| {
        let mut part = None;
        let mut side = "both".to_string();
        let mut action = None;
        let mut dir = None;
        let mut amount = Amount::Normal;
        let mut key = false;
        let mut i = 0;
        while i < rest.len() {
            let flag = rest[i].as_str();
            let val = rest.get(i + 1).cloned();
            match flag {
                "-part" | "-p" => part = val,
                "-side" | "-s" => side = val.unwrap_or_default(),
                "-action" | "-a" => action = val,
                "-dir" | "-d" => dir = val,
                "-amount" | "-amt" => amount = Amount::parse(val.as_deref().unwrap_or("normal"))?,
                "-key" | "-k" => {
                    key = true;
                    i += 1;
                    continue;
                }
                f => return Err(format!("pose: unknown flag '{f}'. Usage: pose -part arm|elbow|leg|knee|foot|head|chest|hips [-side L|R|both] -action … [-dir left|right|forward|back|up|down] [-amount small|large|<deg>|<n>cm] [-key]")),
            }
            i += 2;
        }
        let part = part.ok_or("pose: -part is required")?;
        let action = action.ok_or("pose: -action is required")?;
        let sides: Vec<&'static str> = if matches!(part.as_str(), "head" | "chest" | "hips") {
            vec!["C"]
        } else {
            match side.to_lowercase().as_str() {
                "l" | "left" => vec!["L"],
                "r" | "right" => vec!["R"],
                _ => vec!["L", "R"],
            }
        };
        app.pose(&PoseRequest { part, sides, action, dir, amount, key })
    })())
}

// ---------------------------------------------------------------------------------------------
// Natural-language parsing.

fn part_of(w: &str) -> Option<(&'static str, bool)> {
    // (part, plural)
    Some(match w {
        "arm" | "hand" => ("arm", false),
        "arms" | "hands" => ("arm", true),
        "elbow" | "forearm" => ("elbow", false),
        "elbows" | "forearms" => ("elbow", true),
        "leg" | "thigh" => ("leg", false),
        "legs" | "thighs" => ("leg", true),
        "knee" => ("knee", false),
        "knees" => ("knee", true),
        "foot" => ("foot", false),
        "feet" => ("foot", true),
        "head" | "face" | "neck" => ("head", false),
        "chest" | "torso" | "spine" | "body" | "upper" | "shoulders" => ("chest", false),
        "hips" | "hip" | "waist" | "pelvis" | "cog" => ("hips", false),
        _ => return None,
    })
}

fn action_of(w: &str) -> Option<&'static str> {
    Some(match w {
        "raise" | "raises" | "lift" | "lifts" | "up" => "raise",
        "lower" | "lowers" | "drop" | "down" => "lower",
        "forward" | "forwards" | "front" | "reach" => "forward",
        "backward" | "backwards" | "behind" => "back",
        "bend" | "bends" | "flex" | "fold" => "bend",
        "straighten" | "straight" | "extend" | "stretch" => "straighten",
        "turn" | "turns" | "rotate" | "look" | "looks" => "turn",
        "tilt" | "tilts" | "cock" => "tilt",
        "nod" | "nods" | "bow" => "nod",
        "lean" | "leans" => "lean",
        "twist" | "twists" => "twist",
        "step" | "steps" => "step",
        "crouch" | "crouches" | "squat" | "duck" | "kneel" => "crouch",
        "stand" | "stands" => "stand",
        "spread" | "out" | "outward" | "sideways" => "out",
        "kick" | "kicks" => "kick",
        "shift" | "sway" => "shift",
        _ => return None,
    })
}

/// "raise the left arm a bit and turn the head right" →
/// ["pose -part arm -side L -action raise -amount small", "pose -part head -action turn -dir right"].
pub fn parse_pose(text: &str) -> Option<Vec<String>> {
    let t = text.to_lowercase().replace([',', '!', '?', ';'], " , ").replace('.', " , ");
    let t = t.replace(" and then ", " , ").replace(" then ", " , ").replace(" and ", " , ").replace(" while ", " , ");
    let mut out = Vec::new();
    let mut key = false;
    let mut last_part: Option<&str> = None;
    // Split further where a new verb starts after a clause already has a verb and a body part
    // ("raise both arms a lot bend the right elbow" → two clauses).
    let mut clauses: Vec<String> = Vec::new();
    for c in t.split(',') {
        let mut cur: Vec<&str> = Vec::new();
        let (mut has_verb, mut has_part) = (false, false);
        for w in c.split_whitespace() {
            let verb = matches!(w, "raise" | "lift" | "lower" | "drop" | "bend" | "straighten" | "extend" | "turn" | "rotate" | "look" | "tilt" | "nod" | "lean" | "twist" | "step" | "crouch" | "squat" | "stand" | "kick" | "shift");
            if verb && has_verb && has_part {
                clauses.push(cur.join(" "));
                cur.clear();
                has_part = false;
            }
            has_verb |= verb;
            has_part |= part_of(w).is_some();
            cur.push(w);
        }
        clauses.push(cur.join(" "));
    }
    for clause in &clauses {
        let words: Vec<&str> = clause.split_whitespace().filter(|w| !matches!(*w, "the" | "his" | "her" | "their" | "its" | "my" | "please" | "a" | "an" | "to" | "slightly" | "both" | "with")).collect();
        if words.is_empty() {
            continue;
        }
        if words.iter().any(|w| matches!(*w, "key" | "keyframe" | "keyed")) {
            key = true;
            if words.iter().all(|w| matches!(*w, "key" | "keyframe" | "keyed" | "it" | "set" | "that" | "this" | "pose")) {
                continue;
            }
        }
        let mut part = None;
        let mut plural = clause.contains("both");
        let mut side = None;
        let mut action = None;
        let mut dir: Option<&str> = None;
        let mut amount = None;
        for (i, w) in words.iter().enumerate() {
            let next = words.get(i + 1).copied().unwrap_or("");
            if let Some((p, pl)) = part_of(w) {
                if part.is_none() || (*w != "up" && part == Some("chest") && p != "chest") {
                    part = Some(p);
                    plural |= pl;
                }
                continue;
            }
            match *w {
                "left" | "right" => {
                    let s = if *w == "left" { "L" } else { "R" };
                    // A side word right before a limb is the side; otherwise it's a direction.
                    if part_of(next).is_some() || (next == "side" && part.is_none()) {
                        side = Some(s);
                    } else {
                        dir = Some(*w);
                    }
                    continue;
                }
                "l" | "r" if part_of(next).is_some() => {
                    side = Some(if *w == "l" { "L" } else { "R" });
                    continue;
                }
                "forward" | "forwards" | "front" if action.is_some() => {
                    dir = Some("forward");
                    continue;
                }
                "back" | "backward" | "backwards" | "behind" => {
                    if action.is_some() { dir = Some("back") } else { action = Some("back") }
                    continue;
                }
                "up" | "upward" | "down" | "downward" if action.is_some() && action != Some("raise") && action != Some("lower") => {
                    dir = Some(if w.starts_with("up") { "up" } else { "down" });
                    continue;
                }
                "bit" | "little" | "tad" | "touch" | "slightly" => {
                    amount = Some("small".to_string());
                    continue;
                }
                "lot" | "way" | "all" | "high" | "far" | "fully" => {
                    amount = Some("large".to_string());
                    continue;
                }
                _ => {}
            }
            if let Some(num) = w.strip_suffix("°").or(Some(w)).and_then(|n| n.parse::<f32>().ok()) {
                amount = Some(match next {
                    "cm" | "centimeters" | "centimetres" => format!("{num}cm"),
                    "m" | "meters" | "metres" | "meter" => format!("{num}m"),
                    _ => format!("{num}"),
                });
                continue;
            }
            if let Some(n) = w.strip_suffix("cm").and_then(|n| n.parse::<f32>().ok()) {
                amount = Some(format!("{n}cm"));
                continue;
            }
            if let Some(a) = action_of(w) {
                if action.is_none() || (action == Some("raise") && a != "raise") && !matches!(a, "forward" | "lower") {
                    action = Some(a);
                } else if action == Some("turn") && a == "raise" {
                    dir = Some("up");
                } else if action == Some("turn") && a == "lower" {
                    dir = Some("down");
                }
            }
        }
        // "look up/down/left" etc. imply the head; "crouch"/"squat"/"stand" imply the hips;
        // "lean"/"twist" imply the chest.
        let part = part.or(match action {
            Some("turn" | "nod" | "tilt") => Some("head"),
            Some("crouch" | "stand" | "shift") => Some("hips"),
            Some("lean" | "twist") => Some("chest"),
            Some("step") => Some("foot"),
            Some("kick") => Some("leg"),
            _ => last_part,
        })?;
        let mut action = action?;
        last_part = Some(part);
        // Normalise per part.
        match (part, action) {
            ("head" | "chest", "raise") => {
                action = "nod";
                dir = Some("up");
            }
            ("head" | "chest", "lower") => {
                action = "nod";
                dir = Some("down");
            }
            ("head", "turn") if matches!(dir, Some("up" | "down")) => action = "nod",
            ("head" | "chest", "forward") => {
                action = "lean";
                dir = Some("forward");
            }
            ("head" | "chest", "back") => {
                action = "lean";
                dir = Some("back");
            }
            ("chest", "turn") => action = "twist",
            ("chest", "bend") => {
                action = "lean";
                dir = dir.or(Some("forward"));
            }
            ("foot", "forward") => action = "step",
            ("foot", "step") if dir.is_none() => dir = Some("forward"),
            ("hips", "lower") => action = "crouch",
            ("hips", "raise") => action = "stand",
            ("knee" | "elbow", "raise") => action = "bend",
            ("knee" | "elbow", "lower") => action = "straighten",
            ("leg", "kick") => action = "raise",
            ("arm" | "leg", "turn") => return None,
            _ => {}
        }
        if matches!(part, "head" | "hips") && matches!(action, "turn" | "tilt" | "twist" | "shift") && dir.is_none() {
            return None;
        }
        let mut cmd = format!("pose -part {part}");
        if matches!(part, "arm" | "elbow" | "leg" | "knee" | "foot") {
            let s = if plural { "both" } else { side.unwrap_or(if dir == Some("left") { "L" } else if dir == Some("right") { "R" } else { "both" }) };
            cmd.push_str(&format!(" -side {s}"));
        }
        cmd.push_str(&format!(" -action {action}"));
        if let Some(d) = dir
            && !(matches!(part, "arm" | "elbow" | "leg" | "knee") && matches!(d, "left" | "right"))
        {
            cmd.push_str(&format!(" -dir {d}"));
        }
        if let Some(a) = amount {
            cmd.push_str(&format!(" -amount {a}"));
        }
        out.push(cmd);
    }
    if out.is_empty() {
        return None;
    }
    if key {
        for c in out.iter_mut() {
            c.push_str(" -key");
        }
    }
    Some(out)
}

/// True if the text reads like a posing instruction (so typed / spoken text can skip the model).
pub fn is_pose_request(text: &str) -> bool {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has_part = words.iter().any(|w| part_of(w).is_some());
    let has_pose_verb = words.iter().any(|w| {
        matches!(*w, "raise" | "lift" | "lower" | "bend" | "straighten" | "turn" | "tilt" | "nod" | "lean" | "twist" | "step" | "crouch" | "squat" | "kick" | "look")
    });
    (has_part && has_pose_verb && !words.iter().any(|w| matches!(*w, "create" | "make" | "add" | "build" | "model" | "rig" | "weights" | "skin" | "joint" | "joints")))
        || matches!(words.as_slice(), ["crouch"] | ["squat"] | ["stand", "up"] | ["look", _] | ["look", _, _])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    fn pos(app: &App, n: &str) -> Vec3 {
        rig::world_pos(&app.scene, app.scene.find_by_name(n).unwrap())
    }

    #[test]
    fn parses_phrases() {
        let p = |s: &str| parse_pose(s).unwrap_or_default();
        assert_eq!(p("raise the left arm a bit and turn the head right"), vec!["pose -part arm -side L -action raise -amount small", "pose -part head -action turn -dir right"]);
        assert_eq!(p("Raise both arms"), vec!["pose -part arm -side both -action raise"]);
        assert_eq!(
            p("raise both arms a lot bend the right elbow 60 degrees and turn the head left"),
            vec!["pose -part arm -side both -action raise -amount large", "pose -part elbow -side R -action bend -amount 60", "pose -part head -action turn -dir left"]
        );
        assert_eq!(p("bend the right knee 40 degrees"), vec!["pose -part knee -side R -action bend -amount 40"]);
        assert_eq!(p("look up"), vec!["pose -part head -action nod -dir up"]);
        assert_eq!(p("lean forward a lot"), vec!["pose -part chest -action lean -dir forward -amount large"]);
        assert_eq!(p("crouch 10 cm"), vec!["pose -part hips -action crouch -amount 10cm"]);
        assert_eq!(p("step forward with the left foot and key it"), vec!["pose -part foot -side L -action step -dir forward -key"]);
        assert_eq!(p("tilt head left"), vec!["pose -part head -action tilt -dir left"]);
        assert_eq!(p("lift the right leg"), vec!["pose -part leg -side R -action raise"]);
        assert_eq!(p("lower the left hand, then bend the elbow"), vec!["pose -part arm -side L -action lower", "pose -part elbow -side both -action bend"]);
        assert!(parse_pose("make a sphere").is_none());
        assert!(is_pose_request("raise the left arm") && is_pose_request("look left") && !is_pose_request("create a joint chain for the arm"));
    }

    #[test]
    fn poses_quick_rig() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "character -style human -n hero; select hero; quickRig").unwrap();
        crate::rig::evaluate(&mut app.scene);
        let w0 = pos(&app, "L_wrist");
        let sh = pos(&app, "L_shoulder");
        execute(&mut app, "pose -part arm -side L -action raise -amount 30").unwrap();
        let w1 = pos(&app, "L_wrist");
        assert!(w1.y > w0.y + 0.05, "{w0} → {w1}");
        // Arm length is preserved (rotation about the shoulder).
        assert!(((w1 - sh).length() - (w0 - sh).length()).abs() < 0.02);
        let r0 = pos(&app, "R_wrist");
        execute(&mut app, "pose -part arm -side R -action forward").unwrap();
        assert!(pos(&app, "R_wrist").z > r0.z + 0.05);
        // Elbow bend shortens shoulder→wrist.
        let d0 = (pos(&app, "R_wrist") - pos(&app, "R_shoulder")).length();
        execute(&mut app, "pose -part elbow -side R -action bend -amount 45").unwrap();
        assert!((pos(&app, "R_wrist") - pos(&app, "R_shoulder")).length() < d0 - 0.03);
        // Head turn rotates the head joint's aim (head_end) toward +X (character's left).
        let he0 = pos(&app, "head_end") - pos(&app, "head");
        execute(&mut app, "pose -part head -action nod -dir down -amount 30").unwrap();
        let he1 = pos(&app, "head_end") - pos(&app, "head");
        assert!(he1.z > he0.z + 0.02, "{he0} {he1}");
        // Crouch drops the hips, feet stay planted; stand restores.
        let (h0, a0) = (pos(&app, "hips"), pos(&app, "L_ankle"));
        execute(&mut app, "pose -part hips -action crouch -amount large").unwrap();
        assert!(pos(&app, "hips").y < h0.y - 0.1);
        assert!(pos(&app, "L_ankle").distance(a0) < 0.03, "feet stay on the ground");
        execute(&mut app, "pose -part hips -action stand").unwrap();
        assert!((pos(&app, "hips").y - h0.y).abs() < 2e-3, "{} vs {}", pos(&app, "hips").y, h0.y);
        // Knee bend lifts the foot via IK; -key sets keys.
        let f0 = pos(&app, "L_ankle");
        execute(&mut app, "pose -part foot -side L -action lift -key").unwrap();
        assert!(pos(&app, "L_ankle").y > f0.y + 0.05);
        let fc = app.scene.find_by_name("L_foot_ctrl").unwrap();
        assert!(app.scene.node(fc).is_some_and(|n| !n.anim.is_empty()), "foot control keyed");
        // Natural language end-to-end.
        for c in parse_pose("raise both arms a lot").unwrap() {
            execute(&mut app, &c).unwrap();
        }
        assert!(execute(&mut app, "pose -part arm -action wave").is_err());
        // On an animated rig the pose is keyed so it survives a frame change.
        execute(&mut app, "rigAnimate -anim idle -s 1 -e 24 hero; currentTime 10").unwrap();
        let msg = execute(&mut app, "pose -part arm -side L -action raise -amount large").unwrap();
        assert!(msg.contains("keyed at frame 10"), "{msg}");
        let w = pos(&app, "L_wrist");
        execute(&mut app, "currentTime 11; currentTime 10").unwrap();
        assert!(pos(&app, "L_wrist").distance(w) < 1e-3);
    }

    #[test]
    fn poses_fk_skeleton() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "joint -p 0 1.5 0 -n spine; joint -p 0.2 1.5 0 -n LeftArm; joint -p 0.5 1.5 0 -n LeftForeArm; joint -p 0.8 1.5 0 -n LeftHand").unwrap();
        let h0 = pos(&app, "LeftHand");
        execute(&mut app, "pose -part arm -side L -action raise -amount 90").unwrap();
        crate::rig::evaluate(&mut app.scene);
        let h1 = pos(&app, "LeftHand");
        assert!((h1 - Vec3::new(0.2, 2.1, 0.0)).length() < 0.02, "{h0} → {h1}");
        execute(&mut app, "pose -part elbow -side L -action bend -amount 90").unwrap();
        crate::rig::evaluate(&mut app.scene);
        let e = pos(&app, "LeftForeArm");
        let h2 = pos(&app, "LeftHand");
        assert!(((h2 - e).length() - 0.3).abs() < 0.01 && (h2 - e).normalize().dot(Vec3::Y).abs() < 0.1, "{e} {h2}");
    }
}
