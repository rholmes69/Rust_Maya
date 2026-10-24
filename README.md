# Rust Maya

A Maya-style 3D content-creation app written in Rust with **egui** (UI) and **wgpu** (viewport rendering).
It covers the **UI, polygon and edge modeling, keyframe animation, character rigging (with deformers and a one-click
biped Quick Rig), look development (materials, lights, cameras, shadows), glTF / USD export and an AI copilot**.
Simulation, NURBS, UVs and Arnold rendering show up in the menus as greyed-out "planned" items.

![screenshot](docs/screenshot.png)

## Build & run (Windows / macOS / Linux)

1. Install Rust 1.95 or newer from https://rustup.rs (run `rustup update` if you already have it).
2. In this folder, run:

```
cargo run --release
```

The first build downloads and compiles the dependencies, which takes a few minutes. The app opens with a demo scene
(pCube1, pSphere1, pTorus1). On Windows the release build runs without a console window.

Run the tests with `cargo test`. They cover the mesh and edge ops, scene graph, animation curves, camera math, MEL
commands, preferences, rigging (IK, skinning, constraints, driven keys, blend shapes, clusters, mirroring, Quick Rig),
materials/lights/cameras, the glTF and USD exporters, and the AI parsers and router.
`cargo test --release perf_ -- --ignored --nocapture` runs the viewport geometry benchmark.

## What's included

| Maya area | Implemented |
|---|---|
| **Menu sets** | Modeling / Rigging / Animation / FX / Rendering switch the menus after the common File · Edit · Create · Select · Modify · Display · Windows menus |
| **Status line** | Menu-set selector, new/open/save, undo/redo, Object/Vertex/Face selection modes, grid snap, Workspace selector, panel toggles |
| **Shelves** | Poly Modeling, Rigging, Animation and Custom tabs work (Script Editor → *Save to Shelf* adds buttons to Custom). The other tabs are placeholders. |
| **Workspaces** | Maya Classic, Modeling - Standard, Animation (opens the Graph Editor), Rigging |
| **Tool Box** | Select (Q), Move (W), Rotate (E), Scale (R), layout buttons |
| **Viewport** | wgpu with MSAA and depth; persp/top/front/side cameras; single or four-view layout (Space); wireframe (4), smooth shaded (5), textured (6), wire-on-shaded (Shading menu); grid; axis triad; poly-count HUD |
| **Navigation** | Alt+LMB tumble, Alt+MMB (or MMB) track, Alt+RMB or wheel dolly, F frame selection, A frame all |
| **Selection** | Click, marquee drag, Shift toggle, Ctrl deselect, Ctrl+Shift add; ray-cast picking |
| **Manipulators** | Move (world axes plus a screen-plane center handle), Rotate (gimbal rings showing the angle), Scale (per-axis and uniform). Hold X to snap (grid units / 15°) and Shift for fine control. |
| **Polygon modeling** | Cube, Sphere, Cylinder, Cone, Plane, Torus with live construction history (polyCube1 inputs you can edit). Vertex (F9), Edge (F10) and Face (F11) component editing, Extrude (Ctrl+E), Delete faces/verts/edges, Smooth (Catmull-Clark), Combine, Separate, Triangulate, Reverse normals, Freeze Transformations, Delete History |
| **Edge modeling** | Edge loop / ring selection (double-click an edge), Insert Edge Loop tool (Ctrl+R, live preview), Bevel (Ctrl+B), Bridge two faces, Merge (to center or by distance), Mirror geometry (with seam weld), Delete Edge — on the Poly Modeling shelf, Edit Mesh menu and as MEL |
| **Scene graph** | Hierarchy, Group (Ctrl+G), Parent (P), Unparent (Shift+P), Duplicate (Ctrl+D), Delete, Hide/Show. Maya-style unique names (pCube1, pCubeShape1). |
| **Outliner** | Tree with expand/collapse, search filter, double-click rename, drag-and-drop parenting, context menu |
| **Channel Box** | Translate/Rotate/Scale/Visibility (edits apply to every selected object), keyed channels highlighted red, channel right-click menu (Key Selected, Break Connections, Reset), SHAPES and INPUTS sections |
| **Layer Editor** | Display layers with Visibility (V) and Template (T) toggles, create from selection, add/remove/select members |
| **Attribute Editor** | Transform, display, shape stats, construction history, lambert colour, and animation curves with tangent type (Auto/Linear/Stepped) |
| **Animation** | Set Key (S), Shift+W/E/R key T/R/S, Auto Keyframe, auto-clamped spline curves, Time Slider with key ticks and scrubbing, Range Slider, fps, loop, playback (Alt+V), next/previous key (, .), step frame (Alt+, Alt+.) |
| **Graph Editor** | Curve list, key selection, tangent handles and types, insert/delete/copy/paste keys, pan and zoom (see Animation tools) |
| **Command line / Script Editor** | A MEL-like language: `polyCube -w 2 -n box; move -r 0 1 0; setKeyframe -at tx; select p*; parent; getAttr box.tx` … type `help` for the full list |
| **Files** | Save/Open scenes (`.rma`, JSON), Undo/Redo (100 levels), Recent Files, autosave every few minutes with crash recovery, "save changes?" prompt on New/Open/Quit, OBJ import/export (with .mtl), **glTF 2.0 import and export (.glb/.gltf)**, **USD (.usda) export**, drag-and-drop import |
| **Preferences** | Windows › Preferences: autosave interval, demo scene; layout, shading and tool options are remembered between sessions |

