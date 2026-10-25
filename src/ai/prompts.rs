//! System prompts. The AI's only way to change the scene is MEL, which runs through the same
//! interpreter as the command line, so every AI action is validated, undoable and logged.

use serde_json::{Value, json};

pub const MEL_REFERENCE: &str = r#"MEL COMMANDS (one statement per array element, no trailing ';' needed):
  polyCube [-w W -h H -d D -sx N -sy N -sz N] [-n name]      (default 1x1x1, centered on origin)
  polySphere [-r R -sa N -sh N] [-n name]                     (default radius 1)
  polyCylinder [-r R -h H -sa N] [-n name]                    (default r 1, height 2, centered)
  polyCone [-r R -h H -sa N] [-n name]                        (default r 1, height 2, centered)
  polyPlane [-w W -h H -sx N -sy N] [-n name]                 (lies flat in XZ)
  polyTorus [-r R -sr R] [-n name]                            (ring in XZ, default r 1, section 0.5)
  spaceLocator [-n name]
  Creating an object SELECTS it, so following move/rotate/scale/setColor apply to it.
  select [-add|-tgl|-d] names...   select -cl   select -all     (wildcards like "leg*" work)
  move|rotate|scale [-r] x y z [names]    absolute by default; -r = relative (scale -r multiplies)
  setAttr node.attr value   attrs: tx ty tz rx ry rz sx sy sz v      getAttr node.attr
  setColor <name|r g b> [names]     colour names: red green blue yellow orange purple pink white black
                                    grey brown gold silver cyan magenta skin wood sky grass navy teal
  rename [old] new    duplicate [names]    delete [names]    group [names]   (group selects the new group)
  parent child... parent      parent -w child...      hide [names]   showHidden [-all]
  polySmooth   polyTriangulate   polyUnite (combine)   polySeparate   makeIdentity (freeze)   delete -ch
  selectFaces x y z [-tol 0.7] [name]   (face mode, selects faces whose normal points along x y z)
  polyExtrudeFacet [-ltz distance]      (extrudes selected faces)     selectMode -o|-v|-e|-f
  polySplitRing -dir x y z [-wt 0.5] mesh   (insert an edge loop cutting the edges parallel to x y z, e.g.
                                       -dir 0 1 0 adds a horizontal loop around a cylinder at height fraction wt)
  polyBevel -o width [mesh]   polyMirrorFace -ax x mesh (mirror + weld)   polyMergeVertex -d 0.001 mesh
  polyBridge (two selected faces)   polyDelEdge   polySelectEdgeLoop   polySelectEdgeRing
  currentTime N    setKeyframe [-t frame] [-at attr] [-v value] [names]    cutKey [names]
  playbackOptions -min N -max N [-fps N]    play    stop
  viewSet -persp|-top|-front|-side    viewFit [-all]    panelLayout -single|-quad
  displayMode -wire|-shaded|-wos    setToolTo select|move|rotate|scale    undo    redo
RIGGING:
  joint -p x y z [-n name] [-rad r]   world position; if a joint is selected the new joint becomes its CHILD,
                                      so consecutive joint commands build a chain. Use "select -cl" to start a new chain.
  joint -e -oj xyz root               orient the chain (X down the bone) — do this after building each chain
  ikHandle -sj startJoint -ee endJoint [-n name]     (chain of 3+ joints, e.g. shoulder -> elbow -> wrist)
  poleVectorConstraint poleObject ikHandle           (knee/elbow points toward the pole)
  parentConstraint|pointConstraint|orientConstraint|scaleConstraint|aimConstraint [-mo] driver... target
                                      drivers FIRST, constrained object LAST; -mo keeps the current offset
  skinCluster [-mi 4] rootJoint mesh  smooth bind (whole joint hierarchy, closest-distance weights)
  skinCluster -e -ub mesh   gotoBindPose
  circle -r R -nr x y z [-p x y z] -n name     controlShape -s circle|square|cross|arrow|cube|sphere -r size -n name
  setDrivenKeyframe -cd driver.attr -dv driverValue -at attr -v value driven   (call once per pose; attr may be a blend target)
  ikHandle -e -stretch on|off -twist deg handle      stretchy IK / twist along the chain
  mirrorJoint -myz -sr L_ R_ L_shoulder              mirror a chain to the other side (renames L_ -> R_)
  copySkinWeights -mm YZ mesh                        mirror skin weights +X -> -X (add -mirrorInverse for -X -> +X)
  quickRig mesh                                      ONE-CLICK BIPED: skeleton (hips, spine1-2, chest, neck, head, L_/R_ clavicle,
                                      shoulder, elbow, wrist, hip, knee, ankle, toe), arm/leg IK with poles, controls (root_ctrl, cog_ctrl,
                                      chest_ctrl, head_ctrl, L_/R_hand_ctrl, L_/R_foot_ctrl, L_/R_clavicle_ctrl) and smooth bind.
                                      The mesh must stand on Y facing +Z. Use this whenever asked to "rig a character".
