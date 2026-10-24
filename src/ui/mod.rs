//! Maya-style UI shell: layout, theme, hotkeys and file dialogs.

pub mod anim_editors;
pub mod copilot;
pub mod lookdev;
pub mod menus;
pub mod panels;
pub mod render_view;
pub mod rigging;
pub mod timeline;
pub mod uv_editor;
pub mod viewport;
pub mod windows;

use crate::app::{AiUiRequest, App, CompMode, RightTab, Shading, Tool};
use crate::scene::Channel;
use eframe::egui::{self, Color32, Key, Modifiers};

pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    ImportObj,
    ExportObj(bool),
    Export(bool),
    Quit,
    OpenPath(std::path::PathBuf),
    Recover(std::path::PathBuf),
}

/// Something that must wait for the "save changes?" answer.
#[derive(Clone)]
enum Pending {
    Close,
    New,
    Open,
    OpenPath(std::path::PathBuf),
    Recover(std::path::PathBuf),
}

/// Distance from p to the segment a–b (screen space).
pub fn uv_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

pub fn report(app: &mut App, r: Result<String, String>) {
    match r {
        Ok(s) => {
            if !s.is_empty() {
                for l in s.lines() {
                    app.log.push(format!("// Result: {l}"));
                }
                app.cmd_result = s;
            }
        }
        Err(e) => {
            app.log.push(format!("// Error: {e}"));
            app.cmd_result = format!("Error: {e}");
        }
    }
}

pub struct Shell {
    pub app: App,
    outliner_drag: panels::OutlinerDrag,
    graph: anim_editors::GraphState,
    dope: anim_editors::DopeState,
    poses: anim_editors::PoseState,
    retarget: anim_editors::RetargetState,
    render_ui: render_view::RenderViewUi,
    copilot: copilot::Copilot,
    last_title: String,
    last_time: Option<f64>,
    prefs: crate::prefs::Prefs,
    prefs_snapshot: String,
    last_autosave: std::time::Instant,
    pending: Option<Pending>,
    allow_close: bool,
    recovery: Vec<(std::path::PathBuf, std::time::SystemTime)>,
}

impl Shell {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            crate::render::Renderer::install(rs);
        }
        apply_theme(&cc.egui_ctx);
        let mut prefs = crate::prefs::Prefs::load();
        let crashed = !prefs.clean_exit;
        let mut app = App::new();
        prefs.apply(&mut app);
        if prefs.load_demo_scene {
            demo_scene(&mut app);
        }
        let mut copilot = copilot::Copilot::new(&cc.egui_ctx);
        copilot.sync_shelf_from(&mut app);
        copilot.open = prefs.copilot_open;
        // Mark the session as running; a clean exit sets this back to true.
        prefs.clean_exit = false;
        prefs.save();
        let recovery = if crashed { crate::prefs::list_autosaves().into_iter().take(3).collect() } else { Vec::new() };
        Shell {
            app,
            outliner_drag: Default::default(),
            graph: Default::default(),
            dope: Default::default(),
            poses: Default::default(),
            retarget: Default::default(),
            render_ui: Default::default(),
            copilot,
            last_title: String::new(),
            last_time: None,
            prefs_snapshot: String::new(),
            prefs,
            last_autosave: std::time::Instant::now(),
            pending: None,
            allow_close: false,
            recovery,
        }
    }
}

fn demo_scene(app: &mut App) {
    let _ = crate::commands::execute(app, "polyCube -w 2 -h 2 -d 2; move 0 1 0; polySphere -r 1; move 3.5 1 0; polyTorus; move -3.5 0.5 0");
    if let Some(id) = app.scene.find_by_name("pSphere1") {
        app.scene.node_mut(id).unwrap().color = [0.8, 0.35, 0.3];
    }
    if let Some(id) = app.scene.find_by_name("pTorus1") {
        app.scene.node_mut(id).unwrap().color = [0.35, 0.55, 0.85];
    }
    app.sel.clear();
    app.log.push("// Demo scene loaded: pCube1, pSphere1, pTorus1".into());
    // Fresh history: the demo scene is not undoable.
    app.clear_history_for_new_session();
    app.dirty = false;
}