## Rigging

![rigging](docs/rigging.png)

Switch the menu set to **Rigging** (or use the Rigging shelf):

| Menu | What it does |
|---|---|
| **Skeleton > Create Joints** | Click in any view to place joints; each new joint is the child of the previous one. **Enter** finishes the chain and orients it (X down the bone, Y up). Ortho views place joints on the view plane; hold X to snap to the grid. |
| **Skeleton > Orient Joint / Create IK Handle** | Re-orient selected chains. To make an IK handle, select the start joint then the end joint (chains of 3 joints use an analytic rotate-plane solver; longer chains use CCD). |
| **Skin > Bind Skin** | Select the joints (or just the root) and the mesh. Closest-distance weights, up to 4 influences per vertex. The mesh deforms live in the viewport and for picking. |
| **Skin > Paint Skin Weights** | Drag on the mesh with a brush (Replace / Add / Smooth); the mesh shows the current influence's weights in black and white. Flood, influence list and brush size are in Tool Settings. Each stroke is one undo step. |
| **Skin > Unbind / Go to Bind Pose** | Restore the pose captured at bind time. |
| **Constrain** | Parent, Point, Orient, Scale and Aim constraints (select the drivers, then the target; *maintain offset* option), Pole Vector, and Remove Constraints. |
| **Control** | Circle, square, cross, arrow, cube and sphere control curves, created at the selected object (circles face down the joint). **Set Driven Key…** opens Maya's two-column driver/driven window. |

Each frame evaluates rigging in this order: animation curves → driven keys → constraints → IK → skin deformation.
The Attribute Editor shows joint orient and radius, the IK chain and pole, skinCluster info, constraints (with delete)
and driven keys. Everything is also available as MEL (`joint`, `ikHandle`, `poleVectorConstraint`, `parentConstraint`…,
`skinCluster`, `gotoBindPose`, `circle`, `controlShape`, `setDrivenKeyframe`), so the AI copilot can build rigs too,
for example: *"rig the arm mesh with an IK arm, an elbow pole vector and a wrist control"*.

### Deformers and rig tools (v0.4)