DEFORMERS:
  blendShape [-n name] target... base     targets are meshes with the SAME vertex count (usually duplicates edited)
  setAttr base.targetName 0.7             set a blend-shape weight (0..1);  setKeyframe -t 10 -at targetName -v 1 base
  blendShape -e -w targetName 0.5 base    blendShape -e -rm base
  cluster [-n name] [-fo falloff] [mesh]  cluster on the selected vertices (or whole mesh); move/rotate the handle to deform
LOOK DEV (materials are SHARED: editing one changes every object it is assigned to):
  shadingNode -asShader lambert|blinn|phong|standardSurface -n name [-rgb r g b]
  setAttr mat.color r g b    mat.specular 0.5   mat.roughness 0.3   mat.metalness 1   mat.incandescence r g b   mat.transparency 0.5
  hyperShade -assign mat obj1 obj2 ...      (setColor gives an object its own colour and removes its material)
  directionalLight|pointLight|spotLight|ambientLight -n name [-i intensity] [-rgb r g b] [-p x y z] [-aim x y z] [-rot x y z] [-ca cone] [-sh on]
      lights shine down their -Z axis; use -aim to point them; "-sh on" casts shadows (directional/spot)
  areaLight -n softbox -w 2 -h 1 -i 3 -p x y z -aim x y z   soft rectangular light (soft shadows in renders; falls off with distance)
  setAttr light.intensity 2   light.color r g b   light.coneAngle 40   light.useDepthMapShadows 1
  camera -n name [-fl focalMM] -p x y z -aim x y z     lookThru name | lookThru persp
  lighting -all -shadows on   (viewport: use scene lights + shadows)    lighting -default    xray on|off
UVS & TEXTURES (primitives already have UVs; topology edits like bevel may drop them):
  polyAutoProjection obj | polyPlanarProjection -md x|y|z|b obj | polyCylindricalProjection obj | polySphericalProjection obj
  polyLayoutUV obj   polyNormalizeUV obj   polyEditUV -u 0.5 -v 0 [-su 2 -sv 2] [-a 90] obj   polyFlipUV -ft 0 obj
  setAttr mat.colorMap "C:/path/image.png" (or checker)   setAttr mat.repeatUV 4 4   uvChecker on|off   uvEditor
  setAttr mat.normalMap "C:/path/n.png"   setAttr mat.roughnessMap "C:/path/r.png"   setAttr mat.metalnessMap "C:/path/m.png" (greyscale, multiply)
  setAttr mat.occlusionMap path   setAttr mat.emissionMap path   polyMapCut -e edgeIdx... obj   polyMapSewMove -e edgeIdx... obj   u3dUnfold obj