pub fn apply_theme(ctx: &egui::Context) {
    // Use the monospace font as a glyph fallback for UI symbols (arrows, boxes, shapes).
    let mut fonts = egui::FontDefinitions::default();
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        list.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
    let mut v = egui::Visuals::dark();
    let panel = Color32::from_rgb(68, 68, 68);
    v.panel_fill = panel;
    v.window_fill = Color32::from_rgb(68, 68, 68);
    v.extreme_bg_color = Color32::from_rgb(43, 43, 43);
    v.faint_bg_color = Color32::from_rgb(73, 73, 73);
    v.selection.bg_fill = panels::HILITE;
    v.selection.stroke = egui::Stroke::new(1.0, Color32::WHITE);
    v.override_text_color = Some(Color32::from_rgb(204, 204, 204));
    let r = egui::CornerRadius::same(2);
    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(55, 55, 55));
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(93, 93, 93);
    v.widgets.inactive.bg_fill = Color32::from_rgb(43, 43, 43);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(110, 110, 110);
    v.widgets.active.weak_bg_fill = panels::HILITE;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
    }
    v.window_corner_radius = egui::CornerRadius::same(3);
    v.menu_corner_radius = egui::CornerRadius::same(2);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 3.0);
        s.spacing.button_padding = egui::vec2(5.0, 2.0);
        s.spacing.interact_size.y = 18.0;
        for f in s.text_styles.values_mut() {
            f.size = (f.size * 0.95).max(9.0);
        }
    });
}