| Menu | What it does |
|---|---|
| **Skeleton > Mirror Joints** | Mirrors the selected chain across YZ / XZ / XY and renames `L_` → `R_` (search/replace editable in the menu). |
| **Skeleton > Quick Rig (one-click biped)** | Select a character mesh standing on Y and facing +Z: builds a 25-joint skeleton fitted to its bounding box (hips, spine, chest, neck, head, clavicles, arms, legs, toes), mirrors the limbs, adds arm and leg IK with pole vectors, controls (`root_ctrl`, `cog_ctrl`, `chest_ctrl`, `head_ctrl`, hand/foot/clavicle controls) and binds the mesh. One undo step. |
| **Skin > Mirror Skin Weights** | Copies weights from +X to −X (or the inverse), swapping each influence for its mirror joint. |
| **Deform > Blend Shape** | Select target meshes, then the base. Weights appear as sliders in the Attribute Editor and in the Channel Box INPUTS, can be keyed (◆) and driven with Set Driven Key. |
| **Deform > Cluster** | Creates a cluster handle from the selected vertices (optional soft falloff); move/rotate/scale the handle to deform. |
| **IK handle (Attribute Editor)** | *Stretchy* (bones lengthen to reach the goal) and *Twist*. |

Deformer order is blend shape → skin → clusters. MEL: `mirrorJoint -myz -sr L_ R_ L_shoulder`, `copySkinWeights -mm YZ body`,
`quickRig body`, `blendShape smile face`, `setAttr face.smile 0.5`, `setKeyframe -at smile face`, `cluster -fo 1`,
`ikHandle -e -stretch on -twist 30 arm_ik`.

## Look development (v0.5)

![look dev](docs/lookdev.png)

| Feature | Details |
|---|---|
| **Materials / Hypershade** | Windows › Rendering Editors › Hypershade (or the Rendering shelf): lambert, blinn, phong and standardSurface materials shown as swatches. Edit color, specular, roughness, metalness, incandescence and transparency; assign to the selection; select objects by material; duplicate; delete. Materials are shared, so editing one updates every object that uses it. Objects without a material keep their own lambert1 colour. The Attribute Editor shows the assigned material with a picker. |
| **Lights** | Create › Lights: ambient, directional, point and spot lights are scene nodes (they shine down their −Z axis). The Attribute Editor has colour, intensity, cone/penumbra and *Use Depth Map Shadows*. |
| **Viewport lighting** | Lighting › Use All Lights (**7**) lights the scene with its lights (up to 8) using Blinn-Phong plus a simple environment reflection for metals. **Shadows** uses a 2048² PCF shadow map from the first directional or spot light that casts shadows. **X-Ray** (**Alt+X**) makes surfaces see-through. All three are also buttons on the view toolbar. |
| **Cameras** | Create › Cameras › Camera / Camera from View. Panels › Look Through Selected (or Panels › Perspective › camName): tumbling the view moves the camera, and moving or animating the camera moves the view. Focal length and clip planes are in the Attribute Editor. |

MEL: `shadingNode -asShader blinn -n red -rgb 1 0 0`, `setAttr red.roughness 0.2`, `hyperShade -assign red box ball`,
`sets -e -forceElement redSG box`, `directionalLight -n sun -i 1.2 -rot -50 30 0 -sh on`, `spotLight -ca 40 -p 0 6 4 -aim 0 0 0`,
`camera -n shot -fl 50 -p 8 6 10 -aim 0 1 0`, `lookThru shot`, `lighting -all -shadows on`, `xray on`.

## UVs and textures (v0.7)

![uv editor](docs/uv_editor.png)

