//! Application state and editing actions (everything the UI, hotkeys and the
//! command line can do goes through here so it is undoable).

use crate::camera::{CamKind, Camera};
use crate::mesh::{self, Mesh, Primitive};
use crate::render::{Chunk, FrameGeometry, GpuVertex};
use crate::scene::{Channel, Layer, MeshCache, NodeId, Scene, Shape};
use glam::{Mat4, Vec2, Vec3};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

/// Things the menus, shelves and command line can ask the AI copilot to do.
#[derive(Clone, Debug)]
pub enum AiUiRequest {
    Prompt(String),
    Task(crate::ai::prompts::Task, String),
    ToggleCopilot,
    FocusPrompt,
    ToggleVoice,
    FindMenu,
    HealthCheck,
    PaletteDialog,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    /// Skeleton > Create Joints: click to place joints, Enter to finish the chain.
    Joint,
    /// Skin > Paint Skin Weights.
    PaintWeights,
    /// Mesh Tools > Insert Edge Loop.
    EdgeLoop,
    /// Mesh Tools > Multi-Cut: click edges, Enter to cut.
    MultiCut,
    /// Sculpting brushes.
    Sculpt,
    /// Mesh Tools › Quad Draw (retopology on a live surface).
    QuadDraw,
    /// Create › Curve Tools › CV Curve Tool.
    CurveCv,
    /// Create › Curve Tools › EP Curve Tool.
    CurveEp,
}