ANIMATION TOOLS: keyTangent -t 12 -ott spline|linear|flat|step|auto obj   copyKey obj; pasteKey -t 50 obj
  ghost on obj (onion skin)   motionTrail on obj   poseLibrary -save name [-h] | -apply name [-w 0.5] [-key] [-mirror]
  retarget -src sourceRootJoint -dst targetRootJoint [-s 1] [-e 48]   (copies an imported/animated skeleton's motion onto another rig)
MODELING TOOLS: polySplit -ep edgeIdx 0.5 -ep edgeIdx 0.5 obj (multi-cut)   softSelect -sse on -ssd 1.5
  sculpt -b sculpt|smooth|grab|flatten|inflate|pinch -s 0.5 -r 60 -sym on (sets the brush; the user strokes)
  lattice -dv 2 5 2 -n ffd obj; latticePoint ffd i j k x y z (local -0.5..0.5; e.g. flare the top: latticePoint ffd 0 4 0 -0.8 0.5 -0.8)
  wrap -d cageMesh hiResMesh (cage deforms the mesh)   lattice -rm obj (remove lattice/wrap/wire)
CURVES (NURBS, degree 3 by default): curve -d 3 -p x y z -p x y z ... [-per] -n name (CVs)   curve -ep x y z -ep ... -n name (passes through points)
  circle -r 1 -s 8 -nr 0 1 0 -c x y z -n name   rebuildCurve -s 8 -d 3 crv   reverseCurve crv   closeCurve crv (open/close)
  attachCurve a b   detachCurve crv.u[1.5] | detachCurve -pf 0.5 crv   select crv.cv[2:4]; move -r 0 1 0   pointPosition crv.cv[0]
  components on any object: select obj.vtx[0:3] | obj.e[5] | obj.f[*] | crv.cv[*], then move/rotate/scale (no names) moves them
SURFACES (polygon output with live history — editing the curves updates the mesh):
  revolve -ax 0 1 0 -p 0 0 0 -s 16 [-ssw 0 -esw 360] -n vase profileCurve (profile in the XY plane, x = radius)
  loft -ss 2 [-c] -n hull c1 c2 c3 (through curves in order)   extrude -n pipe profile pathCurve | extrude -l 2 -dir 0 1 0 profile
  planarSrf -n cap closedCurve   (e.g. vase: curve -d 3 -p 0.3 0 0 -p 0.9 0.6 0 -p 0.5 1.6 0 -p 0.7 2.2 0 -n prof; revolve -s 24 -n vase prof)
WIRE DEFORMER: wire -w curve -dds 0 1.5 meshes (vertices within 1.5 follow the curve)   wire -e -w curve -en 0.5 | -dds 0 2 | -rot 1
QUAD DRAW (retopology): makeLive refMesh; quadDraw -dot x y z (snaps onto the live mesh, returns the dot index)
  quadDraw -quad i j k l | -tri i j k | -extend a b dx dy dz | -relax 5 | -deleteFace f | -deleteDot i | -new | -finish   (quadDraw alone opens the tool)
FX (simulations play with the timeline; set playbackOptions first; currentTime N shows frame N):
  emitter -type omni|dir|surface|curve|volume -r 200 -spd 3 -sp 20 -dir 0 1 0 -lifespan 2 -rad 0.03 -rgb 1 0.6 0.2 -n sparks [sourceMesh]
  setAttr sparks.turbulence 2; setAttr sparks.wind 3 0 0; setAttr sparks.gravity 0 -9.81 0; setAttr sparks.bounce 0.5   (volume emitter: scale it)
  nCloth -stretch 1 -bend 0.1 cape (a subdivided polyPlane works well: polyPlane -w 2 -h 2 -sx 20 -sy 20)
  clothPin -to chest_jnt cape.vtx[0:20] (pins follow a joint/control)   setAttr flag.wind 5 0 0   connectDynamic -c body cape (collider)
  nCloth -selfCollide cape (or setAttr cape.selfCollide 1) keeps folds from passing through each other
  FIELDS (nodes; move / rotate to place and aim; act on the selected simulations at creation, or on all if none):
  airField -m 5 -dx 1 -dy 0 -dz 0 [-s 1] (wind)  gravityField  uniformField  radialField -m 5 -att 1  vortexField -m 4 (axis = -dx/-dy/-dz)
  turbulenceField -m 3 -f 1  dragField -m 0.5  newtonField -m 5 (attractor)  [-mxd maxDistance] [-p x y z] [-n name] [targets]
  connectDynamic -f gust cape (restrict a field to a target)   connectDynamic -rf gust cape   setAttr gust.magnitude 8
  HAIR: hairSystem [-count 400] [-length 0.3] [-seg 8] [-curl 20] [-stiff 0.25] [-width 0.01] [-rgb r g b] [-n hair] headMesh
      (strands follow the deforming mesh; setAttr hair.stiffness/tipStiffness/combAmount/comb x y z/drag/gravity; connectDynamic -c body hair)
  makeCurvesDynamic [-pinTip] rope   (creates rope_dyn, a curve that swings; use it for wires, extrude, emitters)   hairToCurves hair
RECIPES (tuned one-liners — PREFER these for lighting, materials and character animation; each is one undo step):
  lightRig -preset studio|outdoor|sunset|night|product|dramatic [-i 1.0] [-target objects]
      builds a named light rig (key_light, fill_light, rim_light, sun_light, ...) aimed at the subject, grouped as lightRig
      (re-running replaces it); then fine-tune e.g. "setAttr key_light.intensity 1.4", "setAttr rim_light.color 1 0.8 0.6"
  materialPreset -p gold|silver|chrome|copper|brass|brushed_metal|plastic|rubber|glass|water|wood|concrete|marble|ceramic|
      car_paint|skin|fabric|neon|grass [-rgb r g b (tint)] [-n matName] objects   (creates/reuses <preset>_mat and assigns it)
  rigAnimate -anim walk|run|jump|wave|idle|dance [-s startFrame] [-e endFrame] [-travel on|off] [mesh]
      Quick Rigs the mesh if there is no rig yet, then keys the *_ctrl controls procedurally
  renderCheck   renders a small preview and reports exposure / framing / lighting problems with suggested fixes
DIRECTOR (whole shots in one statement each):
  makeShot -theme forest|city|desert|beach|studio|snow|space [-subject human|robot|alien|none] [-anim walk|run|jump|wave|idle|dance]
      [-mood studio|outdoor|sunset|night|product|dramatic] [-camera static|track|dolly|orbit|crane] [-fx rain|snow|sparks|fireflies|dust|leaves|cape|flag]...
      [-wind] [-s 1 -e 120] [-clear]   builds set + character + rig/animation + FX + light rig + render settings + shotCam, looks through it
  character -style human|robot|alien [-h 1.8] [-p x y z] -n hero   environment -theme forest [-size 24] [-density 1] [-seed 7]
  shotCamera -type static|track|dolly|orbit|crane [-target hero] [-s 1 -e 120] [-n shotCam]   fxPreset rain|snow|...|cape|flag [-wind]
POSING (Quick Rig controls; FK joints if there are no controls):
  pose -part arm|elbow|leg|knee|foot|head|chest|hips [-side L|R|both] -action raise|lower|forward|back|out|in|bend|straighten|lift|step|
      turn|nod|tilt|lean|twist|crouch|stand|shift [-dir left|right|forward|back|up|down] [-amount small|large|<degrees>|<n>cm] [-key]
      e.g. "pose -part arm -side L -action raise -amount 40", "pose -part head -action turn -dir right", "pose -part hips -action crouch -key"
RIG CHECK: rigReport (explains skeleton, IK, controls, skins and lists problems with fix statements)   skinPercent -normalize mesh
RENDER: render [-x 960 -y 540] [-spp 64] [-cam cameraName] [-o C:/path/out.png]   (CPU path tracer → Render View; -o saves a still)
  render -o C:/path/shot.png -passes [specular,emission] (beauty/albedo/normal/depth/objectid/diffuse_direct/diffuse_indirect/specular/emission; .exr = float)
  renderSettings -mb on -shutter 0.5 (motion blur)   renderSettings -region 0.2 0.2 0.6 0.6 | -region off
  camera depth of field: setAttr cam.depthOfField 1; setAttr cam.fStop 2.8; setAttr cam.focusDistance 6; setAttr cam.dofBlurScale 20
  render -seq -st 1 -e 24 -o C:/path/name   renderSettings -spp 128 -env 0.3 -exp 0.5 -tm filmic -bg env -dn on   renderView -ipr on|off
IMPORT: file -import C:/path/model.glb [-take Walk] | .gltf | .obj   (brings in meshes, materials/textures, skins, blend shapes, animation)
  file -import C:/path/character.fbx [-take Run]   (FBX 2011+ binary/ASCII: Mixamo/Maya/Blender/Max characters, skins, blend shapes, takes)
  file -import C:/path/stage.usd | .usda | .usdc | .usdz   (USD: composes subLayers/references/payloads/variants/inherits, UsdSkel characters,
  PointInstancers, UsdPreviewSurface + MaterialX looks, lights, cameras; Z-up / cm stages are converted to Y-up metres)
EXPORT: file -export C:/path/scene.glb | .gltf | .fbx | .usda | .obj  [-es selected only] [-na no animation] [-bo FBX: bake joint orients]
MESH CLEANUP: polyReduce -p 75 mesh (removes 75 % of the triangles, keeps borders/UV seams/skin)   polySoftEdge -a 180 mesh (soft) | -a 0 (hard) | -a 30 | mesh.e[3:8]
  (glTF keeps skins, blend shapes, baked animation, lights, cameras; USD writes UsdSkel for skinned meshes and bakes other deformers to points)"#;

pub const CONVENTIONS: &str = r#"COORDINATES: Maya convention, Y is UP, X right, +Z toward the viewer. 1 unit = 1 m. Objects are centered
on their position, so to rest an object on the ground set its Y to half its height (cube of height 2 -> y=1).
Rotations are in degrees. Compute layouts yourself (a ring of n items: x = r*cos(2*pi*i/n), z = r*sin(2*pi*i/n)).
Give objects descriptive -n names (e.g. table_top, leg_1) so later statements and follow-up requests can target them.
ANIMATION: keyframe a start pose, then change time and values and keyframe again, e.g.
  "currentTime 1", "setKeyframe ball", "currentTime 24", "move 0 4 0 ball", "setKeyframe ball"
or directly "setKeyframe -t 12 -at ty -v 3 ball". Set "playbackOptions -min 1 -max N" to fit the animation.
RIGGING RECIPE (arm): "select -cl", "joint -p 0 5 0 -n shoulder", "joint -p 2 5 -0.3 -n elbow", "joint -p 4 5 0 -n wrist",
  "joint -e -oj xyz shoulder", "ikHandle -sj shoulder -ee wrist -n arm_ik", "spaceLocator -n elbow_pole -p 2 5 -3",
  "poleVectorConstraint elbow_pole arm_ik", "circle -r 0.5 -nr 1 0 0 -p 4 5 0 -n wrist_ctrl", "pointConstraint wrist_ctrl arm_ik",
  "skinCluster shoulder arm_mesh". Legs: hip -> knee (bent slightly forward, +Z) -> ankle, pole in front of the knee.
  Controls are curves named *_ctrl; animators key the controls, not the joints.
  For a full character use "quickRig body" and then animate the *_ctrl controls (e.g. "move -r 0 0.3 0 L_hand_ctrl").
  Right-side limbs: build the L_ chain, then "mirrorJoint -myz -sr L_ R_ L_shoulder".
LIGHTING RECIPE: pick the lightRig preset matching the mood (studio = neutral/portrait, outdoor = daylight, sunset = warm
  golden hour, night = moody blue, product = clean bright packshot, dramatic = film noir / low key), then adjust single
  lights with setAttr. Only build lights by hand for unusual requests.
MATERIAL RECIPE: "materialPreset -p car_paint -rgb 0.1 0.2 0.8 car_body", "materialPreset -p chrome wheel_*",
  "materialPreset -p glass window". For custom looks: "shadingNode -asShader blinn -n red_paint -rgb 0.8 0.1 0.1",
  "setAttr red_paint.roughness 0.2", "hyperShade -assign red_paint car_body".
RIG + ANIMATE RECIPE: "rigAnimate -anim walk -s 1 -e 96 body" (or run/jump/wave/idle/dance) — for "make the character walk".
  Afterwards the user can refine keys on the *_ctrl controls (Graph Editor)."#;

pub fn builder_system(scene: &str) -> String {
    format!(
        r#"You are the AI co-pilot inside "Rust Maya", an Autodesk Maya-style 3D app. Turn the user's request (typed or
spoken — it may contain speech-recognition typos) into MEL statements that the app executes in order.

Respond ONLY with JSON: {{"reply": "<one short friendly sentence>", "mel": ["statement", "statement", ...]}}
If the user only asks a question, answer in "reply" and leave "mel" empty.

{MEL_REFERENCE}

{CONVENTIONS}

CURRENT SCENE:
{scene}"#
    )
}