| Feature | Details |
|---|---|
| **UVs** | Every primitive is created with Maya-style UVs (the cube uses the cross layout). UVs are kept through extrude, smooth, triangulate, delete, combine, separate, mirror and OBJ round trips (edge tools like bevel or bridge drop them; re-project afterwards). |
| **Projections** | UV menu (Modeling menu set) or the UV Editor toolbar: **Automatic** (6 directions, packed), **Planar** (best plane / X / Y / Z), **Cylindrical**, **Spherical**. In face mode only the selected faces are projected. |
| **UV Editor** | Windows › Modeling Editors › UV Editor (or `uvEditor`): the 0–1 tile with the material's texture (or the checker) behind. Click, Shift-click or drag a marquee to select UVs; double-click (or *Select Shell*) for whole shells. Drag UVs to move them. Flip U/V, rotate ±90°, scale, Layout (packs shells without overlaps), Normalize. Wheel zooms, MMB or Alt pans, F frames. Fully undoable. |
| **Textures** | In a material (Hypershade or Attribute Editor), *Color Map › File…* connects a PNG, JPEG, BMP or TGA image, *Checker* connects the built-in UV checker, and *Repeat UV* tiles it. Textures show in the viewport (toggle with **6** or the ▣ button) through a mip-mapped texture array. The ▚ button / **UV Checker** shows the checker on every mesh. |
| **Export** | glTF writes `TEXCOORD_0`, embeds the images (PNG/JPEG as is, others re-encoded) and connects `baseColorTexture`. USD writes `primvars:st` (face-varying, indexed) and a `UsdPrimvarReader_float2 → UsdTransform2d → UsdUVTexture → UsdPreviewSurface` network (the checker is saved as `<scene>_uvchecker.png` next to the file). OBJ writes `vt` and a companion `.mtl` with `Kd` / `map_Kd`. |

MEL: `polyAutoProjection box`, `polyPlanarProjection -md y floor`, `polyCylindricalProjection`, `polySphericalProjection`,
`polyLayoutUV`, `polyNormalizeUV`, `polyEditUV -u 0.25 -v 0 -su 2 -sv 2 -a 90`, `polyFlipUV -ft 1`,
`setAttr wood.colorMap "C:/tex/wood.jpg"`, `setAttr wood.repeatUV 4 4`, `uvChecker on`, `textured off`.

## Export (v0.6)

File › Export All / Export Selection (glTF / USD), or `file -export path.glb|.gltf|.usda [-es] [-na]`:

| Format | What is written |
|---|---|
| **glTF 2.0** (`.glb` binary, `.gltf` with an embedded buffer) | Node hierarchy (joint orient folded into rotations), meshes with normals, PBR materials (metallic-roughness, emission, alpha blend), skins (joints + inverse bind matrices, 4 influences), blend shapes as morph targets (with target names), baked animation of every node that moves over the playback range (including motion driven by constraints, IK and driven keys) and of morph weights, `KHR_lights_punctual` lights and perspective cameras. Validated with the Khronos glTF Validator (0 errors). |
| **USD** (`.usda`) | Xform hierarchy, Mesh, BasisCurves (controls), UsdPreviewSurface materials with bindings, UsdLux lights (DistantLight, SphereLight, spot via ShapingAPI, DomeLight for ambient), Camera, transform animation as time samples, and skinned / blend-shape / cluster meshes **baked as per-frame points**. Opens in `usdview` and any USD-aware DCC. |

Not exported: cluster deformers in glTF (glTF has no equivalent), ambient lights in glTF, NURBS-style control curves in glTF.

## Import (v0.8)

![import](docs/import.png)

File › Import (glTF / OBJ)…, drag and drop a file onto the window, or `file -import path [-take animName]`:

| Format | What comes in |
|---|---|
| **glTF 2.0** (`.glb`, `.gltf` with embedded, external or data-URI buffers) | Node hierarchy (TRS or matrix), meshes (triangles, strips, fans; vertices welded so edges are connected), UVs, PBR materials with the base-colour texture (embedded images are saved to `<file>_textures/`), skins → joints + skinCluster (exact glTF bind matrices), morph targets → blend shapes (with names and default weights), the first animation (or `-take Walk`) → keyframes on translate/rotate/scale and blend weights (LINEAR, STEP and CUBICSPLINE samplers, sparse accessors), `KHR_lights_punctual` lights and perspective cameras. One undo step. |
| **OBJ** | Meshes with UVs, plus materials from the `.mtl` (`Kd` colour and `map_Kd` texture). |