impl Tool {
    pub fn has_gizmo(self) -> bool {
        matches!(self, Tool::Move | Tool::Rotate | Tool::Scale)
    }
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select Tool",
            Tool::Move => "Move Tool",
            Tool::Rotate => "Rotate Tool",
            Tool::Scale => "Scale Tool",
            Tool::Joint => "Create Joints Tool",
            Tool::PaintWeights => "Paint Skin Weights Tool",
            Tool::EdgeLoop => "Insert Edge Loop Tool",
            Tool::MultiCut => "Multi-Cut Tool",
            Tool::Sculpt => "Sculpt Tool",
            Tool::QuadDraw => "Quad Draw Tool",
            Tool::CurveCv => "CV Curve Tool",
            Tool::CurveEp => "EP Curve Tool",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintOp {
    Replace,
    Add,
    Smooth,
}

#[derive(Clone, Debug)]
pub struct PaintSettings {
    /// Index into the skin's influence list.
    pub influence: usize,
    /// Brush radius in screen pixels.
    pub radius: f32,
    pub value: f32,
    pub op: PaintOp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompMode {
    Object,
    Vertex,
    Edge,
    Face,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Shading {
    Wireframe,
    Shaded,
    WireOnShaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MenuSet {
    Modeling,
    Rigging,
    Animation,
    Fx,
    Rendering,
}

impl MenuSet {
    pub const ALL: [MenuSet; 5] = [MenuSet::Modeling, MenuSet::Rigging, MenuSet::Animation, MenuSet::Fx, MenuSet::Rendering];
    pub fn name(self) -> &'static str {
        match self {
            MenuSet::Modeling => "Modeling",
            MenuSet::Rigging => "Rigging",
            MenuSet::Animation => "Animation",
            MenuSet::Fx => "FX",
            MenuSet::Rendering => "Rendering",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Workspace {
    MayaClassic,
    Modeling,
    Animation,
    Rigging,
}

impl Workspace {
    pub const ALL: [Workspace; 4] = [Workspace::MayaClassic, Workspace::Modeling, Workspace::Animation, Workspace::Rigging];
    pub fn name(self) -> &'static str {
        match self {
            Workspace::MayaClassic => "Maya Classic",
            Workspace::Modeling => "Modeling - Standard",
            Workspace::Animation => "Animation",
            Workspace::Rigging => "Rigging",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RightTab {
    ChannelBox,
    AttributeEditor,
    ToolSettings,
}

pub struct ViewPanel {
    pub cam: Camera,
    pub shading: Shading,
    pub show_grid: bool,
    pub show_hud: bool,
    /// Use the scene's lights (Maya: Lighting > Use All Lights, hotkey 7).
    pub all_lights: bool,
    pub shadows: bool,
    pub xray: bool,
    /// Show file textures (Maya hotkey 6).
    pub textured: bool,
    /// Show the UV checker on every mesh with UVs.
    pub uv_checker: bool,
    /// Camera node this view looks through.
    pub look_through: Option<NodeId>,
    /// Camera node pose last synced with this view (position, rotation).
    pub lt_pose: Option<(Vec3, glam::Quat)>,
}

impl ViewPanel {
    fn new(kind: CamKind) -> Self {
        ViewPanel {
            cam: Camera::new(kind),
            shading: if kind == CamKind::Persp { Shading::Shaded } else { Shading::Wireframe },
            show_grid: true,
            show_hud: true,
            all_lights: false,
            shadows: false,
            xray: false,
            textured: true,
            uv_checker: false,
            look_through: None,
            lt_pose: None,
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    scene: Scene,
    sel: Vec<NodeId>,
    comp_sel: BTreeSet<(NodeId, u32)>,
}

pub struct Panels {
    pub outliner: bool,
    pub right: bool,
    pub shelf: bool,
    pub toolbox: bool,
    pub timeline: bool,
    pub range: bool,
    pub cmdline: bool,
    pub helpline: bool,
    pub status: bool,
    pub graph_editor: bool,
    pub script_editor: bool,
    pub hotkeys: bool,
    pub about: bool,
    pub prefs: bool,
}

pub struct App {
    pub scene: Scene,
    /// Ordered selection; the last entry is the "lead" object.
    pub sel: Vec<NodeId>,
    pub comp_mode: CompMode,
    /// Selected components: (node, vertex index) in vertex mode or (node, face index) in face mode.
    pub comp_sel: BTreeSet<(NodeId, u32)>,
    pub tool: Tool,
    pub views: [ViewPanel; 4],
    pub quad: bool,
    pub active_view: usize,
    pub playing: bool,
    pub loop_playback: bool,
    pub auto_key: bool,
    pub snap_grid: bool,
    pub extrude_offset: f32,
    pub menu_set: MenuSet,
    pub workspace: Workspace,
    pub right_tab: RightTab,
    pub shelf_tab: usize,
    pub custom_shelf: Vec<(String, String)>,
    pub recent_files: Vec<PathBuf>,
    /// Saved AI prompts shown on the AI shelf (persisted with the AI settings).
    pub ai_shelf: Vec<(String, String)>,
    /// Requests from menus/shelves/command line for the AI copilot (drained by the UI shell).
    pub ai_requests: Vec<AiUiRequest>,
    /// Command line language: false = MEL, true = AI prompt.
    pub cmd_ai_mode: bool,
    pub script_ai_prompt: String,
    /// Last failing MEL (script, error) for "Fix with AI".
    pub last_mel_error: Option<(String, String)>,
    pub panels: Panels,
    pub cmd_input: String,
    pub cmd_result: String,
    pub script_input: String,
    pub log: Vec<String>,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    pub rename: Option<(NodeId, String)>,
    pub cb_rename: Option<(NodeId, String)>,
    pub selected_channels: BTreeSet<Channel>,
    pub outliner_filter: String,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(egui::Id, u64)>,
    pub frame_no: u64,
    cache: HashMap<NodeId, MeshCache>,
    geom_gen: u64,
    batch_depth: u32,
    /// Last built geometry and the scene fingerprint it was built from.
    geom_last: Option<(u64, Arc<FrameGeometry>)>,
    /// Per-node cached GPU geometry chunks.
    node_geom: HashMap<NodeId, NodeGeom>,
    pub play_accum: f32,
    pub graph_view: Option<(f32, f32, f32, f32)>,
    pub interact: crate::ui::viewport::Interact,
    batch: Option<bool>,
    pub paint: PaintSettings,
    pub joint_radius: f32,
    pub bevel_width: f32,
    pub mirror_axis: usize,
    pub merge_dist: f32,
    pub mirror_search: String,
    pub mirror_replace: String,
    pub cluster_falloff: f32,
    /// Hypershade window open / selected material.
    pub panels_hypershade: bool,
    pub panels_uv_editor: bool,
    pub panels_dope_sheet: bool,
    pub render: crate::render_view::RenderViewState,
    /// USD export: bake skinned meshes to point samples instead of UsdSkel.
    pub usd_bake_points: bool,
    pub panels_pose_lib: bool,
    pub panels_retarget: bool,
    /// Selected keys (Graph Editor / Dope Sheet).
    pub key_sel: Vec<crate::anim_ops::KeyRef>,
    pub key_clipboard: crate::anim_ops::KeyClipboard,
    /// Ghosted objects and motion trails.
    pub ghosts: BTreeSet<NodeId>,
    pub trails: BTreeSet<NodeId>,
    pub ghost_settings: crate::anim_ops::GhostSettings,
    /// Motion-trail key being dragged: (node, frame, world position at press).
    pub trail_drag: Option<(NodeId, f32, Vec3)>,
    /// Sculpt brush settings and the current stroke.
    pub sculpt: crate::sculpt::SculptSettings,
    pub sculpt_stroke: Option<crate::sculpt::SculptStroke>,
    /// Soft selection (B): falloff radius in world units.
    pub soft_select: bool,
    pub soft_radius: f32,
    pub lattice_div: [u32; 3],
    /// Multi-Cut points (mesh, edge, t).
    pub cut_points: Vec<(NodeId, (u32, u32), f32)>,
    /// Quad Draw tool state.
    pub qd: crate::quad_draw::QuadDraw,
    /// CV / EP Curve tool: points placed so far.
    pub curve_tool: crate::curve_ops::CurveTool,
    pub curve_opts: crate::curve_ops::CurveOpts,
    pub(crate) anim_overlay_cache: Option<(u64, Vec<GpuVertex>, Vec<GpuVertex>)>,
    /// Selected UVs (mesh, uv index) for the UV Editor.
    pub uv_sel: BTreeSet<(NodeId, u32)>,
    /// Loaded textures and the stable layer registry (layer 0 = UV checker).
    pub textures: crate::texture::TextureCache,
    pub tex_layers: Vec<String>,
    tex_set: Option<Arc<crate::texture::TextureSet>>,
    tex_failed: std::collections::HashSet<String>,
    tex_gen: u64,
    pub hs_selected: Option<u32>,
    /// Root of the chain being built with the joint tool.
    pub joint_chain: Option<NodeId>,
    pub panels_sdk: bool,
    pub sdk_driver: Option<(NodeId, Channel)>,
    pub sdk_driven: Option<(NodeId, Channel)>,
}

use eframe::egui;

pub const SEL_GREEN: [f32; 3] = [0.26, 1.0, 0.64];
pub const SEL_WHITE: [f32; 3] = [1.0, 1.0, 1.0];
pub const WIRE_BLUE: [f32; 3] = [0.0, 0.016, 0.373];

impl App {
    pub fn new() -> Self {
        let mut views = [
            ViewPanel::new(CamKind::Persp),
            ViewPanel::new(CamKind::Top),
            ViewPanel::new(CamKind::Front),
            ViewPanel::new(CamKind::Side),
        ];
        views[1].cam.ortho_half = 10.0;
        App {
            scene: Scene::default(),
            sel: Vec::new(),
            comp_mode: CompMode::Object,
            comp_sel: BTreeSet::new(),
            tool: Tool::Select,
            views,
            quad: false,
            active_view: 0,
            playing: false,
            loop_playback: true,
            auto_key: false,
            snap_grid: false,
            extrude_offset: 0.25,
            menu_set: MenuSet::Modeling,
            workspace: Workspace::MayaClassic,
            right_tab: RightTab::ChannelBox,
            shelf_tab: 1,
            custom_shelf: Vec::new(),
            recent_files: Vec::new(),
            ai_shelf: Vec::new(),
            ai_requests: Vec::new(),
            cmd_ai_mode: false,
            script_ai_prompt: String::new(),
            last_mel_error: None,
            panels: Panels {
                outliner: true,
                right: true,
                shelf: true,
                toolbox: true,
                timeline: true,
                range: true,
                cmdline: true,
                helpline: true,
                status: true,
                graph_editor: false,
                script_editor: false,
                hotkeys: false,
                about: false,
                prefs: false,
            },
            cmd_input: String::new(),
            cmd_result: String::new(),
            script_input: String::new(),
            log: vec!["// Rust Maya ready. Type `help` in the command line for commands.".into()],
            file_path: None,
            dirty: false,
            rename: None,
            cb_rename: None,
            selected_channels: BTreeSet::new(),
            outliner_filter: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            frame_no: 0,
            cache: HashMap::new(),
            geom_gen: 0,
            batch_depth: 0,
            geom_last: None,
            node_geom: HashMap::new(),
            play_accum: 0.0,
            graph_view: None,
            interact: Default::default(),
            batch: None,
            paint: PaintSettings { influence: 0, radius: 40.0, value: 1.0, op: PaintOp::Replace },
            joint_radius: 0.5,
            bevel_width: 0.1,
            mirror_axis: 0,
            merge_dist: 0.001,
            mirror_search: "L_".into(),
            mirror_replace: "R_".into(),
            cluster_falloff: 0.0,
            panels_hypershade: false,
            panels_uv_editor: false,
            panels_dope_sheet: false,
            render: Default::default(),
            usd_bake_points: false,
            panels_pose_lib: false,
            panels_retarget: false,
            key_sel: Vec::new(),
            key_clipboard: Vec::new(),
            ghosts: BTreeSet::new(),
            trails: BTreeSet::new(),
            ghost_settings: Default::default(),
            anim_overlay_cache: None,
            trail_drag: None,
            sculpt: Default::default(),
            sculpt_stroke: None,
            soft_select: false,
            soft_radius: 1.0,
            lattice_div: [2, 5, 2],
            cut_points: Vec::new(),
            qd: crate::quad_draw::QuadDraw { offset: 0.002, relax_iters: 5, ..Default::default() },
            curve_tool: crate::curve_ops::CurveTool { degree: 3, ..Default::default() },
            curve_opts: Default::default(),
            uv_sel: BTreeSet::new(),
            textures: Default::default(),
            tex_layers: vec![crate::texture::CHECKER.to_string()],
            tex_set: None,
            tex_failed: Default::default(),
            tex_gen: 0,
            hs_selected: None,
            joint_chain: None,
            panels_sdk: false,
            sdk_driver: None,
            sdk_driven: None,
        }
    }

    // ------------------------------------------------------------------ undo

    /// Save state before a mutation.
    pub fn checkpoint(&mut self) {
        // Inside a batch (one AI request) only the first mutation records an undo step.
        if let Some(recorded) = self.batch {
            if recorded {
                self.dirty = true;
                return;
            }
            self.batch = Some(true);
        }
        self.undo.push(Snapshot { scene: self.scene.clone(), sel: self.sel.clone(), comp_sel: self.comp_sel.clone() });
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    /// Checkpoint for continuous widget edits (drags): only once per interaction.
    pub fn edit_checkpoint(&mut self, id: egui::Id) {
        let continuing = self.last_edit.is_some_and(|(lid, f)| lid == id && self.frame_no - f <= 2);
        if !continuing {
            self.checkpoint();
        }
        self.last_edit = Some((id, self.frame_no));
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot { scene: self.scene.clone(), sel: self.sel.clone(), comp_sel: self.comp_sel.clone() });
            self.restore(s);
            self.info("Undo");
        } else {
            self.warn("There are no more commands to undo.");
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot { scene: self.scene.clone(), sel: self.sel.clone(), comp_sel: self.comp_sel.clone() });
            self.restore(s);
            self.info("Redo");
        } else {
            self.warn("There are no more commands to redo.");
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.scene = s.scene;
        self.sel = s.sel;
        self.comp_sel = s.comp_sel;
        self.dirty = true;
    }

    /// Group all following mutations into a single undo step (used for AI requests).
    pub fn begin_batch(&mut self) {
        if self.batch_depth == 0 {
            self.batch = Some(false);
        }
        self.batch_depth += 1;
    }
    pub fn end_batch(&mut self) {
        self.batch_depth = self.batch_depth.saturating_sub(1);
        if self.batch_depth == 0 {
            self.batch = None;
        }
    }

    pub fn set_view_kind(&mut self, view: usize, kind: CamKind) {
        let cam = &mut self.views[view].cam;
        *cam = Camera { kind, ..cam.clone() };
        if kind != CamKind::Persp {
            cam.target = Vec3::ZERO;
        }
    }

    pub fn clear_history_for_new_session(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // --------------------------------------------------------------- logging

    pub fn info(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        self.log.push(format!("// Result: {m}"));
        self.cmd_result = m;
    }
    pub fn warn(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        self.log.push(format!("// Warning: {m}"));
        self.cmd_result = format!("Warning: {m}");
    }

    // ------------------------------------------------------------ selection

    pub fn lead(&self) -> Option<NodeId> {
        self.sel.last().copied().filter(|id| self.scene.node(*id).is_some())
    }

    pub fn select_modify(&mut self, id: NodeId, shift: bool, ctrl: bool) {
        if shift && ctrl {
            self.sel.retain(|x| *x != id);
            self.sel.push(id);
        } else if shift {
            if self.sel.contains(&id) {
                self.sel.retain(|x| *x != id);
            } else {
                self.sel.push(id);
            }
        } else if ctrl {
            self.sel.retain(|x| *x != id);
        } else {
            self.sel = vec![id];
        }
    }

    pub fn clear_selection(&mut self) {
        if self.comp_mode == CompMode::Object {
            self.sel.clear();
        } else {
            self.comp_sel.clear();
        }
    }

    pub fn select_all(&mut self) {
        if self.comp_mode == CompMode::Object {
            self.sel = self.scene.nodes.iter().filter(|n| self.scene.is_visible(n.id)).map(|n| n.id).collect();
        } else {
            let mut cs = BTreeSet::new();
            for id in self.comp_targets() {
                if let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) {
                    let count = match self.comp_mode {
                        CompMode::Vertex => m.vertex_count(),
                        CompMode::Edge => m.edges().len(),
                        _ => m.face_count(),
                    };
                    for i in 0..count as u32 {
                        cs.insert((id, i));
                    }
                }
            }
            self.comp_sel = cs;
        }
    }

    pub fn invert_selection(&mut self) {
        let all: Vec<NodeId> = self.scene.nodes.iter().filter(|n| n.parent.is_none()).map(|n| n.id).collect();
        self.sel = all.into_iter().filter(|id| !self.sel.contains(id)).collect();
    }

    pub fn select_hierarchy(&mut self) {
        let mut out = Vec::new();
        for &id in &self.sel {
            for d in self.scene.subtree(id) {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
        self.sel = out;
    }

    /// Objects whose components are editable ("hilited" objects).
    pub fn comp_targets(&self) -> Vec<NodeId> {
        let picked: Vec<NodeId> = self
            .sel
            .iter()
            .copied()
            .filter(|id| self.scene.node(*id).is_some_and(|n| n.mesh().is_some() || (self.comp_mode == CompMode::Vertex && matches!(n.shape, Shape::Lattice { .. } | Shape::Curve { .. }))))
            .collect();
        picked
    }

    pub fn set_comp_mode(&mut self, m: CompMode) {
        if self.comp_mode != m {
            self.comp_sel.clear();
            self.comp_mode = m;
            if m != CompMode::Object && self.comp_targets().is_empty() {
                self.warn("Select a polygon object (or a curve / lattice for points) first to edit its components.");
            }
            self.info(match m {
                CompMode::Object => "Object mode",
                CompMode::Vertex => "Vertex mode",
                CompMode::Edge => "Edge mode",
                CompMode::Face => "Face mode",
            });
        }
    }

    /// Vertices affected by the current component selection, grouped per node.
    pub fn comp_vertices(&self) -> HashMap<NodeId, Vec<u32>> {
        let mut out: HashMap<NodeId, BTreeSet<u32>> = HashMap::new();
        for &(id, i) in &self.comp_sel {
            match self.comp_mode {
                CompMode::Vertex => {
                    out.entry(id).or_default().insert(i);
                }
                CompMode::Face => {
                    if let Some(f) = self.scene.node(id).and_then(|n| n.mesh()).and_then(|m| m.faces.get(i as usize)) {
                        out.entry(id).or_default().extend(f.iter().copied());
                    }
                }
                CompMode::Edge => {
                    if let Some(e) = self.scene.node(id).and_then(|n| n.mesh()).and_then(|m| m.edges().get(i as usize).copied()) {
                        out.entry(id).or_default().extend([e.0, e.1]);
                    }
                }
                CompMode::Object => {}
            }
        }
        out.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect()
    }

    /// Manipulator pivot in world space.
    pub fn pivot(&self) -> Option<Vec3> {
        if self.comp_mode != CompMode::Object {
            let cv = self.comp_vertices();
            let mut sum = Vec3::ZERO;
            let mut n = 0;
            for (id, verts) in &cv {
                let Some(pts) = self.scene.node(*id).and_then(|n| n.points()) else { continue };
                let w = self.scene.world_matrix(*id);
                for &v in verts {
                    if let Some(p) = pts.get(v as usize) {
                        sum += w.transform_point3(*p);
                        n += 1;
                    }
                }
            }
            return (n > 0).then(|| sum / n as f32);
        }
        self.lead().map(|id| self.scene.world_matrix(id).transform_point3(Vec3::ZERO))
    }

    // -------------------------------------------------------------- create

    pub fn create_primitive(&mut self, prim: Primitive) -> NodeId {
        self.checkpoint();
        let id = self.scene.add_primitive(prim);
        self.sel = vec![id];
        if self.comp_mode != CompMode::Object {
            self.comp_mode = CompMode::Object;
            self.comp_sel.clear();
        }
        let name = self.scene.node(id).unwrap().name.clone();
        self.info(name);
        id
    }

    pub fn create_locator(&mut self) -> NodeId {
        self.checkpoint();
        let id = self.scene.add_node("locator", Shape::Locator, None);
        self.sel = vec![id];
        id
    }

    pub fn create_group_empty(&mut self) -> NodeId {
        self.checkpoint();
        let id = self.scene.add_node("null", Shape::Group, None);
        self.sel = vec![id];
        id
    }

    // ---------------------------------------------------------------- edit

    pub fn delete_selected(&mut self) {
        match self.comp_mode {
            CompMode::Object => {
                if self.sel.is_empty() {
                    return;
                }
                self.checkpoint();
                let sel = std::mem::take(&mut self.sel);
                self.scene.delete(&sel);
            }
            CompMode::Edge => self.delete_edges(),
            CompMode::Face => {
                if self.comp_sel.is_empty() {
                    return;
                }
                self.checkpoint();
                let mut per: HashMap<NodeId, Vec<usize>> = HashMap::new();
                for &(id, f) in &self.comp_sel {
                    per.entry(id).or_default().push(f as usize);
                }
                for (id, faces) in per {
                    if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                        m.delete_faces(&faces);
                    }
                }
                self.comp_sel.clear();
            }
            CompMode::Vertex => {
                if self.comp_sel.is_empty() {
                    return;
                }
                self.checkpoint();
                let mut per: HashMap<NodeId, std::collections::HashSet<u32>> = HashMap::new();
                for &(id, v) in &self.comp_sel {
                    per.entry(id).or_default().insert(v);
                }
                for (id, verts) in per {
                    if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                        m.delete_vertices(&verts);
                    }
                }
                self.comp_sel.clear();
            }
        }
    }

    pub fn duplicate_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint();
        let roots = self.top_level(&self.sel.clone());
        let new = self.scene.duplicate(&roots);
        let names: Vec<String> = new.iter().filter_map(|id| self.scene.node(*id)).map(|n| n.name.clone()).collect();
        self.sel = new;
        self.info(names.join(" "));
    }

    /// Filter out nodes whose ancestor is also in the list.
    pub fn top_level(&self, ids: &[NodeId]) -> Vec<NodeId> {
        ids.iter().copied().filter(|id| !ids.iter().any(|other| other != id && self.scene.is_ancestor(*other, *id))).collect()
    }

    pub fn group_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint();
        let items = self.top_level(&self.sel.clone());
        let parent = self.scene.node(items[0]).and_then(|n| n.parent);
        let g = self.scene.add_node("group", Shape::Group, parent);
        for id in items {
            self.scene.reparent(id, Some(g));
        }
        self.sel = vec![g];
        let name = self.scene.node(g).unwrap().name.clone();
        self.info(name);
    }

    /// Maya "P": last selected becomes the parent of the others.
    pub fn parent_selected(&mut self) {
        if self.sel.len() < 2 {
            self.warn("Select the child objects, then the parent last.");
            return;
        }
        self.checkpoint();
        let parent = *self.sel.last().unwrap();
        let kids: Vec<NodeId> = self.sel[..self.sel.len() - 1].to_vec();
        for k in kids {
            if !self.scene.reparent(k, Some(parent)) {
                self.warn("Cannot parent a node to itself or its descendant.");
            }
        }
    }

    pub fn unparent_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint();
        for id in self.sel.clone() {
            self.scene.reparent(id, None);
        }
    }

    pub fn rename_node(&mut self, id: NodeId, new_name: &str) {
        let clean: String = new_name.trim().chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
        if clean.is_empty() || self.scene.node(id).is_some_and(|n| n.name == clean) {
            return;
        }
        self.checkpoint();
        let unique = if self.scene.name_exists(&clean) { self.scene.unique_name(&clean) } else { clean };
        if let Some(n) = self.scene.node_mut(id) {
            n.name = unique;
        }
    }

    pub fn toggle_visibility_selected(&mut self, show: bool) {
        self.checkpoint();
        for id in self.sel.clone() {
            if let Some(n) = self.scene.node_mut(id) {
                n.visible = show;
            }
        }
    }

    pub fn show_all(&mut self) {
        self.checkpoint();
        for n in &mut self.scene.nodes {
            n.visible = true;
        }
    }

    // ------------------------------------------------------------ modify

    pub fn freeze_transforms(&mut self) {
        self.checkpoint();
        for id in self.sel.clone() {
            let m = self.scene.node(id).map(|n| n.local_matrix()).unwrap_or(Mat4::IDENTITY);
            // Push the local transform into children & mesh.
            for c in self.scene.children(Some(id)) {
                if let Some(cn) = self.scene.node_mut(c) {
                    let lm = m * cn.local_matrix();
                    let (s, r, t) = lm.to_scale_rotation_translation();
                    let (z, y, x) = r.to_euler(glam::EulerRot::ZYX);
                    cn.translate = t;
                    cn.rotate = Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI);
                    cn.scale = s;
                }
            }
            if let Some(n) = self.scene.node_mut(id) {
                if let Some(mesh) = n.mesh_mut(true) {
                    for p in &mut mesh.positions {
                        *p = m.transform_point3(*p);
                    }
                    if m.determinant() < 0.0 {
                        mesh.reverse_normals();
                    }
                }
                n.translate = Vec3::ZERO;
                n.rotate = Vec3::ZERO;
                n.scale = Vec3::ONE;
            }
        }
        self.info("Freeze Transformations");
    }

    pub fn reset_transforms(&mut self) {
        self.checkpoint();
        for id in self.sel.clone() {
            if let Some(n) = self.scene.node_mut(id) {
                n.translate = Vec3::ZERO;
                n.rotate = Vec3::ZERO;
                n.scale = Vec3::ONE;
            }
        }
    }

    pub fn delete_history(&mut self) {
        self.checkpoint();
        for id in self.sel.clone() {
            if let Some(n) = self.scene.node_mut(id) {
                n.mesh_mut(true);
            }
        }
        self.info("Delete History");
    }

    fn mesh_op(&mut self, name: &str, f: impl Fn(&mut Mesh)) {
        let targets = self.comp_targets();
        if targets.is_empty() {
            self.warn(format!("{name}: select a polygon object."));
            return;
        }
        self.checkpoint();
        for id in targets {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                f(m);
            }
        }
        self.comp_sel.clear();
        self.info(name.to_string());
    }

    pub fn smooth(&mut self) {
        self.mesh_op("Smooth", |m| *m = m.catmull_clark());
    }
    pub fn triangulate(&mut self) {
        self.mesh_op("Triangulate", |m| m.triangulate_faces());
    }
    pub fn reverse_normals(&mut self) {
        self.mesh_op("Reverse", |m| m.reverse_normals());
    }

    pub fn extrude(&mut self) {
        if self.comp_mode != CompMode::Face || self.comp_sel.is_empty() {
            self.warn("Extrude: select faces first (F11 / face mode).");
            return;
        }
        self.checkpoint();
        let mut per: HashMap<NodeId, Vec<usize>> = HashMap::new();
        for &(id, f) in &self.comp_sel {
            per.entry(id).or_default().push(f as usize);
        }
        let off = self.extrude_offset;
        for (id, faces) in per {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                m.extrude_faces(&faces, off);
            }
        }
        self.tool = Tool::Move;
        self.info("polyExtrudeFace1");
    }

    pub fn combine(&mut self) {
        let targets = self.comp_targets();
        if targets.len() < 2 {
            self.warn("Combine: select two or more polygon objects.");
            return;
        }
        self.checkpoint();
        let mut out = Mesh::default();
        for &id in &targets {
            let w = self.scene.world_matrix(id);
            if let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) {
                let mut mm = m.clone();
                for p in &mut mm.positions {
                    *p = w.transform_point3(*p);
                }
                out.append(&mm);
            }
        }
        let color = self.scene.node(targets[0]).map(|n| n.color).unwrap_or([0.62; 3]);
        self.scene.delete(&targets);
        let id = self.scene.add_node("polySurface", Shape::Poly { history: None, mesh: out, rev: crate::scene::next_rev() }, None);
        self.scene.node_mut(id).unwrap().color = color;
        self.sel = vec![id];
        let name = self.scene.node(id).unwrap().name.clone();
        self.info(name);
    }

    pub fn separate(&mut self) {
        let Some(id) = self.lead() else { return };
        let Some(m) = self.scene.node(id).and_then(|n| n.mesh()).cloned() else { return };
        let shells = m.shells();
        if shells.len() < 2 {
            self.warn("Separate: the mesh has only one shell.");
            return;
        }
        self.checkpoint();
        let w = self.scene.world_matrix(id);
        let color = self.scene.node(id).unwrap().color;
        self.scene.delete(&[id]);
        let mut new = Vec::new();
        for mut s in shells {
            for p in &mut s.positions {
                *p = w.transform_point3(*p);
            }
            let nid = self.scene.add_node("polySurface", Shape::Poly { history: None, mesh: s, rev: crate::scene::next_rev() }, None);
            self.scene.node_mut(nid).unwrap().color = color;
            new.push(nid);
        }
        self.sel = new;
    }

    pub fn set_color_selected(&mut self, c: [f32; 3]) {
        for id in self.sel.clone() {
            if let Some(n) = self.scene.node_mut(id) {
                n.color = c;
                n.material = None;
                if let Shape::Light(l) = &mut n.shape {
                    l.color = c;
                }
            }
        }
    }

    // ------------------------------------------------------------ animation

    /// Go to time `t`: animation curves, then the rig (driven keys, constraints, IK) and
    /// simulations, so renders and batch jobs see the same pose as the viewport.
    pub fn set_time(&mut self, t: f32) {
        self.scene.time = t;
        self.scene.evaluate();
        crate::rig::evaluate(&mut self.scene);
    }

    pub fn key_selected(&mut self, channels: &[Channel]) {
        if self.sel.is_empty() {
            self.warn("Nothing selected to key.");
            return;
        }
        self.checkpoint();
        for id in self.sel.clone() {
            self.scene.set_key(id, channels);
        }
        let t = self.scene.time;
        self.info(format!("{} key(s) set at frame {t}", channels.len() * self.sel.len()));
    }

    /// Auto key: re-key channels that already carry animation.
    pub fn auto_key_touch(&mut self, ids: &[NodeId]) {
        if !self.auto_key {
            return;
        }
        for &id in ids {
            let chans: Vec<Channel> = self.scene.node(id).map(|n| n.anim.keys().copied().collect()).unwrap_or_default();
            if !chans.is_empty() {
                self.scene.set_key(id, &chans);
            }
        }
    }

    pub fn delete_keys_selected(&mut self, at_current_only: bool) {
        self.checkpoint();
        let t = self.scene.time;
        for id in self.sel.clone() {
            if let Some(n) = self.scene.node_mut(id) {
                if at_current_only {
                    for c in n.anim.values_mut() {
                        c.remove_key(t);
                    }
                    n.anim.retain(|_, c| !c.keys.is_empty());
                } else {
                    n.anim.clear();
                }
            }
        }
    }

    pub fn step_key(&mut self, forward: bool) {
        let keys = self.scene.key_frames(&self.sel);
        let t = self.scene.time;
        let next = if forward {
            keys.iter().copied().find(|k| *k > t + 1e-4).or(keys.first().copied())
        } else {
            keys.iter().rev().copied().find(|k| *k < t - 1e-4).or(keys.last().copied())
        };
        if let Some(k) = next {
            self.set_time(k);
        }
    }

    pub fn advance_playback(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        self.play_accum += dt * self.scene.fps;
        let steps = self.play_accum.floor();
        if steps >= 1.0 {
            self.play_accum -= steps;
            let mut t = self.scene.time.round() + steps;
            if t > self.scene.play_end {
                if self.loop_playback {
                    t = self.scene.play_start;
                } else {
                    t = self.scene.play_end;
                    self.playing = false;
                }
            }
            self.set_time(t);
        }
    }

    // ---------------------------------------------------------------- layers

    pub fn new_layer(&mut self, from_selection: bool) {
        self.checkpoint();
        let id = self.scene.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        let n = self.scene.layers.len() + 1;
        let palette = [[0.9, 0.4, 0.4], [0.4, 0.8, 0.4], [0.4, 0.6, 0.95], [0.9, 0.8, 0.3], [0.8, 0.5, 0.9]];
        self.scene.layers.push(Layer { id, name: format!("layer{n}"), visible: true, template: false, color: palette[(n - 1) % 5] });
        if from_selection {
            for s in self.sel.clone() {
                if let Some(nd) = self.scene.node_mut(s) {
                    nd.layer = Some(id);
                }
            }
        }
    }

    // ------------------------------------------------------------------ file

    pub fn new_scene(&mut self) {
        self.checkpoint();
        self.scene = Scene::default();
        self.sel.clear();
        self.comp_sel.clear();
        self.comp_mode = CompMode::Object;
        self.file_path = None;
        self.dirty = false;
        self.cache.clear();
    }

    pub fn save_to(&mut self, path: PathBuf) {
        self.scene.render_settings = Some(self.render.settings.clone());
        self.scene.render_camera = match self.render.camera {
            crate::render_view::RenderCam::Node(id) => self.scene.node(id).map(|n| n.name.clone()),
            crate::render_view::RenderCam::ActiveView => None,
        };
        match serde_json::to_string_pretty(&self.scene).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.info(format!("Saved {}", path.display()));
                crate::prefs::push_recent(&mut self.recent_files, &path);
                self.file_path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.warn(format!("Save failed: {e}")),
        }
    }

