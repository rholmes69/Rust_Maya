//! A small MEL-like command language for the command line and Script Editor.

use crate::app::{App, CompMode, Tool};
use crate::mesh::Primitive;
use crate::scene::{Channel, NodeId, Shape};
use glam::{Vec2, Vec3};

pub const HELP: &str = "\
Commands (MEL-style, separate with ';'):
  polyCube [-w W] [-h H] [-d D] [-sx N] [-sy N] [-sz N] [-n name]
  polySphere [-r R] [-sa N] [-sh N] [-n name]
  polyCylinder [-r R] [-h H] [-sa N] [-n name]
  polyCone [-r R] [-h H] [-sa N] [-n name]
  polyPlane [-w W] [-h H] [-sx N] [-sy N] [-n name]
  polyTorus [-r R] [-sr R] [-sa N] [-sh N] [-n name]
  spaceLocator | group [names] | duplicate [-n name] | delete [names]
  select [-add|-tgl|-d] names...  (wildcards *)   select -cl
  move|rotate|scale [-r] x y z [names]
  setAttr node.attr value     getAttr node.attr
  setKeyframe [-at attr] [-t frame] [names]
  currentTime N    playbackOptions [-min N] [-max N] [-fps N]
  parent child... parent   |   parent -w child...
  polySmooth | polyExtrudeFacet | polyTriangulate | polyUnite | polySeparate
  makeIdentity (freeze) | delete -ch (history) | rename old new
  setToolTo select|move|rotate|scale | selectMode -o|-v|-f
  setColor <name|r g b> [names]   hide [names]   showHidden [-all|names]
  selectFaces x y z [-tol 0.7] [name]   polyExtrudeFacet [-ltz dist]   cutKey [names]
  setKeyframe [-t frame] [-at attr] [-v value] [names]
  viewSet -persp|-top|-front|-side   viewFit [-all]   panelLayout -single|-quad
  displayMode -wire|-shaded|-wos    xray [on|off]    lighting -all|-default [-shadows on|off]
Export:
  file -export path.glb|path.gltf|path.usda|path.obj [-es (selected only)] [-na (no animation)] [-bp (USD: bake points instead of UsdSkel)]
  file -import path.glb|path.gltf|path.obj [-take animName]   (glTF: hierarchy, meshes+UVs, materials+textures, skins, blend shapes, animation, lights, cameras)
UVs and textures:
  polyPlanarProjection [-md x|y|z|b] | polyCylindricalProjection | polySphericalProjection | polyAutoProjection  [objects]
  polyMapCut [-e edgeIdx...] [mesh] (cut along selected edges)   polyMapSewMove [-e ...] (move & sew)   u3dUnfold [mesh] (LSCM unfold)
  polyLayoutUV | polyNormalizeUV | polyEditUV [-u du] [-v dv] [-su s] [-sv s] [-a deg] [-pu u -pv v] | polyFlipUV [-ft 0|1]
  setAttr mat.colorMap \"C:/tex/wood.png\" | checker | none      setAttr mat.repeatUV 2 2
  setAttr mat.normalMap \"C:/tex/n.png\"   setAttr mat.roughnessMap \"C:/tex/r.png\"   setAttr mat.metalnessMap \"C:/tex/m.png\"   (none disconnects)      uvChecker on|off   textured on|off   uvEditor
Animation tools:
  keyTangent [-t frame] -itt|-ott auto|spline|linear|flat|step [objects]    copyKey [objs]   pasteKey [-t frame] [objs]
  ghost [on|off] [-st step] [objects]   motionTrail [on|off] [objects]   dopeSheet
  poseLibrary -save name [-h] | -apply name [-w 0.5] [-key] [-mirror] | -delete name | -list
  retarget -src sourceRoot -dst targetRoot [-s start] [-e end]   (joints matched by name / body part)
Modeling tools:
  setToolTo multiCut|sculpt   polySplit -ep edgeIdx t -ep edgeIdx t ... [mesh]   (Multi-Cut)
  softSelect -sse on|off [-ssd radius]   (B toggles; affects move/rotate/scale of components)
  sculpt [-b sculpt|smooth|grab|flatten|inflate|pinch] [-s 0..1] [-r pixels] [-sym on|off] [-inv on|off]
  lattice [-dv 2 5 2] [-n name] [objects]   latticePoint lat i j k x y z   lattice -e -reset lat   lattice -rm objects
  wrap [-d driver] targets...   (or select targets, then the driver last)
Recipes:
  lightRig -preset studio|outdoor|sunset|night|product|dramatic [-i 1] [-target objects]   (replaces the lightRig group)
  materialPreset -p gold|silver|chrome|copper|brass|brushed_metal|plastic|rubber|glass|water|wood|concrete|marble|ceramic|car_paint|skin|fabric|neon|grass [-rgb r g b] [-n name] [objects]
  rigAnimate -anim walk|run|jump|wave|idle|dance [-s 1] [-e 96] [-travel on|off] [mesh]   (Quick Rigs the mesh first if needed)
  renderCheck [-x 320 -y 180 -spp 16] [-cam camera]   (preview render + exposure/framing/lighting report)
Rendering (CPU path tracer):
  render [-x 960 -y 540] [-spp 64] [-b 4] [-cam camera] [-o file.png]   render -seq -st 1 -e 24 -o C:/out/name
  renderSettings [-x W -y H -spp N -b N -env sky -exp stops -tm srgb|filmic -bg env|black -clamp 4 -dn on|off -cam cam|-av]
  renderView [-ipr on|off] [-stop] [-save file.png [-passes]]   render ... -o file.png -passes (beauty/albedo/normal/depth/objectid)
  renderSettings -region x0 y0 x1 y1 (0..1, top-left) | -region off
Look dev:
  shadingNode -asShader lambert|blinn|phong|standardSurface -n name [-rgb r g b]
  setAttr mat.color r g b | mat.specular | mat.roughness | mat.metalness | mat.incandescence r g b | mat.transparency
  hyperShade -assign mat [objects]   sets -e -forceElement matSG objects   hyperShade (opens the window)
  areaLight [-w 2 -h 2] [-i I] [-rgb r g b] [-p x y z] [-aim x y z] [-n name]   (soft rectangular light; render view)
  directionalLight|pointLight|spotLight|ambientLight [-i I] [-rgb r g b] [-ca cone] [-sh on] [-p x y z] [-rot x y z] [-aim x y z] [-n name]
  setAttr light.intensity | light.color r g b | light.coneAngle | light.useDepthMapShadows 1
  camera [-fl 35] [-p x y z] [-aim x y z] [-n name]   lookThru cam|persp   setAttr cam.focalLength 50
Edge modeling:
  selectMode -e   polySplitRing [-wt 0.5] [-dir x y z] [mesh]   polyBevel [-o width]   polyBridge (2 faces)
  polyMergeVertex [-d dist]   polyMirrorFace [-ax x|y|z]   polyDelEdge   polySelectEdgeLoop   polySelectEdgeRing
Rigging:
  joint [-p x y z] [-n name] [-rad r]   (parents to the selected joint)    joint -e -oj xyz [joints]
  ikHandle -sj start -ee end [-n name]   poleVectorConstraint pole ikHandle
  parentConstraint|pointConstraint|orientConstraint|scaleConstraint|aimConstraint [-mo] [-aim x y z] [-u x y z] drivers... target
  skinCluster [-mi 4] [-tsb] joints... meshes    skinCluster -e -ub meshes    gotoBindPose
  circle [-r R] [-nr x y z] [-p x y z] [-n name]    controlShape -s circle|square|cross|arrow|cube|sphere [-r size]
  setDrivenKeyframe -cd driver.attr [-dv driverValue] -at attr [-v value] driven   (attr may be a blend target)
  ikHandle -e [-stretch on|off] [-twist deg] handle
  mirrorJoint [-myz|-mxy|-mxz] [-sr L_ R_] joint      quickRig [mesh]   (one-click biped)
  mirrorSkinWeights [-ax x] [-n2p] mesh    copySkinWeights -mm YZ [-mirrorInverse] mesh
  blendShape [-n name] targets... base    blendShape -e -w target value base    blendShape -e -rm base
  setAttr base.target 0.5 / setKeyframe -at target base  (blend-shape weights)
  cluster [-n name] [-fo falloff] [meshes]   (uses selected vertices in component mode)
  ls [-sl] | undo | redo | file -new | play | stop | help";