Not imported yet: normal, metallic-roughness, occlusion and emissive *maps* (their factors are approximated), orthographic cameras.
Tested against the Khronos sample models CesiumMan, Fox (3 takes), RiggedFigure, AnimatedMorphCube, BoxAnimated, BoxTextured and DamagedHelmet, and with Rust Maya's own glTF round-trip (rig, skin, blend shape, animation, texture, light, camera).

## Animation tools (v0.9)

| Tool | Details |
|---|---|
| **Graph Editor** | Click, Shift-click or box-select keys; drag to move them (whole frames, Ctrl locks the value); **tangent handles** on selected keys (drag to edit, Shift breaks in/out); per-key tangent types **auto, spline, linear, flat, step** (toolbar, Key › Tangents or `keyTangent`); Ctrl+click a curve to insert a key; Del deletes; copy/paste keys. |
| **Dope Sheet** | Windows › Animation Editors › Dope Sheet (or `dopeSheet`): summary and channel rows of diamonds; select, box-select, drag to retime, Del, click the ruler to scrub, wheel zoom. |
| **Ghosting** | Visualize › Ghost Selected (`ghost on`): onion-skin wireframes before (blue) and after (orange) the current frame; count and step are adjustable. Ghosting a rig control or joint ghosts the skinned character. |
| **Editable motion trails** | Visualize › Create Editable Motion Trail (`motionTrail on`): the path over the playback range with frame ticks and red key markers. **Drag a red marker in the viewport** to move that key (translate keys are set at that frame). |
| **Pose Library** | Save poses of the selected controls (or a whole rig with *hierarchy*), apply them with a blend amount, key on apply, or **mirror** them to the other side (L_/R_, Left/Right names). Poses are saved in the scene. |
| **Retargeting** | Animation › Skeleton › Retarget Animation… (or `retarget -src A -dst B`): joints are matched by name and body part (Mixamo, Quick Rig, `leg_joint_L_1`-style chains, `thigh_l`…), the motion is transferred as world-space rotation offsets from each skeleton's bind pose with a facing correction, the hip translation is scaled by leg length, and keys are baked on every frame. Example: import CesiumMan.glb, Quick Rig a character, retarget the walk onto it. |

## Modeling tools (v1.0)

| Tool | Details |
|---|---|
| **Multi-Cut** | Mesh Tools › Multi-Cut (shelf *MCut*, `setToolTo multiCut`): click points on edges (near a vertex snaps to it), Enter or right-click cuts, Esc cancels. UVs are interpolated. Scripted: `polySplit -ep 4 0.5 -ep 9 0.5 mesh`. |
| **Soft Select** | **B** toggles it (Tool Settings › Soft Selection sets the falloff radius). Moving, rotating or scaling vertices, edges or faces pulls the neighbours with a smooth falloff; affected vertices are tinted yellow → red. `softSelect -sse on -ssd 1.5`. |
| **Sculpt brushes** | Sculpting shelf: **Sculpt, Smooth, Grab, Flatten, Inflate, Pinch**, with X symmetry, size (screen pixels), strength and invert (or hold Ctrl). LMB-drag on a mesh; each stroke is one undo step. `sculpt -b grab -s 0.6 -r 80 -sym on`. |
| **Lattice** | Deform › Lattice (`lattice -dv 2 5 2`): a free-form deformation cage fitted around the selected meshes. Move, rotate or scale the lattice to carry the shape; in Vertex mode (F9) drag lattice points (soft select works too); `latticePoint ffd i j k x y z` scripts it; Attribute Editor › Reset Lattice. |
| **Wrap** | Deform › Wrap: select the meshes to deform, then the low-res influence mesh last (`wrap -d cage hiRes`). Each vertex is bound to the closest cage triangle; editing, skinning or animating the cage deforms the high-res mesh. |

Deformer order: blend shape → skin → clusters → lattices → wrap.

![Sculpted ground with X symmetry and a lattice-flared sphere](docs/modeling.png)