    pub fn open_from(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str::<Scene>(&s).map_err(|e| e.to_string())) {
            Ok(scene) => {
                self.checkpoint();
                self.scene = scene;
                self.scene.evaluate();
                if let Some(s) = self.scene.render_settings.clone() {
                    self.render.settings = s;
                }
                self.render.camera = self
                    .scene
                    .render_camera
                    .as_deref()
                    .and_then(|n| self.scene.find_by_name(n))
                    .filter(|&id| self.is_camera(id))
                    .map(crate::render_view::RenderCam::Node)
                    .unwrap_or(crate::render_view::RenderCam::ActiveView);
                self.sel.clear();
                self.comp_sel.clear();
                self.comp_mode = CompMode::Object;
                self.info(format!("Opened {}", path.display()));
                crate::prefs::push_recent(&mut self.recent_files, &path);
                self.file_path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.warn(format!("Open failed: {e}")),
        }
    }

    /// Load an autosave as an unsaved scene (Save will ask for a file name).
    pub fn recover_from(&mut self, path: PathBuf) {
        self.open_from(path.clone());
        if self.file_path.as_ref() == Some(&path) {
            self.recent_files.retain(|p| p != &path);
            self.file_path = None;
            self.dirty = true;
            self.info(format!("Recovered {} — use Save As to keep it", path.display()));
        }
    }