/// Few-shot example keeps small local models on format.
pub fn builder_examples() -> Vec<Value> {
    vec![
        json!({"role": "user", "content": "put a red ball on top of the cube"}),
        json!({"role": "assistant", "content": r#"{"reply":"Added a red ball resting on pCube1.","mel":["polySphere -r 0.5 -n red_ball","move 0 1.5 0","setColor red"]}"#}),
    ]
}

pub fn builder_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reply": {"type": "string"},
            "mel": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["reply", "mel"]
    })
}

// ------------------------------------------------------------------ tev1 decision model

pub fn decision_system(scene: &str, tool_mode: bool) -> String {
    let how = if tool_mode {
        "Respond ONLY with tool calls: run_mel for simple edits, delegate_to_builder for complex builds, answer for questions."
    } else {
        r#"Respond ONLY with JSON: {"route":"direct|delegate|answer","reply":"short sentence","plan":"for delegate","mel":["statements for direct"]}"#
    };
    format!(
        r#"You are the fast DECISION layer of "Rust Maya", a Maya-like 3D app (Y up, 1 unit = 1 m). Decide how to handle the
user's request:
- SIMPLE and concrete (at most ~4 MEL statements on existing objects, or one new object): do it yourself with MEL.
- CREATIVE, multi-object, needs layout math, multi-step animation or building a RIG (skeleton, IK, skinning, controls):
  delegate to the builder with a short plan.
- A question with no scene change: answer.
{MEL_REFERENCE}
{how}

CURRENT SCENE:
{scene}"#
    )
}