impl Shell {
    fn hotkeys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let typing = ctx.egui_wants_keyboard_input();
        let app = &mut self.app;
        let pressed: Vec<(Key, Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        // Global AI shortcuts work even while a text field has focus.
        for (k, mods) in &pressed {
            let cmd = mods.command || mods.ctrl;
            match (k, cmd) {
                (Key::F3, _) | (Key::K, true) => app.ai_requests.push(AiUiRequest::FindMenu),
                (Key::L, true) => app.ai_requests.push(AiUiRequest::FocusPrompt),
                _ => {}
            }
        }
        if typing {
            return;
        }
        for (k, mods) in pressed {
            let cmd = mods.command || mods.ctrl;
            match (k, cmd, mods.shift, mods.alt) {
                (Key::N, true, _, _) => actions.push(Action::New),
                (Key::O, true, _, _) => actions.push(Action::Open),
                (Key::S, true, true, _) => actions.push(Action::SaveAs),
                (Key::S, true, false, _) => actions.push(Action::Save),
                (Key::Q, true, _, _) => actions.push(Action::Quit),
                (Key::Z, true, false, _) | (Key::Z, false, false, false) => app.undo(),
                (Key::Y, true, _, _) | (Key::Z, _, true, false) => app.redo(),
                (Key::D, true, _, _) => app.duplicate_selected(),
                (Key::G, true, _, _) => app.group_selected(),
                (Key::E, true, _, _) => app.extrude(),
                (Key::A, true, true, _) => app.select_all(),
                (Key::I, true, true, _) => app.invert_selection(),
                (Key::A, true, false, _) => {
                    if app.panels.right && app.right_tab == RightTab::AttributeEditor {
                        app.right_tab = RightTab::ChannelBox;
                    } else {
                        app.panels.right = true;
                        app.right_tab = RightTab::AttributeEditor;
                    }
                }
                (Key::H, true, true, _) => app.show_all(),
                (Key::H, true, false, _) => app.toggle_visibility_selected(false),
                (Key::H, false, true, _) => app.toggle_visibility_selected(true),
                (Key::V, false, true, true) => {
                    let s = app.scene.play_start;
                    app.set_time(s);
                }
                (Key::V, false, _, true) => app.playing = !app.playing,
                (Key::Comma, false, _, true) => {
                    let t = (app.scene.time - 1.0).max(app.scene.play_start);
                    app.set_time(t);
                }
                (Key::Period, false, _, true) => {
                    let t = (app.scene.time + 1.0).min(app.scene.play_end);
                    app.set_time(t);
                }
                (Key::Comma, false, _, false) => app.step_key(false),
                (Key::Period, false, _, false) => app.step_key(true),
                (Key::W, false, true, false) => app.key_selected(&Channel::translate()),
                (Key::E, false, true, false) => app.key_selected(&Channel::rotate()),
                (Key::R, false, true, false) => app.key_selected(&Channel::scale()),
                (Key::P, false, true, false) => app.unparent_selected(),
                (Key::P, false, false, false) => app.parent_selected(),
                (Key::Q, false, false, false) => app.tool = Tool::Select,
                (Key::W, false, false, false) => app.tool = Tool::Move,
                (Key::E, false, false, false) => app.tool = Tool::Rotate,
                (Key::R, false, false, false) => app.tool = Tool::Scale,
                (Key::S, false, false, false) => app.key_selected(&Channel::ALL),
                (Key::F, false, false, false) => {
                    let v = app.active_view;
                    app.frame(v, true);
                }
                (Key::A, false, false, false) => {
                    if app.quad {
                        for v in 0..4 {
                            app.frame(v, false);
                        }
                    } else {
                        let v = app.active_view;
                        app.frame(v, false);
                    }
                }
                (Key::Num7, false, false, false) => {
                    let v = &mut app.views[app.active_view];
                    v.all_lights ^= true;
                }
                (Key::X, false, false, true) => {
                    let v = &mut app.views[app.active_view];
                    v.xray ^= true;
                }
                (Key::Num4, false, false, false) => app.views[app.active_view].shading = Shading::Wireframe,
                (Key::Num5, false, false, false) => app.views[app.active_view].shading = Shading::Shaded,
                (Key::Num6, false, false, false) => {
                    // Maya: 6 = textured (shaded + hardware texturing).
                    let v = &mut app.views[app.active_view];
                    v.textured = !(v.textured && v.shading != Shading::Wireframe);
                    if v.shading == Shading::Wireframe {
                        v.shading = Shading::Shaded;
                    }
                }
                (Key::Space, false, false, false) => app.quad = !app.quad,
                (Key::F8, _, _, _) => {
                    let m = if app.comp_mode == CompMode::Object { CompMode::Vertex } else { CompMode::Object };
                    app.set_comp_mode(m);
                }
                (Key::F9, _, _, _) => app.set_comp_mode(CompMode::Vertex),
                (Key::F11, _, _, _) => app.set_comp_mode(CompMode::Face),
                (Key::F10, _, _, _) => app.set_comp_mode(CompMode::Edge),
                (Key::R, true, _, _) => app.tool = Tool::EdgeLoop,
                (Key::B, true, _, _) => {
                    let w = app.bevel_width;
                    let r = app.bevel_selected(w);
                    report(app, r);
                }
                (Key::Delete, _, _, _) | (Key::Backspace, _, _, _) => app.delete_selected(),
                (Key::Escape, _, _, _) => {
                    app.interact.drag = None;
                    if app.tool == Tool::Joint {
                        app.finish_joint_chain();
                    }
                    if !app.cut_points.is_empty() {
                        app.cut_points.clear();
                        app.info("Multi-Cut cancelled");
                    }
                }
                (Key::Enter, false, _, _) if app.tool == Tool::Joint => app.finish_joint_chain(),
                (Key::Enter, false, _, _) if app.tool == Tool::MultiCut => {
                    let r = app.finish_multi_cut().map(|n| format!("Multi-Cut: {n} face(s) split"));
                    report(app, r);
                }
                (Key::B, false, false, false) => {
                    app.soft_select = !app.soft_select;
                    app.info(if app.soft_select { "Soft Select on (B)" } else { "Soft Select off (B)" });
                }
                (Key::Backtick, false, _, _) => app.ai_requests.push(AiUiRequest::ToggleVoice),
                _ => {}
            }
        }
    }

