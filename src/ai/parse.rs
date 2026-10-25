//! Parsing of model output (streamed `{"reply": "...", "mel": ["...", ...]}` objects) and an
//! offline English → MEL parser for instant, model-free handling of simple commands.

use serde_json::Value;

/// Incremental parser: yields the reply text as it grows and each MEL statement in the `mel`
/// array as soon as its closing quote arrives, so commands run while the model is still typing.
#[derive(Default)]
pub struct MelStreamParser {
    pub buf: String,
    scanned: usize,
    arr_start: Option<usize>,
    in_string: bool,
    escape: bool,
    str_start: usize,
    done: bool,
}

impl MelStreamParser {
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        if self.arr_start.is_none() {
            let body = strip_think(&self.buf);
            let offset = self.buf.len() - body.len();
            if let Some(k) = find_key(body, "mel") {
                if let Some(b) = body[k..].find('[') {
                    let s = offset + k + b + 1;
                    self.arr_start = Some(s);
                    self.scanned = s;
                }
            }
        }
        if self.arr_start.is_none() {
            return out;
        }
        let bytes = self.buf.as_bytes();
        let mut i = self.scanned;
        while i < bytes.len() {
            let c = bytes[i];
            if self.in_string {
                if self.escape {
                    self.escape = false;
                } else if c == b'\\' {
                    self.escape = true;
                } else if c == b'"' {
                    self.in_string = false;
                    if let Ok(s) = serde_json::from_str::<String>(&self.buf[self.str_start..=i]) {
                        let s = s.trim().to_string();
                        if !s.is_empty() {
                            out.push(s);
                        }
                    }
                }
            } else if c == b'"' {
                self.in_string = true;
                self.str_start = i;
            } else if c == b']' {
                self.done = true;
                i += 1;
                break;
            }
            i += 1;
        }
        self.scanned = i;
        out
    }

    /// Best-effort decode of the (possibly unfinished) "reply" string.
    pub fn reply_so_far(&self) -> String {
        reply_prefix(&self.buf)
    }
}

pub fn strip_think(s: &str) -> &str {
    match s.find("</think>") {
        Some(i) => &s[i + 8..],
        None => s,
    }
}

fn find_key(s: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{key}\"");
    let k = s.find(&pat)? + pat.len();
    let colon = s[k..].find(':')?;
    Some(k + colon + 1)
}