    pub fn export_obj(&mut self, path: PathBuf, selection_only: bool) {
        let ids: Vec<NodeId> = if selection_only {
            self.sel.iter().flat_map(|id| self.scene.subtree(*id)).collect()
        } else {
            self.scene.nodes.iter().map(|n| n.id).collect()
        };
        let mut objs = Vec::new();
        let mut mtl_for: Vec<(String, String, [f32; 3], Option<String>)> = Vec::new();
        for id in ids {
            let Some(n) = self.scene.node(id) else { continue };
            let Some(m) = n.mesh() else { continue };
            let w = self.scene.world_matrix(id);
            let mut mm = m.clone();
            for p in &mut mm.positions {
                *p = w.transform_point3(*p);
            }
            objs.push((n.name.clone(), mm));
            let (mname, col, map) = match n.material.and_then(|m| self.material(m)) {
                Some(m) => (m.name.clone(), m.color, m.color_map.clone().filter(|t| t != crate::texture::CHECKER)),
                None => (format!("lambert1_{}", n.name), n.color, None),
            };
            mtl_for.push((n.name.clone(), mname, col, map));
        }
        // Companion .mtl with colours and colour maps.
        let mtl_path = path.with_extension("mtl");
        let mut mtl = String::from("# Rust Maya materials\n");
        let mut done = std::collections::HashSet::new();
        for (_, mname, c, map) in &mtl_for {
            if done.insert(mname.clone()) {
                mtl.push_str(&format!("newmtl {mname}\nKd {} {} {}\n", c[0], c[1], c[2]));
                if let Some(m) = map {
                    mtl.push_str(&format!("map_Kd {m}\n"));
                }
            }
        }
        let mut text = format!("mtllib {}\n", mtl_path.file_name().unwrap().to_string_lossy());
        let body = mesh::to_obj(&objs);
        // Insert usemtl after each object line.
        for line in body.lines() {
            text.push_str(line);
            text.push('\n');
            if let Some(name) = line.strip_prefix("o ")
                && let Some((_, m, _, _)) = mtl_for.iter().find(|x| x.0 == name)
            {
                text.push_str(&format!("usemtl {m}\n"));
            }
        }
        let _ = std::fs::write(&mtl_path, mtl);
        match std::fs::write(&path, text) {
            Ok(()) => self.info(format!("Exported {} object(s) to {}", objs.len(), path.display())),
            Err(e) => self.warn(format!("Export failed: {e}")),
        }
    }