    fn run_actions(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        for a in actions {
            // Guard actions that would throw away unsaved changes.
            let guarded = match &a {
                Action::New => Some(Pending::New),
                Action::Open => Some(Pending::Open),
                Action::OpenPath(p) => Some(Pending::OpenPath(p.clone())),
                Action::Recover(p) => Some(Pending::Recover(p.clone())),
                _ => None,
            };
            if let Some(p) = guarded {
                if self.app.dirty {
                    self.pending = Some(p);
                    continue;
                }
            }
            self.run_action(ctx, a);
        }
    }

    fn run_pending(&mut self, ctx: &egui::Context, p: Pending) {
        match p {
            Pending::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::New => self.run_action(ctx, Action::New),
            Pending::Open => self.run_action(ctx, Action::Open),
            Pending::OpenPath(p) => self.run_action(ctx, Action::OpenPath(p)),
            Pending::Recover(p) => self.run_action(ctx, Action::Recover(p)),
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, a: Action) {
        let app = &mut self.app;
        {
            match a {
                Action::OpenPath(p) => app.open_from(p),
                Action::Recover(p) => app.recover_from(p),
                Action::New => app.new_scene(),
                Action::Open => {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Rust Maya Scene", &["rma", "json"]).pick_file() {
                        app.open_from(p);
                    }
                }
                Action::Save => match app.file_path.clone() {
                    Some(p) => app.save_to(p),
                    None => {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Rust Maya Scene", &["rma"]).set_file_name("untitled.rma").save_file() {
                            app.save_to(p);
                        }
                    }
                },
                Action::SaveAs => {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Rust Maya Scene", &["rma"]).set_file_name("untitled.rma").save_file() {
                        app.save_to(p);
                    }
                }
                Action::ImportObj => {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("All supported", &["glb", "gltf", "obj"])
                        .add_filter("glTF 2.0", &["glb", "gltf"])
                        .add_filter("Wavefront OBJ", &["obj"])
                        .pick_file()
                    {
                        let r = crate::importer::import_path(app, &p);
                        report(app, r);
                    }
                }
                Action::ExportObj(sel) => {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Wavefront OBJ", &["obj"]).set_file_name("export.obj").save_file() {
                        app.export_obj(p, sel);
                    }
                }
                Action::Export(sel) => {
                    let stem = app.file_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or("scene".into());
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("glTF binary", &["glb"])
                        .add_filter("glTF (embedded)", &["gltf"])
                        .add_filter("USD ASCII", &["usda"])
                        .set_file_name(format!("{stem}.glb"))
                        .save_file()
                    {
                        let opts = crate::export::ExportOptions::from_app(app, sel);
                        let r = crate::export::export_path(app, &p, &opts);
                        report(app, r);
                    }
                }
                Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }
}