/// Named colours understood by `setColor` (and the offline voice parser).
pub fn named_color(name: &str) -> Option<[f32; 3]> {
    Some(match name.to_lowercase().as_str() {
        "red" => [0.85, 0.15, 0.12],
        "green" => [0.2, 0.7, 0.25],
        "blue" => [0.15, 0.35, 0.9],
        "yellow" => [0.95, 0.85, 0.15],
        "orange" => [0.95, 0.5, 0.1],
        "purple" | "violet" => [0.55, 0.25, 0.8],
        "pink" => [0.95, 0.5, 0.7],
        "white" => [0.95, 0.95, 0.95],
        "black" => [0.05, 0.05, 0.05],
        "grey" | "gray" => [0.5, 0.5, 0.5],
        "brown" => [0.45, 0.28, 0.15],
        "gold" | "golden" => [0.9, 0.7, 0.2],
        "silver" => [0.75, 0.75, 0.78],
        "cyan" => [0.1, 0.8, 0.85],
        "magenta" => [0.85, 0.15, 0.75],
        "skin" => [0.9, 0.7, 0.6],
        "wood" => [0.55, 0.36, 0.2],
        "sky" => [0.5, 0.75, 0.95],
        "grass" => [0.35, 0.6, 0.2],
        "navy" => [0.08, 0.12, 0.4],
        "teal" => [0.1, 0.5, 0.5],
        _ => return None,
    })
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = false;
    for c in s.chars() {
        match c {
            '"' => quote = !quote,
            c if c.is_whitespace() && !quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn wildcard(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() {
            continue;
        }
        match name[pos..].find(p) {
            Some(idx) => {
                if i == 0 && idx != 0 {
                    return false;
                }
                pos += idx + p.len();
            }
            None => return false,
        }
    }
    parts.last().is_none_or(|l| l.is_empty() || name.ends_with(l))
}

struct Args {
    flags: Vec<(String, Option<String>)>,
    positional: Vec<String>,
}

impl Args {
    fn parse(tokens: &[String], valued: &[&str]) -> Args {
        let mut flags = Vec::new();
        let mut positional = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let t = &tokens[i];
            let is_flag = t.starts_with('-') && t.len() > 1 && t.parse::<f32>().is_err();
            if is_flag {
                let name = t[1..].to_string();
                if valued.contains(&name.as_str()) && i + 1 < tokens.len() {
                    flags.push((name, Some(tokens[i + 1].clone())));
                    i += 2;
                    continue;
                }
                flags.push((name, None));
            } else {
                positional.push(t.clone());
            }
            i += 1;
        }
        Args { flags, positional }
    }
    fn has(&self, f: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == f)
    }
    fn get(&self, f: &str) -> Option<&str> {
        self.flags.iter().find(|(n, _)| n == f).and_then(|(_, v)| v.as_deref())
    }
    fn f(&self, names: &[&str], default: f32) -> f32 {
        names.iter().find_map(|n| self.get(n)).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn u(&self, names: &[&str], default: u32) -> u32 {
        names.iter().find_map(|n| self.get(n)).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}

fn resolve(app: &App, names: &[String]) -> Result<Vec<NodeId>, String> {
    let mut out = Vec::new();
    for n in names {
        let matches: Vec<NodeId> = app.scene.nodes.iter().filter(|nd| wildcard(n, &nd.name)).map(|nd| nd.id).collect();
        if matches.is_empty() {
            return Err(format!("No object matches name: {n}"));
        }
        for m in matches {
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }
    Ok(out)
}

fn targets(app: &App, names: &[String]) -> Result<Vec<NodeId>, String> {
    if names.is_empty() { Ok(app.sel.clone()) } else { resolve(app, names) }
}

fn split_attr(s: &str) -> Result<(String, Channel), String> {
    let (node, attr) = s.split_once('.').ok_or_else(|| format!("Expected node.attribute, got '{s}'"))?;
    let ch = Channel::parse(attr).ok_or_else(|| format!("Unknown attribute '{attr}'"))?;
    Ok((node.to_string(), ch))
}

/// Execute one or more ';'-separated commands. Returns the result string of the last one.
pub fn execute(app: &mut App, src: &str) -> Result<String, String> {
    let mut last = String::new();
    for stmt in src.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() || stmt.starts_with("//") {
            continue;
        }
        app.log.push(format!("{stmt};"));
        match run(app, stmt) {
            Ok(r) => last = r,
            Err(e) => {
                app.last_mel_error = Some((src.to_string(), e.clone()));
                return Err(e);
            }
        }
    }
    Ok(last)
}

fn named_prim(app: &mut App, prim: Primitive, a: &Args) -> String {
    let id = app.create_primitive(prim);
    if let Some(n) = a.get("n").or(a.get("name")) {
        let n = n.to_string();
        app.rename_node(id, &n);
    }
    app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default()
}

/// Remove a flag followed by three numbers (e.g. `-p 1 2 3`) from the token list.
fn take_vec3(tokens: &mut Vec<String>, flags: &[&str]) -> Option<Vec3> {
    let i = tokens.iter().position(|t| flags.iter().any(|f| t == &format!("-{f}")))?;
    let nums: Vec<f32> = tokens.iter().skip(i + 1).take(3).filter_map(|t| t.parse().ok()).collect();
    if nums.len() != 3 {
        return None;
    }
    tokens.drain(i..i + 4);
    Some(Vec3::new(nums[0], nums[1], nums[2]))
}

/// setAttr on materials (`mat.color 1 0 0`), lights (`key.intensity 2`) and cameras
/// (`cam.focalLength 50`). None when `spec` isn't one of those.
fn set_look_attr(app: &mut App, spec: &str, vals: &[f32]) -> Option<Result<(), String>> {
    let (node, attr) = spec.split_once('.')?;
    if let Some(mid) = app.material_by_name(node) {
        app.checkpoint();
        return Some(app.material_mut(mid).unwrap().set_attr(attr, vals));
    }
    if Channel::parse(attr).is_some() {
        return None;
    }
    let id = app.scene.find_by_name(node)?;
    match app.scene.node(id).map(|n| &n.shape) {
        Some(Shape::Light(_)) | Some(Shape::Camera(_)) => {}
        _ => return None,
    }
    app.checkpoint();
    Some(match &mut app.scene.node_mut(id).unwrap().shape {
        Shape::Light(l) => l.set_attr(attr, vals),
        Shape::Camera(c) => c.set_attr(attr, vals),
        _ => unreachable!(),
    })
}

/// `node.target` where target is a blend-shape target on node.
fn blend_attr(app: &App, spec: &str) -> Option<(NodeId, usize)> {
    let (node, attr) = spec.split_once('.')?;
    if Channel::parse(attr).is_some() {
        return None;
    }
    let id = app.scene.find_by_name(node)?;
    Some((id, app.blend_index(id, attr)?))
}

fn rig_split(app: &App, ids: &[NodeId]) -> (Vec<NodeId>, Vec<NodeId>) {
    ids.iter().partition(|id| app.is_joint(**id))
}

fn run(app: &mut App, stmt: &str) -> Result<String, String> {
    let mut tokens = tokenize(stmt);
    let pos_flag = take_vec3(&mut tokens, &["p", "position"]);
    let normal_flag = take_vec3(&mut tokens, &["nr", "normal"]);
    let aim_flag = take_vec3(&mut tokens, &["aim", "aimVector"]);
    let dir_flag = take_vec3(&mut tokens, &["dir", "direction"]);
    let up_flag = take_vec3(&mut tokens, &["u", "upVector"]);
    let rgb_flag = take_vec3(&mut tokens, &["rgb"]);
    let rot_flag = take_vec3(&mut tokens, &["rot", "rotation"]);
    let Some((cmd, rest)) = tokens.split_first() else { return Ok(String::new()) };
    let valued = ["w", "h", "d", "sx", "sy", "sz", "r", "sr", "sa", "sh", "n", "name", "at", "attribute", "t", "time", "min", "max", "fps", "width", "height", "depth", "radius", "v", "value", "ltz", "tol", "sj", "ee", "cd", "dv", "mi", "dr", "s", "rad", "oj", "wt", "o", "ax", "mt", "fo", "stretch", "twist", "mm", "ca", "pen", "fl", "i", "md", "u", "su", "sv", "a", "pu", "pv", "ft", "st", "step"];
    let no_values = matches!(
        cmd.as_str(),
        "move" | "rotate" | "scale" | "select" | "parent" | "hide" | "showHidden" | "delete" | "selectMode" | "viewSet" | "displayMode" | "panelLayout" | "setColor" | "polyColor" | "ls" | "cutKey"
    );
    let valued: &[&str] = if no_values { &[] } else { &valued };
    let a = Args::parse(rest, valued);
    match cmd.as_str() {
        "help" => Ok(HELP.to_string()),
        "polyCube" => Ok(named_prim(app, Primitive::Cube {
            width: a.f(&["w", "width"], 1.0),
            height: a.f(&["h", "height"], 1.0),
            depth: a.f(&["d", "depth"], 1.0),
            sx: a.u(&["sx"], 1),
            sy: a.u(&["sy"], 1),
            sz: a.u(&["sz"], 1),
        }, &a)),
        "polySphere" => Ok(named_prim(app, Primitive::Sphere { radius: a.f(&["r", "radius"], 1.0), axis_div: a.u(&["sa"], 20), height_div: a.u(&["sh"], 20) }, &a)),
        "polyCylinder" => Ok(named_prim(app, Primitive::Cylinder { radius: a.f(&["r", "radius"], 1.0), height: a.f(&["h", "height"], 2.0), axis_div: a.u(&["sa"], 20), height_div: a.u(&["sh"], 1), caps: true }, &a)),
        "polyCone" => Ok(named_prim(app, Primitive::Cone { radius: a.f(&["r", "radius"], 1.0), height: a.f(&["h", "height"], 2.0), axis_div: a.u(&["sa"], 20) }, &a)),
        "polyPlane" => Ok(named_prim(app, Primitive::Plane { width: a.f(&["w", "width"], 1.0), height: a.f(&["h", "height"], 1.0), sx: a.u(&["sx"], 10), sy: a.u(&["sy"], 10) }, &a)),
        "polyTorus" => Ok(named_prim(app, Primitive::Torus { radius: a.f(&["r", "radius"], 1.0), section_radius: a.f(&["sr"], 0.5), axis_div: a.u(&["sa"], 20), height_div: a.u(&["sh"], 20) }, &a)),
        "spaceLocator" => {
            let id = app.create_locator();
            let name = a.get("n").or(a.get("name")).map(String::from);
            app.apply_name(id, name.as_deref());
            if let Some(p) = pos_flag {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "group" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            app.group_selected();
            if let (Some(n), Some(id)) = (a.get("n").or(a.get("name")).map(String::from), app.lead()) {
                app.rename_node(id, &n);
            }
            Ok(app.lead().and_then(|id| app.scene.node(id)).map(|n| n.name.clone()).unwrap_or_default())
        }
        "duplicate" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            app.duplicate_selected();
            if let (Some(n), [id]) = (a.get("n").or(a.get("name")).map(String::from), app.sel.clone().as_slice()) {
                app.rename_node(*id, &n);
            }
            Ok(names(app, &app.sel.clone()))
        }
        "delete" => {
            if a.has("ch") {
                if !a.positional.is_empty() {
                    app.sel = resolve(app, &a.positional)?;
                }
                app.delete_history();
                return Ok(String::new());
            }
            // In component mode `delete` removes the selected components (like Maya).
            if a.positional.is_empty() && app.comp_mode != CompMode::Object && !app.comp_sel.is_empty() {
                app.delete_selected();
                return Ok(String::new());
            }
            let ids = targets(app, &a.positional)?;
            app.checkpoint();
            app.scene.delete(&ids);
            app.sel.retain(|id| !ids.contains(id));
            Ok(String::new())
        }
        "select" => {
            if a.has("cl") || a.has("clear") {
                app.sel.clear();
                app.comp_sel.clear();
                return Ok(String::new());
            }
            if a.has("all") || a.has("ado") {
                app.select_all();
                return Ok(names(app, &app.sel.clone()));
            }
            let ids = resolve(app, &a.positional)?;
            if a.has("add") {
                for id in ids {
                    if !app.sel.contains(&id) {
                        app.sel.push(id);
                    }
                }
            } else if a.has("tgl") || a.has("toggle") {
                for id in ids {
                    app.select_modify(id, true, false);
                }
            } else if a.has("d") || a.has("deselect") {
                app.sel.retain(|x| !ids.contains(x));
            } else {
                app.sel = ids;
            }
            Ok(names(app, &app.sel.clone()))
        }
        "move" | "rotate" | "scale" => {
            let nums: Vec<f32> = a.positional.iter().filter_map(|p| p.parse().ok()).collect();
            let objs: Vec<String> = a.positional.iter().filter(|p| p.parse::<f32>().is_err()).cloned().collect();
            if nums.len() != 3 {
                return Err(format!("{cmd}: expected 3 values"));
            }
            let v = Vec3::new(nums[0], nums[1], nums[2]);
            let ids = targets(app, &objs)?;
            if ids.is_empty() {
                return Err(format!("{cmd}: nothing selected"));
            }
            let rel = a.has("r") || a.has("relative") || a.has("by");
            app.checkpoint();
            for id in &ids {
                if let Some(n) = app.scene.node_mut(*id) {
                    let slot = match cmd.as_str() {
                        "move" => &mut n.translate,
                        "rotate" => &mut n.rotate,
                        _ => &mut n.scale,
                    };
                    if rel {
                        if cmd == "scale" { *slot *= v } else { *slot += v }
                    } else {
                        *slot = v;
                    }
                }
            }
            app.auto_key_touch(&ids);
            Ok(String::new())
        }
        "setAttr" => {
            let spec = a.positional.first().ok_or("setAttr: missing node.attr")?;
            let val_blend: Option<f32> = a.positional.get(1).and_then(|v| v.parse().ok());
            if let Some((bid, bi)) = blend_attr(app, spec) {
                app.set_blend_weight(bid, bi, val_blend.ok_or("setAttr: missing value")?, false);
                return Ok(String::new());
            }
            if let Some((node, attr)) = spec.split_once('.')
                && let Some(slot) = crate::look::MapSlot::parse(attr)
            {
                let mid = app.material_by_name(node).ok_or(format!("No material named '{node}'"))?;
                let v = a.positional.get(1).cloned().unwrap_or_default();
                let path = match v.as_str() {
                    "" | "none" | "off" => None,
                    p => Some(p.to_string()),
                };
                app.set_material_map(mid, slot, path)?;
                return Ok(String::new());
            }
            let nums: Vec<f32> = a.positional[1..]
                .iter()
                .filter_map(|v| match v.as_str() {
                    "true" | "on" => Some(1.0),
                    "false" | "off" => Some(0.0),
                    v => v.parse().ok(),
                })
                .collect();
            if let Some(r) = set_look_attr(app, spec, &nums) {
                return r.map(|_| String::new());
            }
            let (node, ch) = split_attr(spec)?;
            let val: f32 = a
                .positional
                .get(1)
                .and_then(|v| match v.as_str() {
                    "true" | "on" => Some(1.0),
                    "false" | "off" => Some(0.0),
                    v => v.parse().ok(),
                })
                .ok_or("setAttr: missing value")?;
            let id = app.scene.find_by_name(&node).ok_or(format!("No object matches name: {node}"))?;
            app.checkpoint();
            app.scene.node_mut(id).unwrap().set(ch, val);
            app.auto_key_touch(&[id]);
            Ok(String::new())
        }
        "getAttr" => {
            let spec = a.positional.first().ok_or("getAttr: missing node.attr")?;
            if let Some((bid, bi)) = blend_attr(app, spec) {
                return Ok(format!("{}", app.scene.node(bid).and_then(|n| n.blend.as_ref()).map(|b| b.weights[bi]).unwrap_or(0.0)));
            }
            if let Some((node, attr)) = spec.split_once('.')
                && let Some(mid) = app.material_by_name(node)
            {
                return app.material(mid).unwrap().get_attr(attr).ok_or(format!("Unknown material attribute '{attr}'"));
            }
            let (node, ch) = split_attr(spec)?;
            let id = app.scene.find_by_name(&node).ok_or(format!("No object matches name: {node}"))?;
            Ok(format!("{}", app.scene.node(id).unwrap().get(ch)))
        }
        "setKeyframe" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            let prev = app.scene.time;
            let at_time = a.get("t").or(a.get("time")).and_then(|t| t.parse::<f32>().ok());
            if let Some(t) = at_time {
                app.set_time(t);
            }
            if let Some(at) = a.get("at").or(a.get("attribute")).filter(|at| Channel::parse(at).is_none() && !matches!(*at, "translate" | "t" | "rotate" | "r" | "scale" | "s")) {
                let ids = app.sel.clone();
                let mut n = 0;
                app.checkpoint();
                for id in ids {
                    if let Some(i) = app.blend_index(id, at) {
                        if let Some(v) = a.get("v").or(a.get("value")).and_then(|v| v.parse::<f32>().ok()) {
                            app.set_blend_weight(id, i, v, true);
                        }
                        app.key_blend_weight(id, i, false);
                        n += 1;
                    }
                }
                if at_time.is_some() {
                    app.set_time(prev);
                }
                return if n > 0 { Ok(format!("{n}")) } else { Err(format!("Unknown attribute '{at}'")) };
            }
            let chans: Vec<Channel> = match a.get("at").or(a.get("attribute")) {
                Some(at) => match at {
                    "translate" | "t" => Channel::translate().to_vec(),
                    "rotate" | "r" => Channel::rotate().to_vec(),
                    "scale" | "s" => Channel::scale().to_vec(),
                    other => vec![Channel::parse(other).ok_or(format!("Unknown attribute '{other}'"))?],
                },
                None => Channel::ALL.to_vec(),
            };
            if let Some(v) = a.get("v").or(a.get("value")).and_then(|v| v.parse::<f32>().ok()) {
                app.checkpoint();
                for id in app.sel.clone() {
                    if let Some(n) = app.scene.node_mut(id) {
                        for c in &chans {
                            n.set(*c, v);
                        }
                    }
                }
            }
            app.key_selected(&chans);
            if at_time.is_some() {
                app.set_time(prev);
            }
            Ok(format!("{}", chans.len() * app.sel.len()))
        }
        "currentTime" => {
            let t: f32 = a.positional.first().and_then(|v| v.parse().ok()).ok_or("currentTime: missing frame")?;
            app.set_time(t);
            Ok(format!("{t}"))
        }
        "playbackOptions" => {
            if let Some(v) = a.get("min").and_then(|v| v.parse().ok()) {
                app.scene.play_start = v;
                app.scene.anim_start = app.scene.anim_start.min(v);
            }
            if let Some(v) = a.get("max").and_then(|v| v.parse().ok()) {
                app.scene.play_end = v;
                app.scene.anim_end = app.scene.anim_end.max(v);
            }
            if let Some(v) = a.get("fps").and_then(|v| v.parse().ok()) {
                app.scene.fps = v;
            }
            Ok(format!("{} {}", app.scene.play_start, app.scene.play_end))
        }
        "parent" => {
            if a.has("w") || a.has("world") {
                app.sel = resolve(app, &a.positional)?;
                app.unparent_selected();
                return Ok(String::new());
            }
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            app.parent_selected();
            Ok(String::new())
        }
        "rename" => {
            let (id, new) = match a.positional.as_slice() {
                [old, new] => (app.scene.find_by_name(old).ok_or(format!("No object matches name: {old}"))?, new.clone()),
                [new] => (app.lead().ok_or("rename: nothing selected")?, new.clone()),
                _ => return Err("rename: usage rename [old] new".into()),
            };
            app.rename_node(id, &new);
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "polySmooth" => {
            app.smooth();
            Ok(String::new())
        }
        "polyExtrudeFacet" | "polyExtrudeFace" => {
            let prev = app.extrude_offset;
            if let Some(d) = a.get("ltz").and_then(|d| d.parse::<f32>().ok()) {
                app.extrude_offset = d;
            }
            app.extrude();
            app.extrude_offset = prev;
            Ok(String::new())
        }
        "polyTriangulate" => {
            app.triangulate();
            Ok(String::new())
        }
        "polyUnite" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            app.combine();
            Ok(names(app, &app.sel.clone()))
        }
        "polySeparate" => {
            app.separate();
            Ok(names(app, &app.sel.clone()))
        }
        "makeIdentity" => {
            app.freeze_transforms();
            Ok(String::new())
        }
        "setToolTo" => {
            let t = a.positional.first().map(|s| s.to_lowercase()).unwrap_or_default();
            app.tool = match t.as_str() {
                s if s.starts_with("select") => Tool::Select,
                s if s.starts_with("move") => Tool::Move,
                s if s.starts_with("rotate") => Tool::Rotate,
                s if s.starts_with("scale") => Tool::Scale,
                s if s.starts_with("multicut") || s.starts_with("polysplit") => Tool::MultiCut,
                s if s.starts_with("sculpt") => Tool::Sculpt,
                _ => return Err(format!("Unknown tool '{t}'")),
            };
            Ok(String::new())
        }
        "selectMode" => {
            if a.has("v") || a.has("vertex") {
                app.set_comp_mode(CompMode::Vertex);
            } else if a.has("f") || a.has("facet") {
                app.set_comp_mode(CompMode::Face);
            } else if a.has("e") || a.has("edge") {
                app.set_comp_mode(CompMode::Edge);
            } else {
                app.set_comp_mode(CompMode::Object);
            }
            Ok(String::new())
        }
        "ls" => {
            let ids: Vec<NodeId> = if a.has("sl") || a.has("selection") {
                app.sel.clone()
            } else if a.positional.is_empty() {
                app.scene.nodes.iter().map(|n| n.id).collect()
            } else {
                resolve(app, &a.positional)?
            };
            Ok(names(app, &ids))
        }
        "undo" => {
            app.undo();
            Ok(String::new())
        }
        "redo" => {
            app.redo();
            Ok(String::new())
        }
        "file" => {
            if a.has("new") || a.has("f") {
                app.new_scene();
                Ok(String::new())
            } else if a.has("import") || a.has("i") {
                let path: String = a.positional.first().cloned().or(a.get("i").map(String::from)).ok_or("file -import: missing path (.glb, .gltf or .obj)")?;
                let p = std::path::PathBuf::from(path.trim_matches('"'));
                let take = rest.iter().position(|t| t == "-take").and_then(|i| rest.get(i + 1)).cloned();
                crate::importer::import_take(app, &p, take.as_deref())
            } else if a.has("export") || a.has("ea") || a.has("es") || a.has("exportAll") || a.has("exportSelected") {
                let path = a.positional.first().ok_or("file -export: missing path (.glb, .gltf, .usda or .obj)")?;
                let sel = a.has("es") || a.has("exportSelected");
                let p = std::path::PathBuf::from(path.trim_matches('"'));
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("obj")) {
                    app.export_obj(p, sel);
                    return Ok(String::new());
                }
                let mut opts = crate::export::ExportOptions::from_app(app, sel);
                if a.has("noAnim") || a.has("na") {
                    opts.animation = false;
                }
                if a.has("bakePoints") || a.has("bp") {
                    opts.usd_skel = false;
                }
                crate::export::export_path(app, &p, &opts)
            } else {
                Err("file: only -new is supported from the command line; use File menu to open/save".into())
            }
        }
        "play" => {
            app.playing = !a.has("st") && !a.has("state");
            Ok(String::new())
        }
        "stop" => {
            app.playing = false;
            Ok(String::new())
        }
        "print" => Ok(rest.join(" ")),
        // ---------------------------------------------------------------- edge modeling
        "polySplitRing" | "polyInsertEdgeLoop" => {
            let t = a.f(&["wt", "weight"], 0.5);
            if let Some(dir) = dir_flag {
                // Pick the mesh edge most parallel to `dir` and cut its ring.
                let ids = targets(app, &a.positional)?;
                let mut total = 0;
                for id in ids {
                    let Some(m) = app.scene.node(id).and_then(|n| n.mesh()) else { continue };
                    let w = app.scene.world_matrix(id);
                    let d = dir.normalize_or_zero();
                    let best = m.edges().into_iter().max_by(|x, y| {
                        let al = |e: (u32, u32)| (w.transform_vector3(m.positions[e.1 as usize] - m.positions[e.0 as usize])).normalize_or_zero().dot(d).abs();
                        al(*x).total_cmp(&al(*y))
                    });
                    if let Some(e) = best {
                        total += app.insert_edge_loop(id, e, t);
                    }
                }
                return Ok(format!("{total} edge(s) inserted"));
            }
            app.split_ring_selected(t)
        }
        "polyBevel" | "polyBevel3" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            let w = a.f(&["o", "offset"], app.bevel_width);
            app.bevel_selected(w)
        }
        "polyBridge" | "polyBridgeEdge" | "polyBridgeFaces" => app.bridge_selected(),
        "polyMergeVertex" | "polyMerge" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            let d = a.get("d").and_then(|v| v.parse::<f32>().ok());
            app.merge_selected(d)
        }
        "polyMirrorFace" | "polyMirror" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            let axis = match a.get("ax").unwrap_or("x") {
                "y" | "Y" | "1" => 1,
                "z" | "Z" | "2" => 2,
                _ => 0,
            };
            app.mirror_selected(axis, a.f(&["mt"], 0.001))
        }
        "polyDelEdge" => {
            app.delete_edges();
            Ok(String::new())
        }
        "polySelectEdgeLoop" => {
            app.select_edge_loop(false);
            Ok(String::new())
        }
        "polySelectEdgeRing" => {
            app.select_edge_loop(true);
            Ok(String::new())
        }
        // ------------------------------------------------------------------ rigging
        "joint" => {
            if a.has("e") || a.has("edit") {
                let ids = targets(app, &a.positional)?;
                // -oj xyz (the only orientation supported): X down the bone, Y up.
                app.orient_joints(&ids);
                return Ok(String::new());
            }
            let radius = a.f(&["rad", "radius"], 0.5);
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_joint(pos_flag.unwrap_or(Vec3::ZERO), name.as_deref(), radius);
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "orientJoint" => {
            let ids = targets(app, &a.positional)?;
            app.orient_joints(&ids);
            Ok(String::new())
        }
        "ikHandle" => {
            if a.has("e") || a.has("edit") {
                let ids = targets(app, &a.positional)?;
                let stretch = a.get("stretch").map(|v| matches!(v, "on" | "true" | "1" | "yes"));
                let twist = a.get("twist").and_then(|v| v.parse().ok());
                for id in ids {
                    app.set_ik_options(id, stretch, twist)?;
                }
                return Ok(String::new());
            }
            let sel = app.sel.clone();
            let start = match a.get("sj") {
                Some(n) => app.scene.find_by_name(n).ok_or(format!("No object matches name: {n}"))?,
                None => *sel.first().ok_or("ikHandle: give -sj start -ee end or select the start and end joints")?,
            };
            let end = match a.get("ee") {
                Some(n) => app.scene.find_by_name(n).ok_or(format!("No object matches name: {n}"))?,
                None => *sel.last().ok_or("ikHandle: missing end joint")?,
            };
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_ik(start, end, name.as_deref())?;
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "poleVectorConstraint" => {
            let ids = targets(app, &a.positional)?;
            if ids.len() != 2 {
                return Err("poleVectorConstraint: give the pole object then the IK handle".into());
            }
            app.pole_vector(ids[0], ids[1])?;
            Ok(String::new())
        }
        "parentConstraint" | "pointConstraint" | "orientConstraint" | "scaleConstraint" | "aimConstraint" => {
            use crate::rig::ConstraintKind as K;
            let kind = match cmd.as_str() {
                "parentConstraint" => K::Parent,
                "pointConstraint" => K::Point,
                "orientConstraint" => K::Orient,
                "scaleConstraint" => K::Scale,
                _ => K::Aim,
            };
            let ids = targets(app, &a.positional)?;
            if a.has("rm") || a.has("remove") {
                app.remove_constraints(&ids);
                return Ok(String::new());
            }
            app.constrain(kind, &ids, a.has("mo") || a.has("maintainOffset"), aim_flag, up_flag)
        }
        "skinCluster" | "bindSkin" => {
            let ids = targets(app, &a.positional)?;
            let (joints, meshes) = rig_split(app, &ids);
            if a.has("ub") || a.has("unbind") {
                app.unbind_skin(&meshes);
                return Ok(String::new());
            }
            let mi = a.u(&["mi"], 4) as usize;
            app.bind_skin(&joints, &meshes, a.has("tsb"), mi)
        }
        "unbindSkin" => {
            let ids = targets(app, &a.positional)?;
            app.unbind_skin(&ids);
            Ok(String::new())
        }
        "gotoBindPose" => {
            let ids = if a.positional.is_empty() { vec![] } else { resolve(app, &a.positional)? };
            app.goto_bind_pose(&ids);
            Ok(String::new())
        }
        "circle" => {
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_control("circle", a.f(&["r", "radius"], 1.0), normal_flag, name.as_deref())?;
            if let Some(p) = pos_flag {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "controlShape" => {
            let shape = a.get("s").unwrap_or("circle").to_string();
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_control(&shape, a.f(&["r", "radius"], 1.0), normal_flag, name.as_deref())?;
            if let Some(p) = pos_flag {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "setDrivenKeyframe" => {
            let cd = a.get("cd").ok_or("setDrivenKeyframe: missing -cd driver.attribute")?;
            let (dn, dch) = split_attr(cd)?;
            let driver = app.scene.find_by_name(&dn).ok_or(format!("No object matches name: {dn}"))?;
            let driven = targets(app, &a.positional)?;
            let dv = a.get("dv").and_then(|v| v.parse().ok());
            let v = a.get("v").or(a.get("value")).and_then(|v| v.parse().ok());
            if let Some(at) = a.get("at").or(a.get("attribute")).filter(|at| Channel::parse(at).is_none()) {
                if let Some((&d, i)) = driven.first().and_then(|d| app.blend_index(*d, at).map(|i| (d, i))) {
                    return app.set_driven_blend_key(driver, dch, d, i, dv, v);
                }
            }
            let chans: Vec<Channel> = match a.get("at").or(a.get("attribute")) {
                Some("translate" | "t") => Channel::translate().to_vec(),
                Some("rotate" | "r") => Channel::rotate().to_vec(),
                Some("scale" | "s") => Channel::scale().to_vec(),
                Some(other) => vec![Channel::parse(other).ok_or(format!("Unknown attribute '{other}'"))?],
                None => return Err("setDrivenKeyframe: missing -at driven attribute".into()),
            };
            app.set_driven_key(driver, dch, &driven, &chans, dv, v)
        }
        "mirrorJoint" => {
            let toks: Vec<String> = rest.to_vec();
            let (mut search, mut replace) = (String::from("L_"), String::from("R_"));
            if let Some(i) = toks.iter().position(|t| t == "-sr" || t == "-searchReplace") {
                search = toks.get(i + 1).cloned().unwrap_or_default();
                replace = toks.get(i + 2).cloned().unwrap_or_default();
            }
            let axis = if toks.iter().any(|t| t == "-mxy" || t == "-mirrorXY") { 2 } else if toks.iter().any(|t| t == "-mxz" || t == "-mirrorXZ") { 1 } else { 0 };
            let skip: Vec<usize> = toks.iter().position(|t| t == "-sr" || t == "-searchReplace").map(|i| vec![i, i + 1, i + 2]).unwrap_or_default();
            let names: Vec<String> = toks.iter().enumerate().filter(|(i, t)| !skip.contains(i) && !t.starts_with('-')).map(|(_, t)| t.clone()).collect();
            let ids = targets(app, &names)?;
            let mut out = Vec::new();
            for id in ids {
                let r = app.mirror_joints(id, axis, &search, &replace)?;
                out.push(app.scene.node(r).unwrap().name.clone());
            }
            Ok(out.join(" "))
        }
        "mirrorSkinWeights" | "copySkinWeights" => {
            let toks: Vec<String> = rest.to_vec();
            let flag_val = |names: &[&str]| toks.iter().position(|t| names.iter().any(|n| t == &format!("-{n}"))).and_then(|i| toks.get(i + 1).cloned());
            let mode = flag_val(&["mm", "mirrorMode", "ax"]).unwrap_or_else(|| "YZ".into()).to_lowercase();
            let axis = match mode.as_str() { "x" | "yz" => 0, "y" | "xz" => 1, _ => 2 };
            let inverse = toks.iter().any(|t| t == "-mi" || t == "-mirrorInverse" || t == "-n2p");
            let skip_vals: Vec<usize> = toks.iter().enumerate().filter(|(_, t)| ["-mm", "-mirrorMode", "-ax"].contains(&t.as_str())).map(|(i, _)| i + 1).collect();
            let names: Vec<String> = toks.iter().enumerate().filter(|(i, t)| !t.starts_with('-') && !skip_vals.contains(i)).map(|(_, t)| t.clone()).collect();
            let ids = targets(app, &names)?;
            let meshes: Vec<NodeId> = ids.into_iter().filter(|id| app.scene.node(*id).is_some_and(|n| n.skin.is_some())).collect();
            if meshes.is_empty() {
                return Err("mirrorSkinWeights: select a skinned mesh".into());
            }
            let mut n = 0;
            for m in meshes {
                n += app.mirror_skin_weights(m, axis, !inverse)?;
            }
            Ok(format!("{n} vertices mirrored"))
        }
        "quickRig" => {
            let ids = targets(app, &a.positional)?;
            let mesh = ids.into_iter().find(|id| app.scene.node(*id).is_some_and(|n| n.mesh().is_some()));
            app.quick_rig(mesh)
        }
        "blendShape" => {
            if a.has("e") || a.has("edit") {
                if a.has("rm") || a.has("remove") {
                    let ids = targets(app, &a.positional)?;
                    app.remove_blend_shape(&ids);
                    return Ok(String::new());
                }
                let target = a.get("w").ok_or("blendShape -e: use -w target value base")?.to_string();
                let val: f32 = a.positional.first().and_then(|v| v.parse().ok()).ok_or("blendShape -e -w: missing value")?;
                let ids = targets(app, &a.positional[1..])?;
                let mut n = 0;
                for id in ids {
                    if let Some(i) = app.blend_index(id, &target) {
                        app.set_blend_weight(id, i, val, false);
                        n += 1;
                    }
                }
                return if n > 0 { Ok(String::new()) } else { Err(format!("blendShape: no target named '{target}'")) };
            }
            let ids = targets(app, &a.positional)?;
            if ids.len() < 2 {
                return Err("blendShape: give target mesh(es) then the base mesh".into());
            }
            let (base, tg) = ids.split_last().unwrap();
            app.create_blend_shape(tg, *base)
        }
        "cluster" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
                app.set_comp_mode(crate::app::CompMode::Object);
            }
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_cluster(name.as_deref(), a.f(&["fo", "falloff"], 0.0))?;
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        // --------------------------------------------------------------- UVs
        "polyPlanarProjection" | "polyProjection" | "polyCylindricalProjection" | "polySphericalProjection" | "polyAutoProjection" | "polyLayoutUV" | "u3dLayout"
        | "polyNormalizeUV" | "polyEditUV" | "polyFlipUV" => {
            use crate::uv_ops::Projection as P;
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
                if app.comp_mode != CompMode::Object {
                    app.set_comp_mode(CompMode::Object);
                }
                app.uv_sel.clear();
            }
            let n = match cmd.as_str() {
                "polyPlanarProjection" | "polyProjection" => {
                    let axis = match a.get("md").unwrap_or("b") {
                        "x" | "X" => 0,
                        "y" | "Y" => 1,
                        "z" | "Z" => 2,
                        "cylindrical" => return app.uv_project(P::Cylindrical).map(|n| format!("{n}")),
                        "spherical" => return app.uv_project(P::Spherical).map(|n| format!("{n}")),
                        _ => 3,
                    };
                    app.uv_project(P::Planar(axis))?
                }
                "polyCylindricalProjection" => app.uv_project(P::Cylindrical)?,
                "polySphericalProjection" => app.uv_project(P::Spherical)?,
                "polyAutoProjection" => app.uv_project(P::Automatic)?,
                "polyLayoutUV" | "u3dLayout" => app.uv_layout()?,
                "polyNormalizeUV" => app.uv_normalize()?,
                "polyFlipUV" => {
                    let s = if a.get("ft").unwrap_or("0") == "1" { Vec2::new(1.0, -1.0) } else { Vec2::new(-1.0, 1.0) };
                    app.uv_transform(Vec2::ZERO, s, 0.0, None, false)?
                }
                _ => {
                    let t = Vec2::new(a.f(&["u"], 0.0), a.f(&["v"], 0.0));
                    let sc = Vec2::new(a.f(&["su"], 1.0), a.f(&["sv"], 1.0));
                    let pivot = (a.get("pu").is_some() || a.get("pv").is_some()).then(|| Vec2::new(a.f(&["pu"], 0.0), a.f(&["pv"], 0.0)));
                    app.uv_transform(t, sc, a.f(&["a", "angle"], 0.0), pivot, false)?
                }
            };
            Ok(format!("{n}"))
        }
        "uvEditor" | "TextureViewWindow" => {
            app.panels_uv_editor = true;
            Ok(String::new())
        }
        "uvChecker" | "textured" => {
            let v = app.active_view;
            let cur = if cmd == "uvChecker" { app.views[v].uv_checker } else { app.views[v].textured };
            let on = match a.positional.first().map(String::as_str) {
                Some("on" | "1" | "true") => true,
                Some("off" | "0" | "false") => false,
                _ => !cur,
            };
            if cmd == "uvChecker" { app.views[v].uv_checker = on } else { app.views[v].textured = on }
            Ok(String::new())
        }
        // -------------------------------------------------------- animation tools
        "keyTangent" => {
            use crate::scene::TanKind;
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &[&str]| toks.iter().position(|t| f.iter().any(|x| t == &format!("-{x}"))).and_then(|i| toks.get(i + 1)).cloned();
            let kind_s = val(&["ott", "outTangentType", "itt", "inTangentType", "tangent"]).ok_or("keyTangent: give -itt/-ott auto|spline|linear|flat|step")?;
            let kind = TanKind::parse(&kind_s).ok_or(format!("keyTangent: unknown tangent '{kind_s}'"))?;
            let frame: Option<f32> = val(&["t", "time"]).and_then(|v| v.parse().ok());
            let skip: Vec<usize> = toks.iter().enumerate().filter(|(_, t)| t.starts_with('-')).map(|(i, _)| i + 1).collect();
            let names: Vec<String> = toks.iter().enumerate().filter(|(i, t)| !t.starts_with('-') && !skip.contains(i)).map(|(_, t)| t.clone()).collect();
            if !names.is_empty() {
                app.sel = resolve(app, &names)?;
            }
            let saved = std::mem::take(&mut app.key_sel);
            if let Some(f) = frame {
                app.key_sel = app.effective_key_sel().into_iter().filter(|k| (k.frame - f).abs() < 1e-4).collect();
                if app.key_sel.is_empty() {
                    app.key_sel = saved;
                    return Err(format!("keyTangent: no keys at frame {f}"));
                }
            } else if names.is_empty() {
                app.key_sel = saved;
            }
            let n = app.set_key_tangents(kind);
            Ok(format!("{n}"))
        }
        "retarget" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            let src = val("-src").or(val("-source")).ok_or("retarget: -src sourceRoot -dst targetRoot [-s start] [-e end]")?;
            let dst = val("-dst").or(val("-target")).ok_or("retarget: missing -dst targetRoot")?;
            let s = app.scene.find_by_name(&src).ok_or(format!("No object matches name: {src}"))?;
            let d = app.scene.find_by_name(&dst).ok_or(format!("No object matches name: {dst}"))?;
            let start = val("-s").and_then(|v| v.parse().ok()).unwrap_or(app.scene.play_start);
            let end = val("-e").and_then(|v| v.parse().ok()).unwrap_or(app.scene.play_end);
            app.retarget(s, d, start, end)
        }
        "ghost" | "ghostSelected" | "motionTrail" => {
            if !a.positional.is_empty() && !matches!(a.positional[0].as_str(), "on" | "off") {
                app.sel = resolve(app, &a.positional)?;
            }
            let want = a.positional.iter().find(|p| matches!(p.as_str(), "on" | "off")).map(|p| p == "on");
            let trail = cmd == "motionTrail";
            let ids = app.sel.clone();
            for id in ids {
                let set = if trail { &mut app.trails } else { &mut app.ghosts };
                let on = want.unwrap_or(!set.contains(&id));
                if on { set.insert(id) } else { set.remove(&id) };
            }
            if let Some(v) = a.get("st").or(a.get("step")).and_then(|v| v.parse().ok()) {
                app.ghost_settings.step = v;
            }
            Ok(String::new())
        }
        "poseLibrary" | "pose" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            let flag = |f: &str| toks.iter().any(|t| t == f);
            if let Some(n) = val("-save") {
                let c = app.save_pose(&n, flag("-h") || flag("-hierarchy"))?;
                return Ok(format!("pose '{n}' saved ({c} nodes)"));
            }
            if let Some(n) = val("-apply") {
                let w = val("-w").and_then(|v| v.parse().ok()).unwrap_or(1.0);
                let c = app.apply_pose(&n, w, flag("-key") || flag("-k"), flag("-mirror") || flag("-m"))?;
                return Ok(format!("{c} nodes posed"));
            }
            if let Some(n) = val("-delete") {
                app.delete_pose(&n);
                return Ok(String::new());
            }
            if flag("-list") || toks.is_empty() {
                app.panels_pose_lib = true;
                return Ok(app.scene.poses.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(" "));
            }
            Err("poseLibrary: -save name [-h] | -apply name [-w 0.5] [-key] [-mirror] | -delete name | -list".into())
        }
        "graphEditor" => {
            app.panels.graph_editor = true;
            Ok(String::new())
        }
        "dopeSheet" | "dopeSheetEditor" => {
            app.panels_dope_sheet = true;
            Ok(String::new())
        }
        "copyKey" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
                app.key_sel.clear();
            }
            Ok(format!("{}", app.copy_keys()))
        }
        "pasteKey" => {
            if !a.positional.is_empty() {
                app.sel = resolve(app, &a.positional)?;
            }
            let t = a.f(&["t", "time"], app.scene.time);
            Ok(format!("{}", app.paste_keys(t)))
        }
        "polyMapCut" | "polyMapSew" | "polyMapSewMove" | "u3dUnfold" | "unfold" | "polyUnfold" => {
            // Optional: -e edgeIndex... selects those edges of the (named or selected) mesh.
            let toks: Vec<String> = rest.to_vec();
            let mut eidx: Vec<u32> = Vec::new();
            let mut objs: Vec<String> = Vec::new();
            let mut in_e = false;
            for t in &toks {
                if t == "-e" || t == "-edges" {
                    in_e = true;
                } else if in_e && let Ok(v) = t.parse::<u32>() {
                    eidx.push(v);
                } else if !t.starts_with('-') {
                    in_e = false;
                    objs.push(t.clone());
                }
            }
            if !objs.is_empty() {
                app.sel = resolve(app, &objs)?;
            }
            if !eidx.is_empty() {
                let id = *app.sel.first().ok_or("select a mesh")?;
                let all = app.scene.node(id).and_then(|n| n.mesh()).map(|m| m.edges()).ok_or("not a polygon mesh")?;
                let edges: Vec<(u32, u32)> = eidx.iter().filter_map(|i| all.get(*i as usize).copied()).collect();
                app.select_edges(id, &edges, false);
            }
            let r = match cmd.as_str() {
                "polyMapCut" => app.uv_cut().map(|n| format!("{n} UVs split")),
                "polyMapSew" | "polyMapSewMove" => app.uv_sew().map(|n| format!("{n} edges sewn")),
                _ => app.uv_unfold().map(|n| format!("{n} shells unfolded")),
            };
            r
        }
        "lightRig" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            if toks.iter().any(|t| t == "-list") {
                return Ok(crate::recipes::LIGHT_PRESETS.iter().map(|p| format!("{} — {}", p.0, p.1)).collect::<Vec<_>>().join("\n"));
            }
            let preset = val("-preset").or(val("-p")).or_else(|| toks.iter().find(|t| !t.starts_with('-')).cloned()).unwrap_or_else(|| "studio".into());
            let scale = val("-i").or(val("-intensity")).and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
            let mut objs: Vec<String> = Vec::new();
            let mut i = 0;
            while i < toks.len() {
                let t = &toks[i];
                if matches!(t.as_str(), "-preset" | "-p" | "-i" | "-intensity" | "-target" | "-t") {
                    if matches!(t.as_str(), "-target" | "-t") {
                        i += 1;
                        while i < toks.len() && !toks[i].starts_with('-') {
                            objs.push(toks[i].clone());
                            i += 1;
                        }
                        continue;
                    }
                    i += 2;
                    continue;
                }
                i += 1;
            }
            let ids = if objs.is_empty() { Vec::new() } else { resolve(app, &objs)? };
            app.light_rig(&preset, &ids, scale)
        }
        "materialPreset" | "shaderPreset" => {
            let toks: Vec<String> = rest.to_vec();
            if toks.iter().any(|t| t == "-list") {
                return Ok(crate::recipes::MATERIAL_PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(" "));
            }
            let (mut preset, mut name, mut objs) = (None, None, Vec::new());
            let mut i = 0;
            while i < toks.len() {
                match toks[i].as_str() {
                    "-p" | "-preset" => {
                        preset = toks.get(i + 1).cloned();
                        i += 2;
                    }
                    "-n" | "-name" => {
                        name = toks.get(i + 1).cloned();
                        i += 2;
                    }
                    "-rgb" => i += 4,
                    t if !t.starts_with('-') => {
                        if preset.is_none() && crate::recipes::material_preset(t).is_some() && app.scene.find_by_name(t).is_none() {
                            preset = Some(t.to_string());
                        } else {
                            objs.push(t.to_string());
                        }
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            let preset = preset.ok_or("materialPreset -p gold|chrome|glass|wood|... [-rgb r g b] [-n name] [objects]")?;
            let ids = if objs.is_empty() { Vec::new() } else { resolve(app, &objs)? };
            app.apply_material_preset(&preset, rgb_flag.map(|c| c.to_array()), name.as_deref(), &ids)
        }
        "rigAnimate" | "animateRig" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            if toks.iter().any(|t| t == "-list") {
                return Ok(crate::recipes::ANIMS.iter().map(|p| format!("{} — {}", p.0, p.1)).collect::<Vec<_>>().join("\n"));
            }
            let anim = val("-anim").or(val("-a")).unwrap_or_else(|| "walk".into());
            let start = val("-s").or(val("-start")).and_then(|v| v.parse::<f32>().ok()).unwrap_or(app.scene.play_start);
            let default_len = match anim.as_str() {
                "jump" => 30.0,
                "run" => 72.0,
                _ => 96.0,
            };
            let end = val("-e").or(val("-end")).and_then(|v| v.parse::<f32>().ok()).unwrap_or(start + default_len);
            let travel = !matches!(val("-travel").as_deref(), Some("off" | "0" | "false"));
            let mut mesh = None;
            let mut i = 0;
            while i < toks.len() {
                if toks[i].starts_with('-') {
                    i += 2;
                    continue;
                }
                mesh = Some(app.scene.find_by_name(&toks[i]).ok_or_else(|| format!("No object matches name: {}", toks[i]))?);
                i += 1;
            }
            app.rig_and_animate(&anim, mesh, start, end, travel)
        }
        "renderCheck" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            let num = |f: &str, d: u32| val(f).and_then(|v| v.parse::<u32>().ok()).unwrap_or(d);
            let rc = app.render_check(val("-cam").as_deref(), num("-x", 320).clamp(16, 2048), num("-y", 180).clamp(16, 2048), num("-spp", 16).clamp(1, 1024))?;
            let mut out = rc.report();
            if !rc.fixes.is_empty() {
                out.push_str(&format!("\nSuggested: {}", rc.fixes.join("; ")));
            }
            app.render.open = true;
            Ok(out)
        }
        "render" | "renderSettings" | "renderView" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            let num = |f: &str| val(f).and_then(|v| v.parse::<f32>().ok());
            let flag = |f: &str| toks.iter().any(|t| t == f);
            let mut s = app.render.settings.clone();
            if let Some(v) = num("-x").or(num("-w")) {
                s.width = v.clamp(8.0, 8192.0) as u32;
            }
            if let Some(v) = num("-y").or(num("-h")) {
                s.height = v.clamp(8.0, 8192.0) as u32;
            }
            if let Some(v) = num("-spp").or(num("-samples")) {
                s.spp = v.clamp(1.0, 100_000.0) as u32;
            }
            if let Some(v) = num("-b").or(num("-bounces")) {
                s.bounces = v.clamp(0.0, 64.0) as u32;
            }
            if let Some(v) = num("-env") {
                s.env = v.max(0.0);
            }
            if let Some(v) = num("-exp").or(num("-exposure")) {
                s.exposure = v.clamp(-20.0, 20.0);
            }
            if let Some(v) = num("-clamp") {
                s.clamp = v.max(0.01);
            }
            if let Some(v) = val("-tm").or(val("-tonemap")) {
                s.tonemap = crate::render_view::parse_tonemap(&v).ok_or_else(|| format!("Unknown tone map '{v}' (srgb, filmic)"))?;
            }
            if flag("-region") || flag("-reg") {
                let i = toks.iter().position(|t| t == "-region" || t == "-reg").unwrap();
                let nums: Vec<f32> = toks.iter().skip(i + 1).take(4).filter_map(|t| t.parse().ok()).collect();
                s.region = if nums.len() == 4 { Some([nums[0], nums[1], nums[2], nums[3]]) } else { None };
            }
            if let Some(v) = val("-dn").or(val("-denoise")) {
                s.denoise = matches!(v.as_str(), "on" | "1" | "true");
            }
            if let Some(v) = val("-bg").or(val("-background")) {
                s.env_background = matches!(v.as_str(), "env" | "sky" | "environment" | "on" | "1");
            }
            let cam = val("-cam").or(val("-camera"));
            let quote = |p: String| p.trim_matches('"').to_string();
            if cmd == "renderSettings" {
                app.render.settings = s.clone();
                if let Some(c) = &cam {
                    let id = app.scene.find_by_name(c).ok_or_else(|| format!("No object matches name: {c}"))?;
                    app.render.camera = if app.is_camera(id) { crate::render_view::RenderCam::Node(id) } else { return Err(format!("{c} is not a camera")) };
                } else if flag("-activeView") || flag("-av") {
                    app.render.camera = crate::render_view::RenderCam::ActiveView;
                }
                return Ok(format!("{}x{} {} spp, {} bounces", s.width, s.height, s.spp, s.bounces));
            }
            if cmd == "renderView" {
                if flag("-stop") {
                    app.stop_render();
                    return Ok(String::new());
                }
                if let Some(p) = val("-save").map(quote) {
                    if flag("-passes") {
                        return Ok(app.save_render_passes(&p)?.join(" "));
                    }
                    app.save_render(&p)?;
                    return Ok(p);
                }
                if flag("-ipr") {
                    app.render.ipr = !matches!(val("-ipr").as_deref(), Some("off") | Some("0"));
                }
                app.render.open = true;
                return Ok(String::new());
            }
            // render
            if flag("-seq") || flag("-sequence") {
                let st = num("-st").or(num("-start")).unwrap_or(app.scene.play_start) as i32;
                let en = num("-e").or(num("-end")).unwrap_or(app.scene.play_end) as i32;
                let out = val("-o").map(quote).ok_or("render -seq needs -o C:/path/name")?;
                if cfg!(test) {
                    let n = app.render_sequence_now(cam.as_deref(), s, st, en, &out)?;
                    return Ok(format!("{n} frames"));
                }
                app.render.settings = s;
                app.start_sequence(st, en, out);
                return Ok(format!("rendering frames {st}-{en}"));
            }
            if let Some(out) = val("-o").map(quote) {
                let job = app.render_now(cam.as_deref(), s.clone())?;
                if flag("-passes") {
                    let files = job.save_passes(std::path::Path::new(&out), s.exposure, s.tonemap)?;
                    return Ok(files.join(" "));
                }
                job.save_png(std::path::Path::new(&out), s.exposure, s.tonemap)?;
                let mut job = job;
                return Ok(format!("{}x{} {} spp in {:.2}s → {out}", s.width, s.height, s.spp, job.elapsed().as_secs_f32()));
            }
            app.render.settings = s;
            if let Some(c) = &cam {
                let id = app.scene.find_by_name(c).ok_or_else(|| format!("No object matches name: {c}"))?;
                app.render.camera = crate::render_view::RenderCam::Node(id);
            }
            app.start_render();
            Ok(String::from("rendering"))
        }
        "lattice" | "ffd" => {
            let toks: Vec<String> = rest.to_vec();
            let flag = |f: &str| toks.iter().any(|t| t == f);
            if flag("-e") && (flag("-reset") || flag("-rs")) || flag("-remove") || flag("-rm") {
                let names: Vec<String> = toks.iter().filter(|t| !t.starts_with('-')).cloned().collect();
                let ids = targets(app, &names)?;
                if flag("-remove") || flag("-rm") {
                    app.remove_lattice_and_wrap(&ids);
                } else {
                    for id in ids {
                        app.reset_lattice(id);
                    }
                }
                return Ok(String::new());
            }
            let mut div = [2u32, 5, 2];
            let mut name = None;
            let mut objs = Vec::new();
            let mut i = 0;
            while i < toks.len() {
                match toks[i].as_str() {
                    "-dv" | "-divisions" => {
                        for k in 0..3 {
                            if let Some(v) = toks.get(i + 1 + k).and_then(|v| v.parse::<u32>().ok()) {
                                div[k] = v;
                            }
                        }
                        i += 4;
                        continue;
                    }
                    "-n" | "-name" => {
                        name = toks.get(i + 1).cloned();
                        i += 2;
                        continue;
                    }
                    t if !t.starts_with('-') => objs.push(t.to_string()),
                    _ => {}
                }
                i += 1;
            }
            if !objs.is_empty() {
                app.sel = resolve(app, &objs)?;
            }
            let id = app.create_lattice(div, name.as_deref())?;
            Ok(app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default())
        }
        "latticePoint" => {
            let toks: Vec<String> = rest.to_vec();
            let name = toks.first().ok_or("latticePoint name i j k x y z")?;
            let id = app.scene.find_by_name(name).ok_or_else(|| format!("No object matches name: {name}"))?;
            let n: Vec<f32> = toks.iter().skip(1).filter_map(|t| t.parse().ok()).collect();
            if n.len() < 6 {
                return Err("latticePoint name i j k x y z   (local coords, -0.5..0.5 at rest)".into());
            }
            app.set_lattice_point(id, [n[0] as u32, n[1] as u32, n[2] as u32], Vec3::new(n[3], n[4], n[5]))?;
            Ok(String::new())
        }
        "wrap" => {
            let toks: Vec<String> = rest.to_vec();
            let mut driver = None;
            let mut objs = Vec::new();
            let mut i = 0;
            while i < toks.len() {
                if toks[i] == "-d" || toks[i] == "-driver" {
                    driver = toks.get(i + 1).cloned();
                    i += 2;
                    continue;
                }
                if !toks[i].starts_with('-') {
                    objs.push(toks[i].clone());
                }
                i += 1;
            }
            let mut ids = targets(app, &objs)?;
            let drv = match driver {
                Some(d) => app.scene.find_by_name(&d).ok_or_else(|| format!("No object matches name: {d}"))?,
                None => ids.pop().ok_or("wrap: select the meshes to deform, then the influence mesh")?,
            };
            let n = app.create_wrap(drv, &ids)?;
            Ok(format!("{n} wrapped"))
        }
        "polySplit" | "polyMultiCut" => {
            // polySplit -ep edgeIndex t -ep edgeIndex t ... [mesh]
            let toks: Vec<String> = rest.to_vec();
            let mut pts = Vec::new();
            let mut objs = Vec::new();
            let mut i = 0;
            while i < toks.len() {
                if toks[i] == "-ep" || toks[i] == "-edgepoint" {
                    let e = toks.get(i + 1).and_then(|v| v.parse::<usize>().ok());
                    let t = toks.get(i + 2).and_then(|v| v.parse::<f32>().ok());
                    match (e, t) {
                        (Some(e), Some(t)) => pts.push((e, t)),
                        _ => return Err("polySplit -ep edgeIndex t ...".into()),
                    }
                    i += 3;
                    continue;
                }
                if !toks[i].starts_with('-') {
                    objs.push(toks[i].clone());
                }
                i += 1;
            }
            let id = *targets(app, &objs)?.first().ok_or("polySplit: select a mesh")?;
            let edges = app.scene.node(id).and_then(|n| n.mesh()).map(|m| m.edges()).ok_or("polySplit: not a polygon mesh")?;
            let mut cut = Vec::new();
            for (e, t) in pts {
                let ed = *edges.get(e).ok_or_else(|| format!("polySplit: edge {e} out of range ({} edges)", edges.len()))?;
                cut.push((id, ed, t));
            }
            app.cut_points = cut;
            let n = app.finish_multi_cut()?;
            Ok(format!("{n} faces split"))
        }
        "softSelect" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            if let Some(v) = val("-sse").or(val("-softSelectEnabled")) {
                app.soft_select = matches!(v.as_str(), "on" | "1" | "true");
            }
            if let Some(r) = val("-ssd").or(val("-softSelectDistance")).and_then(|v| v.parse::<f32>().ok()) {
                app.soft_radius = r.max(0.01);
            }
            Ok(format!("{} {}", app.soft_select, app.soft_radius))
        }
        "sculpt" | "sculptBrush" => {
            let toks: Vec<String> = rest.to_vec();
            let val = |f: &str| toks.iter().position(|t| t == f).and_then(|i| toks.get(i + 1)).cloned();
            if let Some(b) = val("-b").or(val("-brush")) {
                app.sculpt.brush = crate::sculpt::Brush::parse(&b).ok_or_else(|| format!("Unknown brush '{b}' (sculpt, smooth, grab, flatten, inflate, pinch)"))?;
            }
            if let Some(s) = val("-s").or(val("-strength")).and_then(|v| v.parse::<f32>().ok()) {
                app.sculpt.strength = s.clamp(0.0, 1.0);
            }
            if let Some(r) = val("-r").or(val("-radius")).and_then(|v| v.parse::<f32>().ok()) {
                app.sculpt.radius_px = r.clamp(2.0, 500.0);
            }
            if let Some(v) = val("-sym").or(val("-symmetry")) {
                app.sculpt.symmetry_x = matches!(v.as_str(), "on" | "1" | "true" | "x");
            }
            if let Some(v) = val("-inv").or(val("-invert")) {
                app.sculpt.invert = matches!(v.as_str(), "on" | "1" | "true");
            }
            app.tool = Tool::Sculpt;
            Ok(app.sculpt.brush.name().to_string())
        }
        "setColor" | "polyColor" => {
            let (color, names): ([f32; 3], Vec<String>) = match a.positional.first() {
                Some(first) if first.parse::<f32>().is_ok() => {
                    let nums: Vec<f32> = a.positional.iter().take(3).filter_map(|v| v.parse().ok()).collect();
                    if nums.len() != 3 {
                        return Err("setColor: expected a colour name or 3 numbers".into());
                    }
                    let scale = if nums.iter().any(|v| *v > 1.0) { 1.0 / 255.0 } else { 1.0 };
                    ([nums[0] * scale, nums[1] * scale, nums[2] * scale], a.positional[3..].to_vec())
                }
                Some(name) => {
                    let hex = name.trim_start_matches('#');
                    let c = named_color(name).or_else(|| {
                        (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok()).flatten().map(|v| [((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0])
                    });
                    (c.ok_or(format!("setColor: unknown colour '{name}'"))?, a.positional[1..].to_vec())
                }
                None => return Err("setColor: missing colour".into()),
            };
            let ids = targets(app, &names)?;
            if ids.is_empty() {
                return Err("setColor: nothing selected".into());
            }
            app.checkpoint();
            for id in ids {
                if let Some(n) = app.scene.node_mut(id) {
                    n.color = color;
                    // A per-object colour replaces a shared material (use the Hypershade to edit those).
                    n.material = None;
                    if let Shape::Light(l) = &mut n.shape {
                        l.color = color;
                    }
                }
            }
            Ok(String::new())
        }
        // --------------------------------------------------------------- look dev
        "shadingNode" | "createMaterial" => {
            let kind_s = a.positional.first().map(String::as_str).unwrap_or("lambert");
            let kind = crate::look::MatKind::parse(kind_s).ok_or(format!("shadingNode: unsupported type '{kind_s}' (lambert, blinn, phong, standardSurface)"))?;
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_material(kind, name.as_deref());
            if let Some(c) = rgb_flag {
                app.material_mut(id).unwrap().color = c.to_array();
            }
            Ok(app.material(id).unwrap().name.clone())
        }
        "hyperShade" | "assignMaterial" | "sets" => {
            // hyperShade -assign mat [objects] | assignMaterial mat [objects] | sets -e -forceElement matSG objects
            let toks: Vec<String> = rest.to_vec();
            let flag_pos = toks.iter().position(|t| t == "-assign" || t == "-a" || t == "-forceElement" || t == "-fe");
            let (mat_name, objs): (String, Vec<String>) = match (cmd.as_str(), flag_pos) {
                (_, Some(i)) => {
                    let m = toks.get(i + 1).cloned().ok_or("missing material name")?;
                    let objs = toks.iter().enumerate().filter(|(j, t)| *j != i && *j != i + 1 && !t.starts_with('-')).map(|(_, t)| t.clone()).collect();
                    (m, objs)
                }
                ("assignMaterial", None) => {
                    let p: Vec<String> = toks.iter().filter(|t| !t.starts_with('-')).cloned().collect();
                    (p.first().cloned().ok_or("assignMaterial: missing material")?, p[1..].to_vec())
                }
                ("hyperShade", None) if toks.iter().any(|t| t == "-window" || t.is_empty()) || toks.is_empty() => {
                    app.panels_hypershade = true;
                    app.hs_selected = app.lead().and_then(|l| app.scene.node(l)).and_then(|n| n.material);
                    return Ok(String::new());
                }
                _ => return Err(format!("{cmd}: use -assign material [objects]")),
            };
            let ids = targets(app, &objs)?;
            if mat_name == "lambert1" || mat_name == "initialShadingGroup" {
                app.assign_material(None, &ids);
                return Ok(String::new());
            }
            let mid = app.material_by_name(&mat_name).ok_or(format!("No material named '{mat_name}'"))?;
            let n = app.assign_material(Some(mid), &ids);
            if n == 0 {
                return Err("Assign material: select polygon objects".into());
            }
            Ok(format!("{mat_name} assigned to {n} object(s)"))
        }
        "directionalLight" | "pointLight" | "spotLight" | "ambientLight" | "areaLight" => {
            use crate::look::LightKind as L;
            let kind = match cmd.as_str() {
                "directionalLight" => L::Directional,
                "pointLight" => L::Point,
                "spotLight" => L::Spot,
                "areaLight" => L::Area,
                _ => L::Ambient,
            };
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_light(kind, name.as_deref());
            let n = app.scene.node_mut(id).unwrap();
            if let Shape::Light(l) = &mut n.shape {
                l.intensity = a.f(&["i", "intensity"], 1.0);
                if let Some(c) = rgb_flag {
                    l.color = c.to_array();
                }
                l.cone = a.f(&["ca", "coneAngle"], l.cone);
                l.penumbra = a.f(&["pen", "penumbra"], l.penumbra);
                l.shadows = a.has("sh") && !matches!(a.get("sh"), Some("off" | "0" | "false"));
            }
            if let Some(p) = pos_flag {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
            if let Some(r) = rot_flag {
                app.scene.node_mut(id).unwrap().rotate = r;
            }
            if let Some(t) = aim_flag {
                app.aim_at(id, t);
            }
            if kind == L::Area {
                let (w, h) = (a.f(&["w", "width"], 2.0), a.f(&["h", "height"], 2.0));
                app.scene.node_mut(id).unwrap().scale = Vec3::new((w * 0.5).max(1e-3), (h * 0.5).max(1e-3), 1.0);
            }
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "camera" => {
            let name = a.get("n").or(a.get("name")).map(String::from);
            let id = app.create_camera(name.as_deref(), a.f(&["fl", "focalLength"], 35.0));
            if let Some(p) = pos_flag {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
            if let Some(r) = rot_flag {
                app.scene.node_mut(id).unwrap().rotate = r;
            }
            if let Some(t) = aim_flag {
                app.aim_at(id, t);
            }
            Ok(app.scene.node(id).unwrap().name.clone())
        }
        "lookThru" | "lookThrough" => {
            let v = app.active_view;
            match a.positional.first().map(String::as_str) {
                None | Some("persp" | "top" | "front" | "side") => {
                    app.look_through(v, None)?;
                    if let Some(k) = a.positional.first() {
                        let _ = run(app, &format!("viewSet -{k}"));
                    }
                }
                Some(n) => {
                    let id = app.scene.find_by_name(n).ok_or(format!("No object matches name: {n}"))?;
                    app.look_through(v, Some(id))?;
                }
            }
            Ok(String::new())
        }
        "xray" | "displayXRay" => {
            let v = app.active_view;
            let on = match a.positional.first().map(String::as_str) {
                Some("on" | "1" | "true") => true,
                Some("off" | "0" | "false") => false,
                _ => !app.views[v].xray,
            };
            app.views[v].xray = on;
            Ok(String::new())
        }
        "lighting" | "displayLights" => {
            // lighting -all|-default [-shadows on|off]
            let v = app.active_view;
            if a.has("all") || a.has("a") {
                app.views[v].all_lights = true;
            }
            if a.has("default") || a.has("d") {
                app.views[v].all_lights = false;
            }
            if let Some(sv) = rest.iter().position(|t| t == "-shadows").and_then(|i| rest.get(i + 1)) {
                app.views[v].shadows = matches!(sv.as_str(), "on" | "1" | "true");
                if app.views[v].shadows {
                    app.views[v].all_lights = true;
                }
            }
            Ok(String::new())
        }
        "hide" => {
            let ids = targets(app, &a.positional)?;
            app.checkpoint();
            for id in ids {
                if let Some(n) = app.scene.node_mut(id) {
                    n.visible = false;
                }
            }
            Ok(String::new())
        }
        "showHidden" => {
            let ids: Vec<NodeId> = if a.has("all") || a.positional.is_empty() && app.sel.is_empty() {
                app.scene.nodes.iter().map(|n| n.id).collect()
            } else {
                targets(app, &a.positional)?
            };
            app.checkpoint();
            for id in ids {
                if let Some(n) = app.scene.node_mut(id) {
                    n.visible = true;
                }
            }
            Ok(String::new())
        }
        "cutKey" => {
            let ids = targets(app, &a.positional)?;
            app.checkpoint();
            for id in ids {
                if let Some(n) = app.scene.node_mut(id) {
                    n.anim.clear();
                }
            }
            Ok(String::new())
        }
        "selectFaces" => {
            let nums: Vec<f32> = a.positional.iter().filter_map(|p| p.parse().ok()).collect();
            if nums.len() != 3 {
                return Err("selectFaces: expected a direction x y z".into());
            }
            let dir = Vec3::new(nums[0], nums[1], nums[2]).normalize_or_zero();
            let tol = a.get("tol").and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.7);
            let names: Vec<String> = a.positional.iter().filter(|p| p.parse::<f32>().is_err()).cloned().collect();
            if !names.is_empty() {
                app.sel = resolve(app, &names)?;
            }
            app.set_comp_mode(CompMode::Face);
            app.comp_sel.clear();
            let mut count = 0;
            for id in app.comp_targets() {
                let w = app.scene.world_matrix(id);
                let nm = glam::Mat3::from_mat4(w).inverse().transpose();
                if let Some(m) = app.scene.node(id).and_then(|n| n.mesh()) {
                    for f in 0..m.face_count() {
                        if (nm * m.face_normal(f)).normalize_or_zero().dot(dir) >= tol {
                            app.comp_sel.insert((id, f as u32));
                            count += 1;
                        }
                    }
                }
            }
            Ok(format!("{count} face(s) selected"))
        }
        "viewSet" => {
            use crate::camera::CamKind;
            let kind = if a.has("top") {
                CamKind::Top
            } else if a.has("front") {
                CamKind::Front
            } else if a.has("side") || a.has("rightSide") {
                CamKind::Side
            } else {
                CamKind::Persp
            };
            let v = app.active_view;
            app.set_view_kind(v, kind);
            app.frame(v, false);
            Ok(kind.name().to_string())
        }
        "viewFit" => {
            let v = app.active_view;
            let all = a.has("all") || app.sel.is_empty();
            app.frame(v, !all);
            Ok(String::new())
        }
        "panelLayout" => {
            app.quad = a.has("quad") || a.has("four");
            Ok(String::new())
        }
        "displayMode" => {
            use crate::app::Shading;
            let v = app.active_view;
            app.views[v].shading = if a.has("wire") || a.has("wireframe") {
                Shading::Wireframe
            } else if a.has("wos") {
                Shading::WireOnShaded
            } else {
                Shading::Shaded
            };
            Ok(String::new())
        }
        other => Err(format!("Cannot find procedure \"{other}\".")),
    }
}

fn names(app: &App, ids: &[NodeId]) -> String {
    ids.iter().filter_map(|id| app.scene.node(*id)).map(|n| n.name.as_str()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_basics() {
        let mut app = App::new();
        assert_eq!(execute(&mut app, "polyCube -w 2 -n box").unwrap(), "box");
        execute(&mut app, "polySphere; move 1 2 3").unwrap();
        let sph = app.scene.find_by_name("pSphere1").unwrap();
        assert_eq!(app.scene.node(sph).unwrap().translate, Vec3::new(1.0, 2.0, 3.0));
        execute(&mut app, "move -r 1 0 0 pSphere1").unwrap();
        assert_eq!(execute(&mut app, "getAttr pSphere1.tx").unwrap(), "2");
        execute(&mut app, "select p*").unwrap();
        assert_eq!(app.sel, vec![sph]);
        execute(&mut app, "select box pSphere1; parent").unwrap();
        let bx = app.scene.find_by_name("box").unwrap();
        assert_eq!(app.scene.node(bx).unwrap().parent, Some(sph));
        execute(&mut app, "currentTime 1; setKeyframe -at tx pSphere1; currentTime 10; setAttr pSphere1.tx 10; setKeyframe -at tx pSphere1").unwrap();
        execute(&mut app, "currentTime 1").unwrap();
        assert_eq!(execute(&mut app, "getAttr pSphere1.translateX").unwrap(), "2");
        execute(&mut app, "undo").unwrap();
        assert!(execute(&mut app, "bogus").is_err());
        assert!(wildcard("p*1", "pCube1") && !wildcard("p*2", "pCube1") && wildcard("*Cube*", "pCube1"));
    }

    #[test]
    fn ai_facing_commands() {
        let mut app = App::new();
        for m in ["polyCube -w 4 -h 0.2 -d 2 -n table_top", "move 0 2 0", "setColor wood", "polyCube -w 0.2 -h 2 -d 0.2 -n leg_1", "move 1.8 1 0.8", "select table_top leg_1", "group", "rename table"] {
            execute(&mut app, m).unwrap();
        }
        let t = app.scene.find_by_name("table").unwrap();
        let top = app.scene.find_by_name("table_top").unwrap();
        assert_eq!(app.scene.node(top).unwrap().parent, Some(t));
        assert_eq!(app.scene.node(top).unwrap().color, named_color("wood").unwrap());
        execute(&mut app, "setColor 255 128 0 leg_1").unwrap();
        let leg = app.scene.find_by_name("leg_1").unwrap();
        assert!((app.scene.node(leg).unwrap().color[1] - 0.502).abs() < 0.01);
        assert!(execute(&mut app, "setColor plaid").is_err());
        // keyframes with explicit time and value
        execute(&mut app, "setKeyframe -t 1 -at ty -v 0 leg_1; setKeyframe -t 11 -at ty -v 10 leg_1; currentTime 6").unwrap();
        assert!((app.scene.node(leg).unwrap().translate.y - 5.0).abs() < 0.01);
        // face selection by direction + extrude distance
        execute(&mut app, "selectFaces 0 1 0 table_top").unwrap();
        assert_eq!(app.comp_sel.len(), 1);
        execute(&mut app, "polyExtrudeFacet -ltz 0.5").unwrap();
        assert_eq!(app.scene.node(top).unwrap().mesh().unwrap().face_count(), 10);
        execute(&mut app, "selectMode -o; select -d leg_1; viewSet -top; displayMode -wire; panelLayout -quad; hide table; showHidden -all").unwrap();
        assert!(app.quad && app.scene.node(t).unwrap().visible);
        // a failing script is remembered for "Fix with AI"
        assert!(execute(&mut app, "polyCube; polyCub").is_err());
        assert_eq!(app.last_mel_error.as_ref().unwrap().1, "Cannot find procedure \"polyCub\".");
    }

    #[test]
    fn batch_is_one_undo_step() {
        let mut app = App::new();
        app.begin_batch();
        execute(&mut app, "polyCube; polySphere; move 1 2 3").unwrap();
        app.end_batch();
        assert_eq!(app.scene.nodes.len(), 2);
        app.undo();
        assert_eq!(app.scene.nodes.len(), 0);
    }
}