    pub fn import_obj(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let objs = mesh::from_obj(&text);
                if objs.is_empty() {
                    self.warn("No polygon objects found in file.");
                    return;
                }
                self.begin_batch();
                self.checkpoint();
                // Materials from the .mtl (Kd colour, map_Kd texture), first usemtl per object.
                let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let mut obj_mat: HashMap<String, String> = HashMap::new();
                let mut mtl_files = Vec::new();
                let mut cur = String::from("imported");
                for line in text.lines() {
                    let mut it = line.split_whitespace();
                    match it.next() {
                        Some("mtllib") => mtl_files.push(it.collect::<Vec<_>>().join(" ")),
                        Some("o") | Some("g") => cur = it.collect::<Vec<_>>().join("_"),
                        Some("usemtl") => {
                            obj_mat.entry(cur.clone()).or_insert_with(|| it.collect::<Vec<_>>().join(" "));
                        }
                        _ => {}
                    }
                }
                let mut mtl: HashMap<String, ([f32; 3], Option<String>)> = HashMap::new();
                for f in &mtl_files {
                    let Ok(t) = std::fs::read_to_string(dir.join(f)) else { continue };
                    let mut name = String::new();
                    for line in t.lines() {
                        let mut it = line.split_whitespace();
                        match it.next() {
                            Some("newmtl") => {
                                name = it.collect::<Vec<_>>().join(" ");
                                mtl.insert(name.clone(), ([0.8; 3], None));
                            }
                            Some("Kd") => {
                                let c: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                                if c.len() >= 3 && let Some(e) = mtl.get_mut(&name) {
                                    e.0 = [c[0], c[1], c[2]];
                                }
                            }
                            Some("map_Kd") => {
                                // The file name is the last token (options may precede it).
                                if let (Some(tex), Some(e)) = (line.split_whitespace().last(), mtl.get_mut(&name)) {
                                    let p = std::path::Path::new(tex);
                                    let full = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
                                    e.1 = Some(full.display().to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let mut made: HashMap<String, u32> = HashMap::new();
                let mut new = Vec::new();
                for (name, m) in objs {
                    let id = self.scene.add_node(&name, Shape::Poly { history: None, mesh: m, rev: crate::scene::next_rev() }, None);
                    self.apply_name(id, Some(&name));
                    if let Some(mn) = obj_mat.get(&name)
                        && let Some((kd, map)) = mtl.get(mn).cloned()
                    {
                        let mid = match made.get(mn) {
                            Some(m) => *m,
                            None => {
                                let m = self.create_material(crate::look::MatKind::Lambert, Some(mn));
                                if let Some(x) = self.material_mut(m) {
                                    x.color = kd;
                                    x.color_map = map.filter(|p| std::path::Path::new(p).exists());
                                }
                                made.insert(mn.clone(), m);
                                m
                            }
                        };
                        if let Some(n) = self.scene.node_mut(id) {
                            n.material = Some(mid);
                        }
                    }
                    new.push(id);
                }
                self.end_batch();
                self.info(format!("Imported {} object(s), {} material(s)", new.len(), made.len()));
                self.sel = new;
            }
            Err(e) => self.warn(format!("Import failed: {e}")),
        }
    }

    // ----------------------------------------------------------- viewing

    pub fn world_bounds(&self, ids: &[NodeId]) -> Option<(Vec3, Vec3)> {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut any = false;
        for &id in ids {
            let w = self.scene.world_matrix(id);
            let pts: Vec<Vec3> = match self.scene.node(id) {
                Some(n) if n.mesh().is_some() => n.mesh().unwrap().positions.clone(),
                Some(n) if matches!(n.shape, Shape::Fluid(_)) => {
                    let Shape::Fluid(f) = &n.shape else { unreachable!() };
                    crate::fluid::box_lines(&f.settings).into_iter().flat_map(|(a, b)| [a, b]).collect()
                }
                _ => vec![Vec3::ZERO],
            };
            for p in pts {
                let q = w.transform_point3(p);
                lo = lo.min(q);
                hi = hi.max(q);
                any = true;
            }
        }
        any.then_some((lo, hi))
    }

    pub fn frame(&mut self, view: usize, selected: bool) {
        let ids: Vec<NodeId> = if selected && !self.sel.is_empty() {
            self.sel.iter().flat_map(|id| self.scene.subtree(*id)).collect()
        } else {
            self.scene.nodes.iter().filter(|n| self.scene.is_visible(n.id)).map(|n| n.id).collect()
        };
        let b = if selected && self.comp_mode != CompMode::Object && !self.comp_sel.is_empty() {
            self.pivot().map(|p| (p - Vec3::splat(0.5), p + Vec3::splat(0.5)))
        } else {
            self.world_bounds(&ids)
        };
        match b {
            Some((lo, hi)) => self.views[view].cam.frame(lo, hi),
            None => self.views[view].cam.frame(Vec3::splat(-6.0), Vec3::splat(6.0)),
        }
    }

    // ------------------------------------------------------- render geometry

    pub fn tri_cache(&mut self, id: NodeId) -> Option<&MeshCache> {
        let node = self.scene.node(id)?;
        let mesh = node.mesh()?;
        let rev = node.mesh_rev();
        let stale = self.cache.get(&id).is_none_or(|c| c.rev != rev);
        if stale {
            let tri = mesh.triangulate();
            self.cache.insert(id, MeshCache { rev, tri, vert_faces: None });
        }
        self.cache.get(&id)
    }

    /// GPU layer index for a texture name (registers and loads it on first use).
    /// None if the image can't be loaded (a warning is shown once).
    pub fn texture_layer(&mut self, name: &str) -> Option<usize> {
        if let Some(i) = self.tex_layers.iter().position(|n| n == name) {
            return Some(i);
        }
        match self.textures.get(name) {
            Ok(_) => {
                self.tex_layers.push(name.to_string());
                self.tex_set = None;
                Some(self.tex_layers.len() - 1)
            }
            Err(e) => {
                if !self.tex_failed.contains(name) {
                    self.tex_failed.insert(name.to_string());
                    self.warn(format!("Texture not loaded: {e}"));
                }
                None
            }
        }
    }

    fn texture_set(&mut self) -> Arc<crate::texture::TextureSet> {
        if let Some(t) = &self.tex_set {
            return t.clone();
        }
        let mut layers = Vec::new();
        let mut names = Vec::new();
        for n in self.tex_layers.clone() {
            // Keep layer indices stable: a file that stopped loading shows the checker.
            let img = self.textures.get(&n).or_else(|_| self.textures.get(crate::texture::CHECKER));
            if let Ok(img) = img {
                layers.push(img);
                names.push(n);
            }
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        names.hash(&mut h);
        self.tex_gen += 1;
        self.tex_gen.hash(&mut h);
        let t = Arc::new(crate::texture::TextureSet { key: h.finish(), names, layers });
        self.tex_set = Some(t.clone());
        t
    }

    /// Re-read a texture file from disk (after editing it elsewhere).
    pub fn reload_texture(&mut self, name: &str) {
        self.textures.reload(name);
        self.tex_failed.remove(name);
        self.tex_set = None;
        self.geom_last = None;
        self.node_geom.clear();
    }

    /// Fingerprint of everything that affects one node's drawn geometry.
    fn node_key(&self, id: NodeId, lead: Option<NodeId>, comp_targets: &[NodeId]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let Some(n) = self.scene.node(id) else { return 0 };
        id.hash(&mut h);
        self.scene.is_visible(id).hash(&mut h);
        self.scene.is_template(id).hash(&mut h);
        hash_mat4(&mut h, &self.scene.world_matrix(id));
        for c in n.color {
            h.write_u32(c.to_bits());
        }
        (self.sel.contains(&id), lead == Some(id), comp_targets.contains(&id)).hash(&mut h);
        if let Some(c) = &n.cloth {
            (c.cache.version, c.cache.current).hash(&mut h);
        }
        match &n.shape {
            Shape::Poly { .. } => n.mesh_rev().hash(&mut h),
            Shape::Particles(ps) => {
                (ps.cache.version, ps.cache.current, ps.cache.key).hash(&mut h);
                format!("{:?}", ps.settings).hash(&mut h);
            }
            Shape::Hair(hs) => {
                (hs.cache.version, hs.cache.current, hs.cache.key).hash(&mut h);
                format!("{:?}", hs.settings).hash(&mut h);
            }
            Shape::Fluid(fl) => {
                (fl.cache.version, fl.cache.current, fl.cache.key).hash(&mut h);
                format!("{:?}", fl.settings).hash(&mut h);
            }
            other => format!("{other:?}").hash(&mut h),
        }
        if let Some(m) = n.material.and_then(|m| self.material(m)) {
            format!("{m:?}").hash(&mut h);
        }
        if self.comp_mode == CompMode::Face {
            for (nid, f) in self.comp_sel.iter() {
                if *nid == id {
                    f.hash(&mut h);
                }
            }
        }
        // Deformer inputs.
        if let Some(sk) = &n.skin {
            sk.weights.len().hash(&mut h);
            for j in &sk.joints {
                hash_mat4(&mut h, &self.scene.world_matrix(*j));
            }
        }
        if let Some(b) = &n.blend {
            b.targets.len().hash(&mut h);
            for w in &b.weights {
                h.write_u32(w.to_bits());
            }
        }
        for c in &n.clusters {
            hash_mat4(&mut h, &self.scene.world_matrix(c.handle));
            c.weights.len().hash(&mut h);
        }
        for wb in &n.wires {
            hash_mat4(&mut h, &self.scene.world_matrix(wb.curve));
            (wb.binds.len(), wb.envelope.to_bits(), wb.rotation.to_bits()).hash(&mut h);
            if let Some(Shape::Curve { points, .. }) = self.scene.node(wb.curve).map(|x| &x.shape) {
                for p in points {
                    (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()).hash(&mut h);
                }
            }
        }
        for b in &n.lattices {
            hash_mat4(&mut h, &self.scene.world_matrix(b.lattice));
            if let Some(Shape::Lattice { points, .. }) = self.scene.node(b.lattice).map(|x| &x.shape) {
                for p in points {
                    (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()).hash(&mut h);
                }
            }
        }
        if let Some(w) = &n.wrap
            && let Some(d) = self.scene.node(w.driver)
        {
            hash_mat4(&mut h, &self.scene.world_matrix(w.driver));
            d.mesh_rev().hash(&mut h);
            // Driver deformation (skin joints, blend weights...).
            if let Some(sk) = &d.skin {
                for j in &sk.joints {
                    hash_mat4(&mut h, &self.scene.world_matrix(*j));
                }
            }
            if let Some(b) = &d.blend {
                for x in &b.weights {
                    h.write_u32(x.to_bits());
                }
            }
        }
        if self.soft_select && self.comp_mode != CompMode::Object && self.comp_targets().contains(&id) {
            // Soft-selection colours depend on the selection.
            (self.soft_radius.to_bits(), self.comp_sel.len()).hash(&mut h);
            for (nid, v) in &self.comp_sel {
                if *nid == id {
                    v.hash(&mut h);
                }
            }
        }
        if self.tool == Tool::PaintWeights && lead == Some(id) {
            // Weights change while painting: always rebuild the painted mesh.
            (self.paint.influence, self.frame_no).hash(&mut h);
        }
        h.finish()
    }

    pub fn build_geometry(&mut self) -> Arc<FrameGeometry> {
        let lead = self.lead();
        let comp_targets = if self.comp_mode != CompMode::Object { self.comp_targets() } else { Vec::new() };
        // Whole-scene fingerprint: reuse the last geometry (and skip the GPU upload) if nothing changed.
        let keys: Vec<(NodeId, u64)> = self.scene.nodes.iter().map(|n| (n.id, self.node_key(n.id, lead, &comp_targets))).collect();
        let global = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            keys.hash(&mut h);
            (self.comp_mode as u8, self.tool.name(), self.scene.layers.len()).hash(&mut h);
            for l in &self.scene.layers {
                (l.id, l.visible, l.template).hash(&mut h);
            }
            if !self.ghosts.is_empty() || !self.trails.is_empty() {
                self.anim_fingerprint().hash(&mut h);
            }
            h.finish()
        };
        if let Some((k, g)) = &self.geom_last
            && *k == global
        {
            return g.clone();
        }
        let key_of: HashMap<NodeId, u64> = keys.into_iter().collect();
        self.geom_gen += 1;
        let mut g = FrameGeometry { generation: self.geom_gen, ..Default::default() };
        let live: std::collections::HashSet<NodeId> = self.scene.nodes.iter().map(|n| n.id).collect();
        self.cache.retain(|id, _| live.contains(id));
        self.node_geom.retain(|id, _| live.contains(id));
        let face_sel: std::collections::HashSet<(NodeId, u32)> =
            if self.comp_mode == CompMode::Face { self.comp_sel.iter().copied().collect() } else { Default::default() };
        let mut extra_wire: Vec<GpuVertex> = Vec::new();
        let ids: Vec<NodeId> = self.scene.nodes.iter().map(|n| n.id).collect();
        for id in ids {
            if !self.scene.is_visible(id) {
                continue;
            }
            let template = self.scene.is_template(id);
            let w = self.scene.world_matrix(id);
            let nm = Mat4::from_mat3(glam::Mat3::from_mat4(w).inverse().transpose());
            let node = self.scene.node(id).unwrap();
            let color = node.color;
            let is_locator = matches!(node.shape, Shape::Locator);
            let selected = self.sel.contains(&id);
            let hilite = comp_targets.contains(&id);
            let wire_col = if hilite {
                [0.6, 0.85, 1.0]
            } else if template {
                [0.55, 0.55, 0.55]
            } else if Some(id) == lead {
                SEL_GREEN
            } else if selected {
                SEL_WHITE
            } else {
                WIRE_BLUE
            };
            if is_locator {
                let c = if selected { wire_col } else { [0.1, 0.1, 0.55] };
                for ax in [Vec3::X, Vec3::Y, Vec3::Z] {
                    let a = w.transform_point3(-ax);
                    let b = w.transform_point3(ax);
                    for v in [a, b] {
                        let vert = GpuVertex::line(v, c);
                        extra_wire.push(vert);
                    }
                }
                continue;
            }
            if let Shape::Fluid(fl) = &node.shape {
                let c = if selected { wire_col } else { [0.45, 0.75, 0.95] };
                for (a, b) in crate::fluid::box_lines(&fl.settings) {
                    extra_wire.push(GpuVertex::line(w.transform_point3(a), c));
                    extra_wire.push(GpuVertex::line(w.transform_point3(b), c));
                }
                let tris = crate::fluid::preview_tris(fl, w);
                if !tris.is_empty() {
                    let lo = tris.iter().fold(Vec3::splat(f32::MAX), |a, t| a.min(t.0));
                    let hi = tris.iter().fold(Vec3::splat(f32::MIN), |a, t| a.max(t.0));
                    let verts: Vec<GpuVertex> = tris
                        .iter()
                        .map(|(p, n, col)| GpuVertex { pos: p.to_array(), nrm: n.to_array(), col: *col, mat: GpuVertex::DEFAULT_MAT, emi: [0.0; 4], uv: [0.0; 2], maps: [0.0; 4] })
                        .collect();
                    g.tris_transp.push(Chunk::preview(verts, Some((lo, hi))));
                    g.transp_nodes.push(id);
                }
                if let Some(v) = crate::fluid::VolumeGeom::from_fluid(id, w, fl) {
                    g.volumes.push(Arc::new(v));
                }
                continue;
            }
            if let Shape::Field(f) = &node.shape {
                let c = if selected { wire_col } else { [0.55, 0.9, 0.8] };
                for (a, b) in crate::fx::field_icon(f) {
                    extra_wire.push(GpuVertex::line(w.transform_point3(a), c));
                    extra_wire.push(GpuVertex::line(w.transform_point3(b), c));
                }
                continue;
            }
            if let Shape::Hair(hs) = &node.shape {
                if selected {
                    for st in hs.strands().iter().take(600) {
                        for seg in st.windows(2) {
                            extra_wire.push(GpuVertex::line(seg[0], wire_col));
                            extra_wire.push(GpuVertex::line(seg[1], wire_col));
                        }
                    }
                }
                let tris = crate::hair::hair_tris(hs);
                if !tris.is_empty() {
                    let lo = tris.iter().fold(Vec3::splat(f32::MAX), |a, t| a.min(t.0));
                    let hi = tris.iter().fold(Vec3::splat(f32::MIN), |a, t| a.max(t.0));
                    let verts: Vec<GpuVertex> = tris
                        .iter()
                        .map(|(p, n, col)| GpuVertex { pos: p.to_array(), nrm: n.to_array(), col: [col[0], col[1], col[2], 1.0], mat: GpuVertex::DEFAULT_MAT, emi: [0.0; 4], uv: [0.0; 2], maps: [0.0; 4] })
                        .collect();
                    g.tris.push(Chunk::with_bounds(verts, Some((lo, hi))));
                    g.tri_nodes.push(id);
                }
                continue;
            }
            if let Shape::Particles(ps) = &node.shape {
                // Emitter icon (a small star) + particles as shaded octahedra.
                let c = if selected { wire_col } else { [1.0, 0.65, 0.3] };
                let o = w.transform_point3(Vec3::ZERO);
                for d in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 0.0).normalize(), Vec3::new(1.0, -1.0, 0.0).normalize(), Vec3::new(0.0, 1.0, 1.0).normalize(), Vec3::new(0.0, 1.0, -1.0).normalize()] {
                    extra_wire.push(GpuVertex::line(o - d * 0.25, c));
                    extra_wire.push(GpuVertex::line(o + d * 0.25, c));
                }
                let tris = crate::fx::particle_tris(ps);
                if !tris.is_empty() {
                    let lo = tris.iter().fold(Vec3::splat(f32::MAX), |a, t| a.min(t.0));
                    let hi = tris.iter().fold(Vec3::splat(f32::MIN), |a, t| a.max(t.0));
                    let verts: Vec<GpuVertex> = tris
                        .iter()
                        .map(|(p, n, col)| GpuVertex { pos: p.to_array(), nrm: n.to_array(), col: [col[0], col[1], col[2], 1.0], mat: GpuVertex::DEFAULT_MAT, emi: [0.0; 4], uv: [0.0; 2], maps: [0.0; 4] })
                        .collect();
                    g.tris.push(Chunk::with_bounds(verts, Some((lo, hi))));
                    g.tri_nodes.push(id);
                }
                continue;
            }
            if let Shape::Lattice { div, points } = &node.shape {
                let c = if selected { wire_col } else { [0.7, 0.5, 0.95] };
                for (a, b) in crate::deform::lattice_lines(*div, points) {
                    extra_wire.push(GpuVertex::line(w.transform_point3(a), c));
                    extra_wire.push(GpuVertex::line(w.transform_point3(b), c));
                }
                continue;
            }
            if let Shape::Light(_) | Shape::Camera(_) = &node.shape {
                let c = if selected { wire_col } else if matches!(node.shape, Shape::Light(_)) { [0.95, 0.85, 0.35] } else { [0.75, 0.75, 0.8] };
                let is_cam = matches!(node.shape, Shape::Camera(_));
                let mut cam_v = Vec::new();
                for (a, b) in crate::look::icon_lines(&node.shape) {
                    for v in [w.transform_point3(a), w.transform_point3(b)] {
                        if is_cam {
                            cam_v.push(GpuVertex::line(v, c));
                        } else {
                            extra_wire.push(GpuVertex::line(v, c));
                        }
                    }
                }
                if is_cam {
                    g.camera_icons.push((id, cam_v));
                }
                continue;
            }
            // Rig objects are drawn on top of the shaded scene (overlay pass).
            let rig_col = |base: [f32; 3]| if Some(id) == lead { SEL_GREEN } else if selected { SEL_WHITE } else { base };
            let mut line = |a: Vec3, b: Vec3, c: [f32; 3]| {
                g.overlay.push(GpuVertex::line(a, c));
                g.overlay.push(GpuVertex::line(b, c));
            };
            match &node.shape {
                Shape::Joint { radius } => {
                    let c = rig_col([0.3, 0.55, 1.0]);
                    let p = w.transform_point3(Vec3::ZERO);
                    let r = crate::rig::world_rotation(&self.scene, id);
                    let rad = radius * 0.5;
                    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                        let pts = crate::rig::circle_points(rad, r * axis, 12);
                        for i in 0..pts.len() {
                            line(p + pts[i], p + pts[(i + 1) % pts.len()], c);
                        }
                    }
                    for k in crate::rig::child_joints(&self.scene, id) {
                        let kp = crate::rig::world_pos(&self.scene, k);
                        let dir = kp - p;
                        if dir.length() < 1e-5 {
                            continue;
                        }
                        let u = dir.normalize().any_orthonormal_vector() * rad;
                        let v = dir.normalize().cross(u.normalize()) * rad;
                        let base = [p + u, p + v, p - u, p - v];
                        for i in 0..4 {
                            line(base[i], kp, c);
                            line(base[i], base[(i + 1) % 4], c);
                        }
                    }
                    continue;
                }
                Shape::IkHandle { start, end, pole, .. } => {
                    let c = rig_col([0.95, 0.75, 0.2]);
                    let h = w.transform_point3(Vec3::ZERO);
                    if self.scene.node(*start).is_some() {
                        line(crate::rig::world_pos(&self.scene, *start), h, c);
                    }
                    for ax in [Vec3::X, Vec3::Y, Vec3::Z] {
                        line(h - ax * 0.3, h + ax * 0.3, c);
                    }
                    if let (Some(pl), Some(mid)) = (pole.filter(|p| self.scene.node(*p).is_some()), self.scene.node(*end).and_then(|n| n.parent)) {
                        let a = crate::rig::world_pos(&self.scene, mid);
                        let b = crate::rig::world_pos(&self.scene, pl);
                        // dashed pole-vector line
                        for i in 0..10 {
                            if i % 2 == 0 {
                                line(a.lerp(b, i as f32 / 10.0), a.lerp(b, (i + 1) as f32 / 10.0), [0.7, 0.7, 0.7]);
                            }
                        }
                    }
                    continue;
                }
                Shape::ClusterHandle => {
                    // A "C" glyph like Maya's cluster handle.
                    let c = rig_col([0.9, 0.9, 0.5]);
                    let h = w.transform_point3(Vec3::ZERO);
                    let r = crate::rig::world_rotation(&self.scene, id);
                    for i in 0..9 {
                        let a0 = (40.0 + i as f32 * 31.0).to_radians();
                        let a1 = (40.0 + (i + 1) as f32 * 31.0).to_radians();
                        let p0 = h + r * Vec3::new(a0.cos(), a0.sin(), 0.0) * 0.25;
                        let p1 = h + r * Vec3::new(a1.cos(), a1.sin(), 0.0) * 0.25;
                        line(p0, p1, c);
                    }
                    continue;
                }
                Shape::Curve { closed, .. } => {
                    let c = rig_col(color);
                    let cv = crate::nurbs::Curve::from_shape(&node.shape).unwrap();
                    let pts: Vec<Vec3> = cv.tessellate(12).iter().map(|p| w.transform_point3(*p)).collect();
                    let n = pts.len();
                    let segs = if *closed { n } else { n.saturating_sub(1) };
                    for i in 0..segs {
                        line(pts[i], pts[(i + 1) % n], c);
                    }
                    continue;
                }
                _ => {}
            }
            let in_sel = selected || hilite || template;
            let key = key_of.get(&id).copied().unwrap_or(0);
            if let Some(c) = self.node_geom.get(&id)
                && c.key == key
            {
                if let Some(t) = &c.tris {
                    if c.transp {
                        g.tris_transp.push(t.clone());
                        g.transp_nodes.push(id);
                    } else {
                        g.tris.push(t.clone());
                        g.tri_nodes.push(id);
                    }
                }
                if let Some(wc) = &c.wire {
                    g.wire_all.push(wc.clone());
                    if in_sel {
                        g.wire_sel.push(wc.clone());
                    }
                }
                continue;
            }
            // Deformers (blend shape → skin → clusters) give world-space positions directly.
            let def_pos: Option<Vec<Vec3>> = crate::rig::deformed_positions(&self.scene, id);
            let viz: Option<Vec<f32>> = (self.tool == Tool::PaintWeights && Some(id) == lead)
                .then(|| node.skin.as_ref().map(|sk| (0..sk.weights.len()).map(|v| sk.weight(v, self.paint.influence)).collect()))
                .flatten();
            let mat = node.material.and_then(|m| self.material(m)).cloned();
            let layer = mat.as_ref().and_then(|m| m.color_map.clone()).and_then(|t| self.texture_layer(&t)).map(|l| l as f32 + 1.0).unwrap_or(0.0);
            let repeat = mat.as_ref().map(|m| Vec2::from(m.repeat_uv)).unwrap_or(Vec2::ONE);
            let mut maps = [0.0f32; 4];
            if let Some(m) = &mat {
                for (k, slot) in [crate::look::MapSlot::Normal, crate::look::MapSlot::Roughness, crate::look::MapSlot::Metalness].into_iter().enumerate() {
                    if let Some(t) = m.map(slot).clone() {
                        maps[k] = self.texture_layer(&t).map(|l| l as f32 + 1.0).unwrap_or(0.0);
                    }
                }
                // w packs occlusion (low) and emission (high) layers + 1.
                let lay = |app: &mut App, s: crate::look::MapSlot| m.map(s).clone().and_then(|t| app.texture_layer(&t)).map(|l| l as u32 + 1).unwrap_or(0);
                let ao = lay(self, crate::look::MapSlot::Occlusion);
                let em = lay(self, crate::look::MapSlot::Emission);
                maps[3] = (ao + em * 1024) as f32;
            }
            let (color, mparams, emi, alpha) = match &mat {
                Some(m) => (m.color, [m.specular, m.roughness, m.metalness, layer], [m.emission[0], m.emission[1], m.emission[2], 0.0], 1.0 - m.transparency),
                None => (color, GpuVertex::DEFAULT_MAT, [0.0; 4], 1.0),
            };
            if self.tri_cache(id).is_none() {
                continue;
            }
            let mut chunk = NodeGeom { key, transp: alpha < 0.999, ..Default::default() };
            let mut tri_v: Vec<GpuVertex> = Vec::new();
            let mut wire_v: Vec<GpuVertex> = Vec::new();
            let bounds: Option<(Vec3, Vec3)>;
            {
                let mesh = self.scene.node(id).unwrap().mesh().unwrap();
                let cache = self.cache.get_mut(&id).unwrap();
                // Deformed corners from the cached triangulation (topology is unchanged).
                let deformed: Option<(Vec<Vec3>, Vec<Vec3>)> = def_pos.as_ref().map(|p| {
                    let vf = cache.vert_faces.get_or_insert_with(|| mesh.vertex_faces());
                    mesh.deformed_corners(&cache.tri, vf, p)
                });
                let tri = &cache.tri;
                let has_face_sel = !face_sel.is_empty();
                if !template {
                    let corner = |i: usize| -> GpuVertex {
                        let f = tri.tri_face[i / 3];
                        let vi = tri.vert[i] as usize;
                        let c = if has_face_sel && face_sel.contains(&(id, f)) {
                            [0.95, 0.45, 0.2]
                        } else if let Some(wv) = viz.as_ref().and_then(|z| z.get(vi)) {
                            // Maya weight display: black (0) to white (1).
                            let k = 0.06 + wv * 0.94;
                            [k, k, k]
                        } else {
                            color
                        };
                        let (wp, n) = match &deformed {
                            Some((cp, cn)) => (cp[i], cn[i]),
                            None => (w.transform_point3(tri.pos[i]), nm.transform_vector3(tri.nrm[i]).normalize_or_zero()),
                        };
                        let (uv, has) = match tri.uv.get(i) {
                            Some(t) => (*t * repeat, 1.0),
                            None => (Vec2::ZERO, 0.0),
                        };
                        GpuVertex { pos: wp.to_array(), nrm: n.to_array(), col: [c[0], c[1], c[2], alpha], mat: mparams, emi: [emi[0], emi[1], emi[2], has], uv: uv.to_array(), maps }
                    };
                    tri_v = crate::mesh::par_map(tri.pos.len(), &corner);
                }
                let wpos: Vec<Vec3> = match def_pos {
                    Some(p) => p,
                    None => mesh.positions.iter().map(|p| w.transform_point3(*p)).collect(),
                };
                bounds = (!wpos.is_empty()).then(|| wpos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p))));
                wire_v.reserve(tri.edges.len() * 2);
                for &(a, b) in &tri.edges {
                    wire_v.push(GpuVertex::line(wpos[a as usize], wire_col));
                    wire_v.push(GpuVertex::line(wpos[b as usize], wire_col));
                }
            }
            if !tri_v.is_empty() {
                let t = Chunk::with_bounds(tri_v, bounds);
                if chunk.transp {
                    g.tris_transp.push(t.clone());
                    g.transp_nodes.push(id);
                } else {
                    g.tris.push(t.clone());
                    g.tri_nodes.push(id);
                }
                chunk.tris = Some(t);
            }
            if !wire_v.is_empty() {
                let wc = Chunk::with_bounds(wire_v, None);
                g.wire_all.push(wc.clone());
                if in_sel {
                    g.wire_sel.push(wc.clone());
                }
                chunk.wire = Some(wc);
            }
            self.node_geom.insert(id, chunk);
        }
        let (ghost_wire, trail_lines) = self.anim_overlays();
        extra_wire.extend(ghost_wire);
        g.overlay.extend(trail_lines);
        if !extra_wire.is_empty() {
            let c = Chunk::with_bounds(extra_wire, None);
            g.wire_all.push(c.clone());
            g.wire_sel.push(c);
        }
        self.collect_lights(&mut g);
        g.textures = Some(self.texture_set());
        let g = Arc::new(g);
        self.geom_last = Some((global, g.clone()));
        g
    }
}