fn reply_prefix(buf: &str) -> String {
    let buf = strip_think(buf);
    let Some(k) = find_key(buf, "reply") else { return String::new() };
    let rest = &buf[k..];
    let Some(q) = rest.find('"') else { return String::new() };
    let mut out = String::new();
    let mut chars = rest[q + 1..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(c) => out.push(c),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

/// Authoritative parse of a complete reply. Tolerates code fences, `<think>` blocks, a bare MEL
/// array, `"mel"` given as one string, and prose around the JSON.
pub fn parse_reply(content: &str) -> Result<(String, Vec<String>), String> {
    let s = strip_think(content).trim();
    let s = s.trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let Some(start) = s.find(['{', '[']) else {
        // Plain prose answer.
        return if s.is_empty() { Err("empty reply from model".into()) } else { Ok((s.to_string(), vec![])) };
    };
    let v: Value = serde_json::Deserializer::from_str(&s[start..])
        .into_iter::<Value>()
        .next()
        .ok_or("empty JSON")?
        .map_err(|e| format!("invalid JSON from model: {e}"))?;
    let (reply, mel) = match &v {
        Value::Array(a) => (String::new(), a.clone()),
        Value::Object(_) => {
            let reply = v["reply"].as_str().or(v["text"].as_str()).unwrap_or("").to_string();
            let mel = match &v["mel"] {
                Value::Array(a) => a.clone(),
                Value::String(s) => vec![Value::String(s.clone())],
                _ => match &v["script"] {
                    Value::String(s) => vec![Value::String(s.clone())],
                    _ => vec![],
                },
            };
            (reply, mel)
        }
        _ => (String::new(), vec![]),
    };
    let mel = mel
        .into_iter()
        .filter_map(|m| m.as_str().map(|s| s.trim().to_string()))
        .flat_map(|s| split_statements(&s))
        .collect();
    Ok((reply, mel))
}

/// Split a MEL script into statements (on ';' and newlines, ignoring `//` comments).
pub fn split_statements(script: &str) -> Vec<String> {
    script
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .flat_map(|l| l.split(';'))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Offline English → MEL. Returns None when the sentence isn't confidently understood.
// ---------------------------------------------------------------------------------------------

fn word_num(w: &str) -> Option<f32> {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-');
    if let Ok(n) = w.parse::<f32>() {
        return Some(n);
    }
    Some(match w {
        "zero" => 0.0,
        "one" | "a" | "an" => 1.0,
        "two" | "to" | "too" => 2.0,
        "three" => 3.0,
        "four" | "for" => 4.0,
        "five" => 5.0,
        "six" => 6.0,
        "seven" => 7.0,
        "eight" => 8.0,
        "nine" => 9.0,
        "ten" => 10.0,
        "twelve" => 12.0,
        "fifteen" => 15.0,
        "twenty" => 20.0,
        "thirty" => 30.0,
        "forty-five" => 45.0,
        "forty" => 40.0,
        "fifty" => 50.0,
        "sixty" => 60.0,
        "ninety" => 90.0,
        "hundred" => 100.0,
        "half" => 0.5,
        _ => return None,
    })
}

fn number_in(words: &[&str]) -> Option<f32> {
    words.iter().filter(|w| !matches!(**w, "a" | "an" | "to" | "too" | "for")).find_map(|w| word_num(w))
}

pub const PRIMS: [(&str, &str); 8] = [
    ("cube", "polyCube"),
    ("box", "polyCube"),
    ("sphere", "polySphere"),
    ("ball", "polySphere"),
    ("cylinder", "polyCylinder"),
    ("cone", "polyCone"),
    ("plane", "polyPlane"),
    ("torus", "polyTorus"),
];

/// Short requests that map onto a recipe: "studio lighting", "make it gold", "make him walk",
/// "rig and animate a dance", "render check".
pub fn parse_recipe(words: &[&str]) -> Option<String> {
    if words.len() > 8 {
        return None;
    }
    let has = |w: &str| words.contains(&w);
    let t = words.join(" ");
    if matches!(t.as_str(), "render check" | "check render" | "check my render" | "check render quality" | "review render" | "review my render") {
        return Some("renderCheck".into());
    }
    // Lighting: a mood word plus light/lighting/lit.
    if words.iter().any(|w| w.starts_with("light") || *w == "lit" || *w == "lights") {
        let preset = [
            ("studio", "studio"),
            ("portrait", "studio"),
            ("outdoor", "outdoor"),
            ("outdoors", "outdoor"),
            ("daylight", "outdoor"),
            ("day", "outdoor"),
            ("sunny", "outdoor"),
            ("sunset", "sunset"),
            ("golden", "sunset"),
            ("dusk", "sunset"),
            ("night", "night"),
            ("moonlight", "night"),
            ("moody", "night"),
            ("product", "product"),
            ("packshot", "product"),
            ("dramatic", "dramatic"),
            ("noir", "dramatic"),
            ("cinematic", "dramatic"),
        ]
        .iter()
        .find(|(k, _)| has(k))
        .map(|(_, p)| *p);
        if let Some(p) = preset {
            return Some(format!("lightRig -preset {p}"));
        }
    }
    // Materials: "make it gold", "gold material", "chrome shader", "turn it into glass".
    let mat_words = ["material", "shader", "make", "turn", "into", "look", "like", "it", "them", "this", "selected", "selection", "a", "an", "made", "out"];
    if words.iter().all(|w| mat_words.contains(w) || crate::recipes::material_preset(w).is_some() || matches!(*w, "car" | "paint" | "brushed" | "metal"))
        && words.iter().any(|w| matches!(*w, "material" | "shader" | "make" | "turn" | "made" | "look"))
    {
        let joined = t.replace("car paint", "car_paint").replace("brushed metal", "brushed_metal");
        if let Some(p) = joined.split_whitespace().find(|w| crate::recipes::material_preset(w).is_some() && !matches!(*w, "light")) {
            return Some(format!("materialPreset -p {p}"));
        }
    }
    // Rig + animate: an action verb plus a "make it / rig / animate / cycle" cue.
    let anim = [("walk", "walk"), ("walking", "walk"), ("run", "run"), ("running", "run"), ("jog", "run"), ("jump", "jump"), ("jumping", "jump"), ("wave", "wave"), ("waving", "wave"), ("idle", "idle"), ("breathe", "idle"), ("breathing", "idle"), ("dance", "dance"), ("dancing", "dance")]
        .iter()
        .find(|(k, _)| has(k))
        .map(|(_, a)| *a);
    let cue = words.iter().any(|w| matches!(*w, "make" | "rig" | "animate" | "cycle" | "him" | "her" | "character" | "it" | "let"));
    if let Some(a) = anim
        && cue
        && !has("script")
        && !has("mel")
    {
        return Some(format!("rigAnimate -anim {a}"));
    }
    None
}

pub fn parse_simple(text: &str) -> Option<Vec<String>> {
    let t = text.to_lowercase().replace([',', '!', '?', '.'], " ");
    let t = t.trim();
    let words: Vec<&str> = t.split_whitespace().filter(|w| !matches!(*w, "please" | "the" | "now" | "can" | "you")).collect();
    if words.is_empty() {
        return None;
    }
    let t = words.join(" ");
    let has = |w: &str| words.contains(&w);
    if let Some(r) = parse_recipe(&words) {
        return Some(vec![r]);
    }
    if matches!(
        t.as_str(),
        "explain my rig" | "explain rig" | "explain the rig" | "check my rig" | "check rig" | "check the rig" | "rig report" | "how does my rig work" | "how does the rig work" | "what's wrong with my rig" | "what is wrong with my rig"
    ) {
        return Some(vec!["rigReport".into()]);
    }
    if crate::director::is_shot_request(&t) {
        return Some(vec![crate::director::shot_mel(&crate::director::parse_shot(&t))]);
    }
    if crate::posing::is_pose_request(&t)
        && let Some(p) = crate::posing::parse_pose(text)
    {
        return Some(p);
    }
    if words.len() > 9 || t.contains(" and ") || t.contains(" then ") || t.contains(" of ") || t.contains("animate") || t.contains(" each ") {
        return None;
    }
    let one = |s: &str| Some(vec![s.to_string()]);
    match t.as_str() {
        "undo" | "undo that" | "go back" => return one("undo"),
        "redo" => return one("redo"),
        "play" | "play animation" | "start playback" => return one("play"),
        "stop" | "pause" | "stop playback" => return one("stop"),
        "delete" | "delete it" | "delete that" | "delete selected" | "delete selection" | "remove it" => return one("delete"),
        "select all" => return one("select -all"),
        "deselect" | "deselect all" | "select none" | "clear selection" => return one("select -cl"),
        "duplicate" | "duplicate it" | "copy it" => return one("duplicate"),
        "group" | "group them" | "group it" | "group selected" => return one("group"),
        "smooth" | "smooth it" | "smooth mesh" => return one("polySmooth"),
        "set key" | "set a key" | "key it" | "keyframe" | "set keyframe" | "add keyframe" => return one("setKeyframe"),
        "frame all" | "show all" | "zoom all" => return one("viewFit -all"),
        "frame selected" | "frame selection" | "focus" | "zoom to selection" => return one("viewFit"),
        "four view" | "quad view" | "four views" => return one("panelLayout -quad"),
        "single view" | "one view" => return one("panelLayout -single"),
        "wireframe" | "wireframe mode" => return one("displayMode -wire"),
        "shaded" | "smooth shade" | "shaded mode" => return one("displayMode -shaded"),
        "wireframe on shaded" => return one("displayMode -wos"),
        "vertex mode" | "vertices" => return one("selectMode -v"),
        "face mode" | "faces" => return one("selectMode -f"),
        "edge mode" | "edges" => return one("selectMode -e"),
        "bevel" | "bevel it" | "bevel edges" => return one("polyBevel"),
        "bridge" | "bridge faces" => return one("polyBridge"),
        "merge" | "merge vertices" => return one("polyMergeVertex"),
        "mirror" | "mirror it" | "mirror geometry" => return one("polyMirrorFace -ax x"),
        "insert edge loop" | "add edge loop" | "edge loop" => return one("polySplitRing"),
        "quick rig" | "rig it" | "rig the character" | "rig character" | "auto rig" | "rig this character" => return one("quickRig"),
        "mirror joints" | "mirror skeleton" | "mirror joint" => return one("mirrorJoint -myz -sr L_ R_"),
        "mirror weights" | "mirror skin weights" => return one("copySkinWeights -mm YZ"),
        "cluster" | "create cluster" | "make a cluster" => return one("cluster"),
        "blend shape" | "create blend shape" | "make blend shape" => return one("blendShape"),
        "x-ray" | "xray" | "x ray" | "toggle x-ray" | "xray mode" | "x-ray mode" => return one("xray"),
        "use all lights" | "all lights" | "scene lights" => return one("lighting -all"),
        "default lighting" | "default lights" => return one("lighting -default"),
        "shadows" | "shadows on" | "show shadows" | "turn on shadows" => return one("lighting -all -shadows on"),
        "shadows off" | "hide shadows" | "turn off shadows" => return one("lighting -shadows off"),
        "hypershade" | "open hypershade" | "materials" => return one("hyperShade"),
        "add a light" | "add light" | "create light" | "add sun" | "add a sun" => return one("directionalLight -i 1 -rot -50 30 0 -sh on"),
        "add a camera" | "add camera" | "create camera" => return one("camera"),
        "uv checker" | "show uv checker" | "checker" | "uv checker on" => return one("uvChecker on"),
        "uv checker off" | "hide uv checker" => return one("uvChecker off"),
        "uv editor" | "open uv editor" | "show uvs" => return one("uvEditor"),
        "unwrap" | "unwrap it" | "auto uv" | "automatic uvs" | "auto unwrap" | "automatic projection" => return one("polyAutoProjection"),
        "layout uvs" | "pack uvs" | "layout" => return one("polyLayoutUV"),
        "planar projection" | "planar map" => return one("polyPlanarProjection -md b"),
        "spherical projection" => return one("polySphericalProjection"),
        "cylindrical projection" => return one("polyCylindricalProjection"),
        "textures" | "show textures" | "textured" => return one("textured on"),
        "ghost" | "ghost it" | "onion skin" | "ghost selected" => return one("ghost on"),
        "unghost" | "ghost off" => return one("ghost off"),
        "motion trail" | "show motion trail" | "trail" => return one("motionTrail on"),
        "hide motion trail" | "motion trail off" => return one("motionTrail off"),
        "dope sheet" | "open dope sheet" => return one("dopeSheet"),
        "graph editor" | "open graph editor" => return one("graphEditor"),
        "pose library" | "poses" => return one("poseLibrary -list"),
        "linear tangents" | "make it linear" => return one("keyTangent -ott linear"),
        "stepped tangents" | "make it stepped" | "stepped" => return one("keyTangent -ott step"),
        "smooth tangents" | "spline tangents" => return one("keyTangent -ott spline"),
        "copy keys" => return one("copyKey"),
        "render" | "render it" | "render the scene" | "render frame" | "render current frame" | "make a render" => return one("render"),
        "ipr" | "start ipr" | "interactive render" => return one("renderView -ipr on; render"),
        "stop render" | "stop rendering" | "stop ipr" => return one("renderView -ipr off; renderView -stop"),
        "render view" | "open render view" => return one("renderView"),
        "multi cut" | "multi-cut" | "multicut" | "cut tool" => return one("setToolTo multiCut"),
        "sculpt" | "sculpt tool" | "sculpt mode" => return one("sculpt -b sculpt"),
        "smooth brush" => return one("sculpt -b smooth"),
        "grab brush" => return one("sculpt -b grab"),
        "flatten brush" => return one("sculpt -b flatten"),
        "inflate brush" => return one("sculpt -b inflate"),
        "pinch brush" => return one("sculpt -b pinch"),
        "symmetry on" | "sculpt symmetry" => return one("sculpt -sym on"),
        "symmetry off" => return one("sculpt -sym off"),
        "soft select" | "soft selection" | "soft select on" => return one("softSelect -sse on"),
        "soft select off" => return one("softSelect -sse off"),
        "lattice" | "add lattice" | "add a lattice" | "create lattice" => return one("lattice"),
        "remove lattice" | "remove wrap" => return one("lattice -rm"),
        "wrap" | "wrap deformer" => return one("wrap"),
        "paste keys" => return one("pasteKey"),
        "object mode" => return one("selectMode -o"),
        "move tool" => return one("setToolTo move"),
        "rotate tool" => return one("setToolTo rotate"),
        "scale tool" => return one("setToolTo scale"),
        "select tool" => return one("setToolTo select"),
        "freeze transformations" | "freeze transforms" | "freeze" => return one("makeIdentity"),
        "delete history" => return one("delete -ch"),
        "new scene" | "clear scene" => return one("file -new"),
        _ => {}
    }
    for v in ["top", "front", "side", "persp", "perspective"] {
        if t == format!("{v} view") || t == format!("{v} camera") || t == format!("go to {v} view") || t == format!("look through {v}") {
            let v = if v == "perspective" { "persp" } else { v };
            return one(&format!("viewSet -{v}"));
        }
    }
    // go to frame N
    if (t.starts_with("go to frame") || t.starts_with("frame ")) && !t.contains("all") && !t.contains("select") {
        return number_in(&words).map(|n| vec![format!("currentTime {}", n.round())]);
    }
    // create / add / make [a] [color] <prim>
    if matches!(words[0], "create" | "add" | "make" | "new" | "give") {
        if let Some((_, cmd)) = PRIMS.iter().find(|(w, _)| has(w)) {
            // Only plain "create [a] [colour] <primitive>" sentences; anything else goes to the AI.
            let plain = words.iter().all(|w| {
                matches!(*w, "create" | "add" | "make" | "new" | "give" | "me" | "a" | "an" | "another" | "one" | "polygon" | "poly")
                    || PRIMS.iter().any(|(p, _)| p == w)
                    || crate::commands::named_color(w).is_some()
            });
            if !plain {
                return None;
            }
            let mut out = vec![cmd.to_string()];
            if let Some(c) = words.iter().find(|w| crate::commands::named_color(w).is_some()) {
                out.push(format!("setColor {c}"));
            }
            return Some(out);
        }
    }
    // colour it red / make it blue / paint it gold
    if (words[0] == "color" || words[0] == "colour" || words[0] == "paint" || words[0] == "make") && (has("it") || has("selected") || has("them")) {
        if let Some(c) = words.iter().find(|w| crate::commands::named_color(w).is_some()) {
            return one(&format!("setColor {c}"));
        }
        if has("bigger") || has("larger") {
            return one("scale -r 1.5 1.5 1.5");
        }
        if has("smaller") {
            return one("scale -r 0.667 0.667 0.667");
        }
    }
    // move [it] up/down/left/right/forward/back N
    if words[0] == "move" {
        let n = number_in(&words[1..]).unwrap_or(1.0);
        let dir = if has("up") {
            [0.0, n, 0.0]
        } else if has("down") {
            [0.0, -n, 0.0]
        } else if has("left") {
            [-n, 0.0, 0.0]
        } else if has("right") {
            [n, 0.0, 0.0]
        } else if has("forward") || has("forwards") || has("closer") {
            [0.0, 0.0, n]
        } else if has("back") || has("backward") || has("backwards") || has("away") {
            [0.0, 0.0, -n]
        } else {
            return None;
        };
        return one(&format!("move -r {} {} {}", dir[0], dir[1], dir[2]));
    }
    // rotate [it] N [degrees] [on|around] x|y|z
    if words[0] == "rotate" || words[0] == "spin" || words[0] == "turn" {
        let n = number_in(&words[1..]).unwrap_or(90.0);
        let axis = if has("x") { 0 } else if has("z") { 2 } else { 1 };
        let mut v = [0.0; 3];
        v[axis] = n;
        return one(&format!("rotate -r {} {} {}", v[0], v[1], v[2]));
    }
    // scale [it] [by] N
    if words[0] == "scale" {
        let n = number_in(&words[1..])?;
        return one(&format!("scale -r {n} {n} {n}"));
    }
    // extrude [by] N
    if words[0] == "extrude" {
        let n = number_in(&words[1..]).unwrap_or(0.5);
        return one(&format!("polyExtrudeFacet -ltz {n}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_from_short_requests() {
        let p = |t: &str| parse_simple(t).map(|v| v.join("; "));
        assert_eq!(p("studio lighting").as_deref(), Some("lightRig -preset studio"));
        assert_eq!(p("Give it sunset lighting please").as_deref(), Some("lightRig -preset sunset"));
        assert_eq!(p("light it like a product shot").as_deref(), Some("lightRig -preset product"));
        assert_eq!(p("make it gold").as_deref(), Some("materialPreset -p gold"));
        assert_eq!(p("chrome material").as_deref(), Some("materialPreset -p chrome"));
        assert_eq!(p("turn it into glass").as_deref(), Some("materialPreset -p glass"));
        assert_eq!(p("make it car paint").as_deref(), Some("materialPreset -p car_paint"));
        assert_eq!(p("make it walk").as_deref(), Some("rigAnimate -anim walk"));
        assert_eq!(p("rig and animate a dance").as_deref(), Some("rigAnimate -anim dance"));
        assert_eq!(p("walk cycle").as_deref(), Some("rigAnimate -anim walk"));
        assert_eq!(p("render check").as_deref(), Some("renderCheck"));
        // Not recipes.
        assert_eq!(p("make it red").as_deref(), Some("setColor red"));
        assert!(p("add a light").is_some_and(|m| m.starts_with("directionalLight")));
        assert!(parse_recipe(&["run", "the", "script"]).is_none());
    }

    #[test]
    fn stream_parser_emits_statements_early() {
        let mut p = MelStreamParser::default();
        assert!(p.push("{\"reply\":\"Building a ta").is_empty());
        assert_eq!(p.reply_so_far(), "Building a ta");
        assert!(p.push("ble\",\"mel\":[\"polyCube -w 4 -h 0.2 -n top\",\"mo").first().is_some_and(|s| s == "polyCube -w 4 -h 0.2 -n top"));
        let rest = p.push("ve 0 2 0\", \"print \\\"hi\\\"\"]}");
        assert_eq!(rest, vec!["move 0 2 0".to_string(), "print \"hi\"".to_string()]);
        assert_eq!(p.reply_so_far(), "Building a table");
    }

    #[test]
    fn final_parse_variants() {
        let (r, m) = parse_reply("<think>x</think>```json\n{\"reply\":\"ok\",\"mel\":\"polyCube; move 0 1 0\"}\n```").unwrap();
        assert_eq!(r, "ok");
        assert_eq!(m, vec!["polyCube", "move 0 1 0"]);
        let (_, m) = parse_reply("[\"undo\"]").unwrap();
        assert_eq!(m, vec!["undo"]);
        let (r, m) = parse_reply("Press W for the move tool.").unwrap();
        assert!(m.is_empty() && r.starts_with("Press"));
    }

    #[test]
    fn offline_phrases() {
        assert_eq!(parse_simple("Undo."), Some(vec!["undo".into()]));
        assert_eq!(parse_simple("add a red sphere"), Some(vec!["polySphere".into(), "setColor red".into()]));
        assert_eq!(parse_simple("move it up 2"), Some(vec!["move -r 0 2 0".into()]));
        assert_eq!(parse_simple("rotate it ninety degrees on x"), Some(vec!["rotate -r 90 0 0".into()]));
        assert_eq!(parse_simple("top view"), Some(vec!["viewSet -top".into()]));
        assert_eq!(parse_simple("go to frame 24"), Some(vec!["currentTime 24".into()]));
        assert_eq!(parse_simple("build a castle with four towers and a moat"), None);
        assert_eq!(parse_simple("make a ring of cubes"), None);
        assert_eq!(parse_simple("make the sphere bounce"), None);
        assert_eq!(parse_simple("explain my rig"), Some(vec!["rigReport".into()]));
        assert_eq!(parse_simple("raise the left arm and look right"), Some(vec!["pose -part arm -side L -action raise".into(), "pose -part head -action turn -dir right".into()]));
        assert!(parse_simple("make a shot of a robot walking in the desert at sunset").is_some_and(|v| v[0].starts_with("makeShot -theme desert -subject robot -anim walk")));
        assert_eq!(parse_simple("add a big cube next to the sphere"), None);
    }
}