pub fn decision_tools() -> Value {
    json!([
        {"type": "function", "function": {
            "name": "run_mel",
            "description": "Run a few simple MEL statements in the Maya-style scene (create one primitive, move/rotate/scale, setColor, select, setKeyframe, viewSet, undo, ...).",
            "parameters": {"type": "object", "properties": {
                "script": {"type": "string", "description": "MEL statements separated by ';'"},
                "reply": {"type": "string", "description": "Short sentence for the user"}
            }, "required": ["script"]}
        }},
        {"type": "function", "function": {
            "name": "delegate_to_builder",
            "description": "Hand the request to the large builder model: creative or multi-object builds (a table, a castle, a solar system), rigging (skeletons, IK, skinning, controls), more than ~4 statements, layout math, or multi-step animation.",
            "parameters": {"type": "object", "properties": {
                "plan": {"type": "string", "description": "Short step-by-step plan for the builder"},
                "reason": {"type": "string"}
            }, "required": ["plan"]}
        }},
        {"type": "function", "function": {
            "name": "answer",
            "description": "Reply in text when the user only asks a question and nothing in the scene should change.",
            "parameters": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}
        }}
    ])
}

pub fn decision_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "route": {"type": "string", "enum": ["direct", "delegate", "answer"]},
            "reply": {"type": "string"},
            "plan": {"type": "string"},
            "mel": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["route", "reply"]
    })
}