/// Cached GPU vertices of one mesh node.
#[derive(Default)]
pub struct NodeGeom {
    key: u64,
    tris: Option<Arc<Chunk>>,
    transp: bool,
    wire: Option<Arc<Chunk>>,
}

fn hash_mat4(h: &mut impl std::hash::Hasher, m: &Mat4) {
    for x in m.to_cols_array() {
        h.write_u32(x.to_bits());
    }
}

/// Grid lines for a view (Maya: 12 units, 1 unit spacing, dark axis lines).
pub fn grid_lines(kind: CamKind) -> Vec<GpuVertex> {
    let mut v = Vec::new();
    let size = 12i32;
    let (u, w): (Vec3, Vec3) = match kind {
        CamKind::Persp | CamKind::Top => (Vec3::X, Vec3::Z),
        CamKind::Front => (Vec3::X, Vec3::Y),
        CamKind::Side => (Vec3::Z, Vec3::Y),
    };
    let line = [0.42, 0.42, 0.42, 1.0];
    let axis = [0.12, 0.12, 0.12, 1.0];
    for i in -size..=size {
        let c = if i == 0 { axis } else { line };
        let f = i as f32;
        for (a, b) in [(u * f - w * size as f32, u * f + w * size as f32), (w * f - u * size as f32, w * f + u * size as f32)] {
            v.push(GpuVertex::line(a, [c[0], c[1], c[2]]));
            v.push(GpuVertex::line(b, [c[0], c[1], c[2]]));
        }
    }
    v
}