## Rendering: path tracer and Render View (v1.1)

Status line **🎬** (or Rendering › Render › Render Current Frame, the *Render* shelf button, or `render`) renders the active view — or any camera — with a built-in multi-threaded **CPU path tracer** into the **Render View** window. The image refines progressively; **IPR** re-renders automatically whenever geometry, materials, lights, the camera or the settings change (selection changes don't).

- **Materials:** Lambert / Blinn / Phong / standardSurface colour, specular, roughness (GGX), metalness, incandescence (emissive), transparency and file textures (including the UV checker).
- **Lights:** directional, point and spot lights (cone + penumbra) with ray-traced shadows, ambient lights, and a sky dome (*Sky*). An empty scene uses a default key light like the viewport.
- **Global illumination:** diffuse and glossy bounces (default 4) with Russian roulette; a *Clamp* setting fights fireflies.
- **Output:** exposure, sRGB or Filmic (ACES) tone mapping, and a sky or transparent background (PNG alpha). Save… writes a PNG, and **Sequence…** renders a frame range to `name.0001.png …` without blocking the UI.

```
render -x 1280 -y 720 -spp 128 -b 4 -cam shot -o C:/renders/still.png     (blocking still)
render -seq -st 1 -e 48 -o C:/renders/walk                                (image sequence)
renderSettings -x 960 -y 540 -spp 64 -env 0.3 -exp 0.5 -tm filmic -bg env   renderView -ipr on | -stop | -save C:/r.png
```

![Render View](docs/render_view.png)

## Performance (v0.6)

The viewport keeps a fingerprint of everything that affects drawing. If nothing changed, the last frame's geometry is
reused and nothing is uploaded to the GPU. Each mesh's GPU vertices are cached per node and rebuilt only when that node changes
(transform, mesh edit, material, selection, deformer inputs), and the renderer re-uploads only the blocks that changed.
Deformed meshes reuse their cached triangulation and recompute normals across threads. On the 2-core test machine a frame
with a 20k-vertex skinned mesh deforming went from about 33 ms to about 9 ms, and a frame where nothing changed costs about 0 ms.

## AI: Ollama, OpenRouter, Whisper voice and the tev1 decision model

![AI copilot](docs/ai_copilot.png)

The AI changes the scene only through **MEL**, using the same interpreter as the command line. Every AI action is
therefore validated, logged in the Script Editor and undoable. A whole AI request is **one undo step**.

### Setup

* **Ollama (local):** install from https://ollama.com and run `ollama pull qwen3:8b` (the builder). Also pull or create
  your **`tev1`** decision model; any installed model named `tev1…` is picked up automatically.
* **OpenRouter (cloud, optional):** set the `OPENROUTER_API_KEY` environment variable, or paste the key in
  **AI → AI Settings**. The live model catalogue appears in the model pickers, with filters for tool-calling and free models.
* **Voice:** the first time you press the mic button (or the **`** key), the chosen Whisper model is downloaded once from
  Hugging Face (`base.en` is about 145 MB) and cached in your user cache folder under `rust_maya/whisper/`. It then runs
  fully locally through `candle`. To build without voice, use `cargo run --release --no-default-features`.

Settings are saved to `%APPDATA%\rust_maya\ai_settings.json` on Windows (`~/.config/rust_maya/` on Linux). If you type
the OpenRouter key into AI Settings, it is stored there in plain text; the environment variable keeps it out of the file.

### How a request flows

1. The **instant parser** handles simple phrases ("undo", "top view", "add a red sphere", "move it up 2") with no model.
2. The **tev1 decision model** reads the request plus a scene summary and calls one of three tools: `run_mel` (does
   simple edits itself), `answer` (questions) or `delegate_to_builder` (creative or multi-object builds, with a plan).
   It uses native tool calling when the model supports it and JSON routing otherwise. The chat shows who handled each
   request, for example "tev1 → builder · plan: …".
3. The **builder model** (Ollama or OpenRouter) streams `{"reply", "mel": [...]}`. Each statement runs as soon as it
   arrives. If the builder fails, the **fallback** model is tried.

### The 20 AI enhancements

| # | Enhancement | Where |
|---|---|---|
| 1 | **AI Copilot panel**: streaming chat, MEL runs while the model types, Stop button, one undo step per request | ✨ AI button, Ctrl+L |
| 2 | **Ollama provider**: auto-picks the best installed model, in-app `ollama pull` with progress, GPU-memory readout | AI Settings |
| 3 | **OpenRouter provider**: live catalogue, filters (tool-calling / free), API key or env var, SSE streaming | AI Settings |
| 4 | **tev1 decision router**: direct / answer / delegate routing via tool calls, with JSON fallback | automatic |
| 5 | **Per-role models**: Decision, Builder, Helper and Fallback, mixing local and cloud models freely | AI Settings |
| 6 | **Automatic fallback**: for example, Ollama is offline so the request goes to OpenRouter | AI Settings → Fallback |
| 7 | **Whisper voice commands**: tiny, base, small or distil-small, run locally, with a mic level meter | 🎤, ` key |
| 8 | **Hands-free listening**: stops when you pause (adjustable threshold), auto-send or review first | AI Settings |
| 9 | **Spoken replies** through the system's text-to-speech (Windows SAPI, macOS `say`, Linux `spd-say`/`espeak`) | AI Settings |
| 10 | **Instant offline parser** for common commands, with zero latency and no model needed | automatic |
| 11 | **AI mode for the command line**: the MEL button toggles MEL ↔ AI, like Maya's MEL/Python switch | command line |
| 12 | **Find Menu**: fuzzy search over commands; anything not found can be sent to the AI as a question or a request | F3 / Ctrl+K |
| 13 | **Generate MEL** from a description, placed in the Script Editor for review | Script Editor |
| 14 | **Explain MEL**: plain-language walkthrough of the current script | Script Editor |
| 15 | **Fix with AI**: repairs the last failing MEL using its error message | Script Editor, AI menu |
| 16 | **Smart Rename**: descriptive names from each object's shape, size, position and hierarchy | Outliner right-click, AI menu |
| 17 | **AI color palette** for the selection from a mood ("warm sunset", "neon cyberpunk"…) | AI menu, Copilot |
| 18 | **Scene Health Check**: rule-based report (unfrozen transforms, history, n-gons, default names…), plus AI advice and one-click fixes | AI menu, AI shelf |
| 19 | **AI animation**: keyframes from plain language ("bounce three times over 72 frames"), using `setKeyframe -t -at -v` | Copilot |
| 20 | **AI shelf, saved prompts and history**: ⭐ saves a prompt as a shelf button; ↑/↓ recalls earlier prompts (both persist) | ✨ AI shelf tab |

New MEL commands added for the AI (and for you): `setColor`, `selectFaces x y z`, `polyExtrudeFacet -ltz`, `viewSet`,
`viewFit`, `panelLayout`, `displayMode`, `hide`, `showHidden`, `cutKey`, and `setKeyframe -t/-at/-v`.

### AI recipes and render check (v1.2)

The Copilot has four recipe buttons. The recipes also work offline, by typing or speaking short requests such as "studio lighting", "make it chrome", "make him walk" or "render check". Builder models are prompted to use them as high-level building blocks.

| Recipe | What it does | MEL |
|---|---|---|
| **💡 Light** | Light rigs fitted to the subject's bounds: **studio** (three-point), **outdoor**, **sunset**, **night**, **product**, **dramatic**. The lights are named (key_light, rim_light, …) and grouped as `lightRig`, so re-running replaces the rig and the AI can fine-tune it with `setAttr`. The sky level for the path tracer is set too. "Let the AI light it…" sends the job to the builder model. | `lightRig -preset sunset [-i 1.2] [-target obj]` |
| **🎨 Materials** | 19 tuned presets (gold, chrome, glass, car paint, wood, marble, skin, neon…), applied to the selection and shared as `<preset>_mat`; `-rgb` tints a preset. "Let the AI shade the scene…" asks the model to give every object a fitting material. | `materialPreset -p car_paint -rgb 0.1 0.2 0.8 body` |
| **🏃 Rig & animate** | Quick Rigs the selected mesh if there is no rig yet, then keys a procedural **walk, run, jump, wave, idle or dance** on the controls. During a walk the planted foot stays put, and it all goes in one undo step. | `rigAnimate -anim walk -s 1 -e 96 [-travel off] body` |
| **🔍 Render check** | Renders a 384×216 preview with the path tracer and measures it: coverage, exposure, contrast, clipped highlights and crushed blacks, objects cut off at the frame edge, missing lights, unshaded meshes. It shows the image with the findings and one-click fixes. The preview is then sent to a **vision model** (an installed `gemma3`, `qwen2.5vl` or `llava`, or OpenRouter `gpt-4o-mini`; set it under AI Settings › Vision), which writes a critique and suggests fixes. A text-only model gets the measurements instead. | `renderCheck [-x 320 -y 180 -spp 16]` |

![Render check](docs/ai_render_check.png)

### Testing without models

`tools/mock_ai.py` imitates both the Ollama and OpenRouter APIs, including tev1 tool calls and streaming:

```
python tools/mock_ai.py 8765
set MOCK_AI=http://127.0.0.1:8765 && cargo test mock -- --ignored
```

To try the UI against it, set the Ollama host to `http://127.0.0.1:8765`, the OpenRouter key to `sk-or-test`, and
`"openrouter_url": "http://127.0.0.1:8765/api/v1"` in `ai_settings.json`.

## Project layout

```
src/
  main.rs        app entry (eframe + wgpu)
  scene.rs       DAG nodes, channels, keyframe curves, layers, materials
  mesh.rs        n-gon mesh, primitives, extrude, Catmull-Clark, shells, OBJ I/O, deformed-normal fast path
  mesh_ops.rs    edge loops/rings, insert edge loop, bevel, bridge, merge, mirror, dissolve edge
  modeling_ops.rs undoable edge-modeling actions
  prefs.rs       preferences, recent files, autosave and crash recovery
  look.rs        materials, lights, cameras, look-through, GPU light setup
  export/        glTF 2.0 (.glb/.gltf) and USD (.usda) exporters with animation baking
  camera.rs      persp/ortho cameras, projection, picking rays
  render.rs      wgpu renderer (offscreen MSAA + depth, shaded/line pipelines, blit into egui)
  app.rs         editor state and every undoable action
  commands.rs    MEL-like command interpreter
  rig.rs         skinning, blend shapes, clusters, IK (twist/stretch), constraints, driven keys, joint orient
  rig_ops.rs     undoable rigging actions (incl. mirror joints/weights, Quick Rig) for menus, tools, MEL and the AI
  ai/            providers (Ollama + OpenRouter), tev1 router, prompts, stream/offline parsers, scene context
  voice.rs       cpal microphone + VAD + Whisper on candle (feature "voice", on by default)
  tts.rs         spoken replies via the OS text-to-speech
  ui/            menus+status line+shelf, panels (toolbox/outliner/channel box/AE), timeline,
                 viewport (navigation, picking, manipulators, joint tool, weight painting, HUD), floating
                 windows, copilot (AI UI), rigging (menus, tool settings, Set Driven Key),
                 lookdev (Hypershade, light/camera/material editors, Rendering shelf)
```

## Roadmap

These items appear in the UI as disabled "planned" entries:
- Deformers: wire
- Modeling: Quad Draw, UV cut/sew/unfold, normal and other texture maps, NURBS curves and surfaces
- Area lights, render passes/AOVs and denoising, USD import, UsdSkel export (deformation is baked to points today), FBX
- FX: particles, cloth, fluids

*Not affiliated with Autodesk. Maya is a trademark of Autodesk, Inc.*