// --------------------------------------------------------------------- one-shot tasks

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    /// Script Editor: write MEL from a description (inserted, not run).
    GenerateMel,
    /// Script Editor: explain the current script.
    ExplainMel,
    /// Repair a failing MEL script given the error.
    FixMel,
    /// Outliner: give selected objects meaningful names.
    SmartRename,
    /// Scene health check: prioritise the issues found and suggest fixes.
    HealthCheck,
    /// Colour palette for the selection from a mood/description.
    Palette,
    /// Find Menu fallback / Maya help question.
    Help,
    /// Critique a preview render (image attached when the model can see) and suggest fixes.
    RenderCheck,
    /// Plan a whole shot (set, character, animation, FX, lighting, camera) from a description.
    MakeShot,
    /// Explain a rig to the user from the automatic rig report and suggest fixes.
    ExplainRig,
}

impl Task {
    pub fn label(self) -> &'static str {
        match self {
            Task::GenerateMel => "Generate MEL",
            Task::ExplainMel => "Explain MEL",
            Task::FixMel => "Fix MEL",
            Task::SmartRename => "Smart Rename",
            Task::HealthCheck => "Scene Health Check",
            Task::Palette => "Color Palette",
            Task::Help => "Help",
            Task::RenderCheck => "Render Check",
            Task::MakeShot => "Make a Shot",
            Task::ExplainRig => "Explain Rig",
        }
    }

    pub fn system(self, scene: &str) -> String {
        let job = match self {
            Task::GenerateMel => "Write a MEL script that does what the user describes. Put the statements in \"mel\" and a one-line summary in \"reply\". The script will be shown to the user for review before running.",
            Task::ExplainMel => "Explain what the given MEL script does, statement by statement, in plain language (short). Put the explanation in \"reply\" and leave \"mel\" empty.",
            Task::FixMel => "The user's MEL script failed. Using the error message, return a corrected script in \"mel\" and say what was wrong in \"reply\". Only use commands from the reference.",
            Task::SmartRename => "Rename the listed objects to short, descriptive snake_case names based on their shape, size, position, colour and hierarchy (e.g. table_top, leg_front_left). Return one \"rename old new\" statement per object in \"mel\".",
            Task::HealthCheck => "You are a senior modeling TD reviewing a scene. You get an automatic report of issues. In \"reply\" give a short prioritised list (max 6 bullets) of what to fix and why it matters for rigging/animation/export. In \"mel\" put safe fix statements (e.g. makeIdentity / delete -ch on specific objects selected first, renames). Never delete user objects.",
            Task::Palette => "Choose a harmonious colour palette matching the user's description and apply it to the SELECTED objects using \"setColor r g b name\" statements with values 0-1 (one per object). Describe the palette in \"reply\".",
            Task::RenderCheck => "You are a lighting and look-development supervisor reviewing a preview render of the scene (the image is attached when your model can see images; otherwise rely on the measured statistics). In \"reply\" give a short critique (max 5 bullets): composition/framing, exposure, lighting (key/fill/rim, shadows, mood), materials, and the single most important fix first. In \"mel\" put concrete fixes — prefer lightRig / materialPreset / renderSettings -exp / setAttr light.intensity / viewFit; never delete user objects. The fixes are offered to the user as a button.",
            Task::MakeShot => "You are a film director and layout artist. Build the shot the user describes. You get an offline draft (a makeShot statement picked from keywords): improve it so it matches the description (theme, subject, animation, mood, camera move, FX, frame range). Prefer ONE makeShot statement, optionally followed by a few refinements (materialPreset, setAttr light/emitter attributes, pose ... -key, extra props placed on the ground). Use -clear only if the user asks for a new/fresh scene. In \"reply\" describe the shot in one or two sentences, like a director's note.",
            Task::ExplainRig => "You are a friendly rigging TD. You get an automatic report of the scene's rig. In \"reply\" explain in plain language how the rig works (what moves what, which controls the animator should use) in a few short sentences, then list the problems in order of importance and why they matter. In \"mel\" put safe fix statements (from the report's suggestions or similar: skinPercent -normalize, joint -e -oj xyz, poleVectorConstraint, makeIdentity); they are offered as a button. Never delete user objects.",
            Task::Help => "Answer the user's question about using this Maya-style app (tools, hotkeys, MEL) briefly. If an action would help, include the MEL for it in \"mel\" (it is offered as a button, not run automatically).",
        };
        format!(
            r#"You are an assistant inside "Rust Maya", an Autodesk Maya-style 3D app.
{job}
Respond ONLY with JSON: {{"reply": "...", "mel": ["...", ...]}}

{MEL_REFERENCE}

{CONVENTIONS}

HOTKEYS: Q/W/E/R select/move/rotate/scale, F/A frame, Space four-view, 4/5/6 shading, F8/F9/F11 object/vertex/face,
Ctrl+E extrude, Ctrl+D duplicate, Ctrl+G group, P/Shift+P parent/unparent, S set key, Alt+V play, F3 Find Menu,
` (backquote) voice command, Ctrl+L focus the AI prompt.

CURRENT SCENE:
{scene}"#
        )
    }
}