impl Shell {
    /// Autosave, preferences persistence, crash recovery and the unsaved-changes prompt.
    fn session_ui(&mut self, ctx: &egui::Context) {
        // Drag and drop: .glb / .gltf / .obj import; .rma opens (asking to save first).
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "rma" {
                self.pending = Some(Pending::OpenPath(p));
                continue;
            }
            let r = crate::importer::import_path(&mut self.app, &p);
            report(&mut self.app, r);
        }
        // Unsaved changes when the window is closed.
        if ctx.input(|i| i.viewport().close_requested()) && self.app.dirty && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        if let Some(p) = self.pending.clone() {
            let file = self.app.file_path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or("untitled".into());
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("save_changes")).show(ctx, |ui| {
                ui.heading("Save Changes");
                ui.label(format!("Save changes to {file}?"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Don't Save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) => {
                    self.pending = None;
                    self.run_action(ctx, Action::Save);
                    if !self.app.dirty {
                        self.run_pending(ctx, p);
                    }
                }
                Some(1) => {
                    self.pending = None;
                    self.app.dirty = false;
                    self.run_pending(ctx, p);
                }
                Some(2) => self.pending = None,
                _ => {}
            }
        }
        // Crash recovery offer (shown once at startup).
        if !self.recovery.is_empty() {
            let mut close = false;
            let mut pick = None;
            egui::Modal::new(egui::Id::unique("recover")).show(ctx, |ui| {
                ui.heading("Recover Unsaved Work?");
                ui.label("Rust Maya didn't shut down cleanly last time. These autosaves are available:");
                for (p, t) in &self.recovery {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if ui.button(format!("Recover {name}  ({})", crate::prefs::age_text(*t))).clicked() {
                        pick = Some(p.clone());
                    }
                }
                ui.add_space(4.0);
                if ui.button("Start fresh").clicked() {
                    close = true;
                }
            });
            if let Some(p) = pick {
                self.app.recover_from(p);
                close = true;
            }
            if close {
                self.recovery.clear();
            }
        }
        // Autosave (the UI only redraws on input, so schedule a wake-up for the timer).
        let period = self.prefs.autosave_minutes.max(0.5) * 60.0;
        if self.prefs.autosave_enabled && self.app.dirty {
            let left = (period - self.last_autosave.elapsed().as_secs_f32()).max(0.05);
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(left));
        }
        if self.prefs.autosave_enabled && self.app.dirty && self.last_autosave.elapsed().as_secs_f32() > period {
            self.last_autosave = std::time::Instant::now();
            match crate::prefs::autosave(&self.app) {
                Ok(p) => self.app.info(format!("Autosaved {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
                Err(e) => self.app.warn(format!("Autosave failed: {e}")),
            }
        }
        // Preferences window.
        if self.app.panels.prefs {
            let mut open = true;
            egui::Window::new("Preferences").open(&mut open).collapsible(false).default_width(340.0).show(ctx, |ui| {
                ui.label(egui::RichText::new("Files").strong());
                ui.checkbox(&mut self.prefs.autosave_enabled, "Autosave");
                ui.horizontal(|ui| {
                    ui.label("Every (minutes)");
                    ui.add(egui::DragValue::new(&mut self.prefs.autosave_minutes).range(0.5..=120.0).speed(0.1));
                });
                ui.label(egui::RichText::new(format!("Autosaves: {}", crate::prefs::autosave_dir().display())).small().weak());
                ui.checkbox(&mut self.prefs.load_demo_scene, "Open the demo scene at startup");
                ui.separator();
                ui.label(egui::RichText::new("The window layout, panels, shading modes, tool options and recent files are remembered automatically.").small().weak());
                if ui.button("Reset layout to defaults").clicked() {
                    let keep = (self.prefs.recent.clone(), self.prefs.autosave_enabled, self.prefs.autosave_minutes);
                    self.prefs = crate::prefs::Prefs { recent: keep.0, autosave_enabled: keep.1, autosave_minutes: keep.2, clean_exit: false, ..Default::default() };
                    self.prefs.apply(&mut self.app);
                    self.copilot.open = self.prefs.copilot_open;
                }
            });
            self.app.panels.prefs = open;
        }
        // Persist preferences when they change (checked about once a second).
        if self.app.frame_no % 60 == 0 {
            self.prefs.capture(&self.app, self.copilot.open);
            if let Ok(s) = serde_json::to_string(&self.prefs) {
                if s != self.prefs_snapshot {
                    self.prefs.save();
                    self.prefs_snapshot = s;
                }
            }
        }
    }
}

impl eframe::App for Shell {
    fn on_exit(&mut self) {
        self.prefs.capture(&self.app, self.copilot.open);
        self.prefs.clean_exit = true;
        self.prefs.save();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.app.frame_no += 1;
        let now = ctx.input(|i| i.time);
        let dt = self.last_time.map(|t| (now - t) as f32).unwrap_or(0.0).min(0.25);
        self.last_time = Some(now);
        self.app.advance_playback(dt);
        if self.app.playing {
            ctx.request_repaint();
        }
        // Drop stale selection (after undo/delete).
        let scene = &self.app.scene;
        self.app.sel.retain(|id| scene.node(*id).is_some());
        self.app.comp_sel.retain(|(id, _)| scene.node(*id).is_some());

        let mut actions = Vec::new();
        self.hotkeys(&ctx, &mut actions);

        self.copilot.update(&mut self.app, &ctx);
        // Leaving the joint tool finishes (and orients) the chain being built.
        if self.app.tool != Tool::Joint && self.app.joint_chain.is_some() {
            let t = self.app.tool;
            self.app.finish_joint_chain();
            self.app.tool = t;
        }
        // Rig evaluation: driven keys -> constraints -> IK (skinning is applied at draw time).
        crate::rig::evaluate(&mut self.app.scene);
        self.app.sync_look_through();

        let app = &mut self.app;
        let bar_frame = egui::Frame::new().fill(Color32::from_rgb(68, 68, 68)).inner_margin(egui::Margin::symmetric(4, 2));
        egui::Panel::top("menubar").frame(bar_frame).show(ui, |ui| menus::menu_bar(app, ui, &mut actions));
        if app.panels.status {
            egui::Panel::top("status").frame(bar_frame).show(ui, |ui| menus::status_line(app, ui, &mut actions));
        }
        if app.panels.shelf {
            egui::Panel::top("shelf").frame(bar_frame).show(ui, |ui| menus::shelf(app, ui));
        }
        if app.panels.helpline {
            egui::Panel::bottom("helpline").frame(bar_frame).show(ui, |ui| timeline::help_line(app, ui));
        }
        if app.panels.cmdline {
            egui::Panel::bottom("cmdline").frame(bar_frame).show(ui, |ui| timeline::command_line(app, ui));
        }
        if app.panels.range {
            egui::Panel::bottom("range").frame(bar_frame).show(ui, |ui| timeline::range_slider(app, ui));
        }
        if app.panels.timeline {
            egui::Panel::bottom("timeslider").frame(bar_frame).show(ui, |ui| timeline::time_slider(app, ui));
        }
        if app.panels.toolbox {
            egui::Panel::left("toolbox").frame(bar_frame).exact_size(38.0).resizable(false).show(ui, |ui| panels::toolbox(app, ui));
        }
        if app.panels.outliner {
            egui::Panel::left("outliner").frame(bar_frame).default_size(220.0).size_range(140.0..=500.0).resizable(true).show(ui, |ui| {
                ui.label(egui::RichText::new("Outliner").strong());
                panels::outliner(app, ui, &mut self.outliner_drag);
            });
        }
        if self.copilot.open {
            egui::Panel::right("copilot").frame(bar_frame).default_size(330.0).size_range(260.0..=640.0).resizable(true).show(ui, |ui| self.copilot.panel(app, ui));
        }
        if app.panels.right {
            egui::Panel::right("rightpanel").frame(bar_frame).default_size(290.0).size_range(220.0..=600.0).resizable(true).show(ui, |ui| panels::right_panel(app, ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(40, 40, 40))).show(ui, |ui| {
            viewport::show_views(app, ui);
        });

        windows::script_editor(app, &ctx);
        anim_editors::graph_editor(app, &ctx, &mut self.graph);
        anim_editors::dope_sheet(app, &ctx, &mut self.dope);
        anim_editors::pose_library(app, &ctx, &mut self.poses);
        anim_editors::retarget_window(app, &ctx, &mut self.retarget);
        windows::hotkeys(app, &ctx);
        windows::about(app, &ctx);
        rigging::sdk_window(app, &ctx);
        lookdev::hypershade_window(app, &ctx);
        uv_editor::uv_editor_window(app, &ctx);
        render_view::render_view_window(app, &ctx, &mut self.render_ui);
        if app.render_tick() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        self.run_actions(&ctx, actions);
        self.session_ui(&ctx);

        let app = &self.app;
        let file = app.file_path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or("untitled".into());
        let title = format!("Rust Maya 2026 - {}{}", file, if app.dirty { "*" } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}