#[cfg(test)]
mod perf_tests {
    use super::*;
    use crate::commands::execute;

    /// cargo test --release perf_ -- --ignored --nocapture
    #[test]
    #[ignore]
    fn perf_build_geometry() {
        let mut app = App::new();
        app.scene.nodes.clear();
        for m in [
            "select -cl; joint -p 0 -3 0 -n a; joint -p 0 0 0 -n b; joint -p 0 3 0 -n c",
            "polyCylinder -r 1 -h 6 -sa 128 -sh 160 -n tube",
            "skinCluster a tube",
            "polyCube -n box; duplicate -n box2; blendShape box2 box; setAttr box.box2 0.5",
            "polySphere -sa 100 -sh 100 -n ball; move 4 0 0",
        ] {
            execute(&mut app, m).unwrap();
        }
        let tube = app.scene.find_by_name("tube").unwrap();
        eprintln!("tube verts {}", app.scene.node(tube).unwrap().mesh().unwrap().positions.len());
        let b = app.scene.find_by_name("b").unwrap();
        let t0 = std::time::Instant::now();
        let n = 20;
        for i in 0..n {
            app.scene.node_mut(b).unwrap().rotate.z = i as f32;
            let _ = app.build_geometry();
        }
        eprintln!("animated (deforming): {:.2} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let _ = app.build_geometry();
        }
        eprintln!("static: {:.2} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let _ = crate::rig::deformed_positions(&app.scene, tube);
        }
        eprintln!("deformed_positions(tube): {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        let mesh = app.scene.node(tube).unwrap().mesh().unwrap().clone();
        let tri = mesh.triangulate();
        let vf = mesh.vertex_faces();
        let p = crate::rig::deformed_positions(&app.scene, tube).unwrap();
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let _ = mesh.deformed_corners(&tri, &vf, &p);
        }
        eprintln!("deformed_corners(tube): {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        let t0 = std::time::Instant::now();
        let lead = app.lead();
        for _ in 0..n {
            for nd in &app.scene.nodes {
                let _ = app.node_key(nd.id, lead, &[]);
            }
        }
        eprintln!("keys: {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }
}
