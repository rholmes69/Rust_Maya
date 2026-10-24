//! AI Copilot dock, AI settings, voice control, Find Menu, Scene Health Check, palette dialog.

use crate::ai::context;
use crate::ai::parse::parse_simple;
use crate::ai::prompts::Task;
use crate::ai::provider::{ModelInfo, ModelRef, Provider};
use crate::ai::{self, AiClient, AiEvent, AiRequest, AiSettings, ChatMsg};
use crate::app::{AiUiRequest, App};
use eframe::egui::{self, Color32, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    System,
}

struct Entry {
    role: Role,
    text: String,
    meta: String,
    mel: Vec<String>,
    errors: Vec<String>,
    /// MEL offered as a button rather than run automatically (help answers, health fixes).
    suggested: Vec<String>,
    /// Preview image (render check).
    image: Option<egui::TextureHandle>,
}

impl Entry {
    fn new(role: Role, text: impl Into<String>) -> Self {
        Entry { role, text: text.into(), meta: String::new(), mel: vec![], errors: vec![], suggested: vec![], image: None }
    }
}

#[derive(Default)]
struct VoiceUi {
    status: String,
    ready: Option<String>,
    download: Option<(String, f32)>,
    transcribing: bool,
    error: Option<String>,
}

pub struct Copilot {
    pub settings: AiSettings,
    client: AiClient,
    #[allow(dead_code)]
    repaint: egui::Context,
    voice: Option<crate::voice::VoiceEngine>,
    voice_ui: VoiceUi,
    speaker: crate::tts::Speaker,
    entries: Vec<Entry>,
    history: Vec<ChatMsg>,
    input: String,
    focus_input: bool,
    hist_cursor: Option<usize>,
    busy: bool,
    busy_task: Option<Task>,
    current: Option<usize>,
    route_note: String,
    ollama: Result<Vec<ModelInfo>, String>,
    openrouter: Result<Vec<ModelInfo>, String>,
    models_loaded: bool,
    or_filter: String,
    or_free_only: bool,
    or_tools_only: bool,
    pull_name: String,
    pull: Option<(String, f32)>,
    vram: Option<(String, f32)>,
    pub open: bool,
    show_settings: bool,
    find_open: bool,
    find_query: String,
    find_focus: bool,
    health_open: bool,
    health_report: String,
    health_ai: Option<(String, Vec<String>)>,
    palette_open: bool,
    palette_prompt: String,
    settings_dirty: bool,
}

/// Built-in actions searchable from Find Menu (label, MEL).
const FIND_ITEMS: &[(&str, &str)] = &[
    ("Create Polygon Cube", "polyCube"),
    ("Create Polygon Sphere", "polySphere"),
    ("Create Polygon Cylinder", "polyCylinder"),
    ("Create Polygon Cone", "polyCone"),
    ("Create Polygon Plane", "polyPlane"),
    ("Create Polygon Torus", "polyTorus"),
    ("Create Locator", "spaceLocator"),
    ("Duplicate", "duplicate"),
    ("Delete", "delete"),
    ("Group", "group"),
    ("Parent", "parent"),
    ("Unparent", "parent -w"),
    ("Select All", "select -all"),
    ("Deselect All", "select -cl"),
    ("Undo", "undo"),
    ("Redo", "redo"),
    ("Mesh > Smooth", "polySmooth"),
    ("Mesh > Triangulate", "polyTriangulate"),
    ("Mesh > Combine", "polyUnite"),
    ("Mesh > Separate", "polySeparate"),
    ("Edit Mesh > Extrude", "polyExtrudeFacet"),
    ("Modify > Freeze Transformations", "makeIdentity"),
    ("Edit > Delete by Type > History", "delete -ch"),
    ("Select Top Faces", "selectFaces 0 1 0"),
    ("Vertex Mode", "selectMode -v"),
    ("Face Mode", "selectMode -f"),
    ("Object Mode", "selectMode -o"),
    ("Move Tool", "setToolTo move"),
    ("Rotate Tool", "setToolTo rotate"),
    ("Scale Tool", "setToolTo scale"),
    ("Set Key", "setKeyframe"),
    ("Delete Keys (cutKey)", "cutKey"),
    ("Play", "play"),
    ("Stop", "stop"),
    ("Perspective View", "viewSet -persp"),
    ("Top View", "viewSet -top"),
    ("Front View", "viewSet -front"),
    ("Side View", "viewSet -side"),
    ("Frame All", "viewFit -all"),
    ("Frame Selection", "viewFit"),
    ("Four View Layout", "panelLayout -quad"),
    ("Single View Layout", "panelLayout -single"),
    ("Wireframe", "displayMode -wire"),
    ("Smooth Shade All", "displayMode -shaded"),
    ("Wireframe on Shaded", "displayMode -wos"),
    ("Hide Selection", "hide"),
    ("Show All Hidden", "showHidden -all"),
    ("New Scene", "file -new"),
];

fn fuzzy(q: &str, s: &str) -> Option<i32> {
    let q = q.to_lowercase();
    let s = s.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if let Some(i) = s.find(&q) {
        return Some(1000 - i as i32);
    }
    let mut it = s.chars();
    let mut score = 0;
    for c in q.chars().filter(|c| !c.is_whitespace()) {
        loop {
            match it.next() {
                Some(x) if x == c => {
                    score += 1;
                    break;
                }
                Some(_) => score -= 1,
                None => return None,
            }
        }
    }
    Some(score)
}

impl Copilot {
    pub fn new(ctx: &egui::Context) -> Self {
        let settings = AiSettings::load();
        let c = ctx.clone();
        let client = AiClient::new(&settings, move || c.request_repaint());
        let mut s = Copilot {
            settings,
            client,
            repaint: ctx.clone(),
            voice: None,
            voice_ui: VoiceUi::default(),
            speaker: Default::default(),
            entries: vec![],
            history: vec![],
            input: String::new(),
            focus_input: false,
            hist_cursor: None,
            busy: false,
            busy_task: None,
            current: None,
            route_note: String::new(),
            ollama: Err("not checked yet".into()),
            openrouter: Err("not checked yet".into()),
            models_loaded: false,
            or_filter: String::new(),
            or_free_only: false,
            or_tools_only: true,
            pull_name: "qwen3:8b".into(),
            pull: None,
            vram: None,
            open: true,
            show_settings: false,
            find_open: false,
            find_query: String::new(),
            find_focus: false,
            health_open: false,
            health_report: String::new(),
            health_ai: None,
            palette_open: false,
            palette_prompt: "warm sunset".into(),
            settings_dirty: false,
        };
        s.entries.push(Entry::new(
            Role::System,
            "Hi! Type or say what you want to build — \"make a table with four legs\", \"animate the sphere bouncing\", \"top view\". \
             Simple commands run instantly; tev1 routes the rest to your builder model (Ollama or OpenRouter).",
        ));
        s.client.send(AiRequest::ListModels);
        s
    }

    pub fn sync_shelf_from(&mut self, app: &mut App) {
        if app.ai_shelf.is_empty() && !self.settings.prompt_shelf.is_empty() {
            app.ai_shelf = self.settings.prompt_shelf.clone();
        }
    }

    // ------------------------------------------------------------------- model roles

    fn installed_ollama(&self) -> Vec<String> {
        self.ollama.as_ref().map(|v| v.iter().map(|m| m.id.clone()).collect()).unwrap_or_default()
    }

    pub fn builder(&self) -> Option<ModelRef> {
        if let Some(b) = &self.settings.builder {
            return Some(b.clone());
        }
        if let Some(m) = ai::pick_builder(&self.installed_ollama()) {
            return Some(ModelRef::new(Provider::Ollama, m));
        }
        self.settings.has_openrouter_key().then(|| ModelRef::new(Provider::OpenRouter, ai::SUGGESTED_OPENROUTER[0]))
    }

    pub fn router(&self) -> Option<ModelRef> {
        if let Some(r) = &self.settings.router {
            return Some(r.clone());
        }
        if self.settings.auto_router {
            return crate::ai::router::pick_decision_model(&self.installed_ollama()).map(|m| ModelRef::new(Provider::Ollama, m));
        }
        None
    }

    fn helper(&self) -> Option<ModelRef> {
        self.settings.helper.clone().or_else(|| self.builder())
    }

    /// A model that can look at images: the configured one, an installed Ollama vision model,
    /// a vision-capable helper on OpenRouter, or a default OpenRouter vision model.
    fn vision_model(&self) -> Option<ModelRef> {
        if let Some(v) = &self.settings.vision {
            return Some(v.clone());
        }
        let installed = self.installed_ollama();
        for pref in ["qwen2.5vl", "qwen3-vl", "gemma3", "llama3.2-vision", "minicpm-v", "llava"] {
            if let Some(m) = installed.iter().find(|m| m.starts_with(pref)) {
                return Some(ModelRef::new(Provider::Ollama, m.clone()));
            }
        }
        if let Some(m) = installed.iter().find(|m| crate::ai::provider::is_vision_name(m)) {
            return Some(ModelRef::new(Provider::Ollama, m.clone()));
        }
        if let Some(h) = self.helper().filter(|h| h.provider == Provider::OpenRouter)
            && self.openrouter.as_ref().is_ok_and(|v| v.iter().any(|m| m.id == h.model && m.vision))
        {
            return Some(h);
        }
        self.settings.has_openrouter_key().then(|| ModelRef::new(Provider::OpenRouter, "openai/gpt-4o-mini"))
    }

    /// Render a preview, measure it, show the automatic findings, then ask a (vision) model
    /// for a critique with fixes.
    pub fn render_check(&mut self, app: &mut App) {
        self.open = true;
        let rc = match app.render_check(None, 384, 216, 24) {
            Ok(rc) => rc,
            Err(e) => {
                self.entries.push(Entry::new(Role::System, format!("Render check failed: {e}")));
                return;
            }
        };
        let mut e = Entry::new(Role::Assistant, rc.report());
        e.meta = "render check · automatic analysis".into();
        e.suggested = rc.fixes.clone();
        let img = egui::ColorImage::from_rgba_unmultiplied([rc.width as usize, rc.height as usize], &rc.rgba);
        e.image = Some(self.repaint.load_texture(format!("render_check_{}", self.entries.len()), img, egui::TextureOptions::LINEAR));
        self.entries.push(e);
        let model = self.vision_model().or_else(|| self.helper());
        let Some(model) = model else {
            self.entries.push(Entry::new(Role::System, "Connect Ollama (e.g. `ollama pull qwen2.5vl` or `gemma3`) or OpenRouter for an AI critique of the render."));
            return;
        };
        if self.busy_task.is_some() {
            return;
        }
        let sees = self.vision_model().as_ref() == Some(&model);
        let prompt = format!(
            "Review this preview render of my scene.\nMeasured: {}\nAutomatic findings: {}\n{}",
            rc.summary(),
            if rc.issues.is_empty() { "none".to_string() } else { rc.issues.join(" | ") },
            if sees { "The preview image is attached." } else { "(No image: judge from the statistics and the scene description.)" }
        );
        self.client.set_endpoints(self.settings.endpoints());
        self.busy_task = Some(Task::RenderCheck);
        self.client.send(AiRequest::Task { task: Task::RenderCheck, model, scene: context::scene_summary(app), prompt, image: sees.then(|| rc.png.clone()) });
    }

    // ------------------------------------------------------------------- requests

    pub fn submit(&mut self, app: &mut App, prompt: String) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if self.settings.prompt_history.last() != Some(&prompt) {
            self.settings.prompt_history.push(prompt.clone());
            if self.settings.prompt_history.len() > 200 {
                self.settings.prompt_history.remove(0);
            }
            self.settings_dirty = true;
        }
        self.hist_cursor = None;
        self.entries.push(Entry::new(Role::User, &prompt));
        self.open = true;
        // 1) Instant offline parser (no model needed).
        let parsed = parse_simple(&prompt);
        if parsed.as_deref() == Some(&["renderCheck".to_string()][..]) {
            self.render_check(app);
            return;
        }
        if let Some(mel) = parsed {
            let mut e = Entry::new(Role::Assistant, "Done.");
            e.meta = "instant · offline parser".into();
            app.begin_batch();
            for m in &mel {
                if let Err(err) = crate::commands::execute(app, m) {
                    e.errors.push(format!("{m}: {err}"));
                }
            }
            app.end_batch();
            e.mel = mel;
            self.history.push(ChatMsg { role: "user".into(), content: prompt });
            self.history.push(ChatMsg { role: "assistant".into(), content: ai::transcript_json("Done.", &e.mel) });
            self.entries.push(e);
            return;
        }
        if self.busy {
            self.entries.push(Entry::new(Role::System, "Still working on the previous request — press Stop first."));
            return;
        }
        let Some(builder) = self.builder() else {
            self.entries.push(Entry::new(
                Role::System,
                "No model available. Start Ollama and pull a model (e.g. `ollama pull qwen3:8b`, plus `tev1` as decision model), \
                 or add an OpenRouter API key in AI Settings (⚙).",
            ));
            self.show_settings = true;
            return;
        };
        self.client.set_endpoints(self.settings.endpoints());
        let scene = context::scene_summary(app);
        let router = self.router();
        self.route_note = match &router {
            Some(r) => format!("{} deciding…", r.model),
            None => format!("{} building…", builder.model),
        };
        self.client.send(AiRequest::Chat {
            router,
            builder,
            fallback: self.settings.fallback.clone(),
            history: self.history.clone(),
            scene,
            prompt: prompt.clone(),
            temperature: self.settings.temperature,
        });
        self.history.push(ChatMsg { role: "user".into(), content: prompt });
        self.entries.push(Entry::new(Role::Assistant, ""));
        self.current = Some(self.entries.len() - 1);
        self.busy = true;
        app.begin_batch();
    }

    pub fn run_task(&mut self, app: &App, task: Task, prompt: String) {
        let Some(model) = self.helper() else {
            self.entries.push(Entry::new(Role::System, format!("{}: no model available — configure Ollama or OpenRouter in AI Settings.", task.label())));
            self.open = true;
            return;
        };
        if self.busy_task.is_some() {
            self.entries.push(Entry::new(Role::System, "Another assistant task is still running."));
            return;
        }
        self.client.set_endpoints(self.settings.endpoints());
        self.busy_task = Some(task);
        self.client.send(AiRequest::Task { task, model, scene: context::scene_summary(app), prompt, image: None });
    }

    fn handle_ui_requests(&mut self, app: &mut App) {
        for r in std::mem::take(&mut app.ai_requests) {
            match r {
                AiUiRequest::Prompt(p) => self.submit(app, p),
                AiUiRequest::Task(t, p) => self.run_task(app, t, p),
                AiUiRequest::ToggleCopilot => self.open = !self.open,
                AiUiRequest::FocusPrompt => {
                    self.open = true;
                    self.focus_input = true;
                }
                AiUiRequest::ToggleVoice => self.toggle_mic(),
                AiUiRequest::FindMenu => {
                    self.find_open = true;
                    self.find_focus = true;
                    self.find_query.clear();
                }
                AiUiRequest::HealthCheck => {
                    self.health_open = true;
                    self.health_report = context::health_report(app);
                    self.health_ai = None;
                }
                AiUiRequest::PaletteDialog => self.palette_open = true,
                AiUiRequest::Settings => {
                    self.open = true;
                    self.show_settings = true;
                }
            }
        }
    }

    fn poll(&mut self, app: &mut App) {
        while let Ok(ev) = self.client.rx.try_recv() {
            match ev {
                AiEvent::Models { ollama, openrouter } => {
                    self.ollama = ollama;
                    self.openrouter = openrouter;
                    self.models_loaded = true;
                    if let Some(b) = self.builder().filter(|b| b.provider == Provider::Ollama) {
                        self.client.send(AiRequest::Vram(b.model));
                    }
                }
                AiEvent::PullProgress(s, f) => self.pull = Some((s, f)),
                AiEvent::PullDone(r) => {
                    self.pull = None;
                    match r {
                        Ok(m) => self.entries.push(Entry::new(Role::System, format!("Pulled {m}."))),
                        Err(e) => self.entries.push(Entry::new(Role::System, format!("Pull failed: {e}"))),
                    }
                    self.client.send(AiRequest::ListModels);
                }
                AiEvent::Vram(m, f) => self.vram = Some((m, f)),
                AiEvent::Routed { router, route, detail, secs } => {
                    self.route_note = match route.as_str() {
                        "delegate" => format!("{router} → builder · {secs:.1}s"),
                        r => format!("handled by {router} · {r} · {secs:.1}s"),
                    };
                    if let Some(e) = self.current.and_then(|i| self.entries.get_mut(i)) {
                        e.meta = self.route_note.clone();
                        if !detail.is_empty() {
                            e.meta.push_str(&format!("\nplan: {}", detail.chars().take(240).collect::<String>()));
                        }
                    }
                }
                AiEvent::RouterFailed(e) => {
                    if let Some(en) = self.current.and_then(|i| self.entries.get_mut(i)) {
                        en.meta = format!("decision model failed ({e}); builder handles it");
                    }
                }
                AiEvent::Fallback { from, to, error } => {
                    self.entries.push(Entry::new(Role::System, format!("{from} failed ({error}). Retrying on {to}…")));
                }
                AiEvent::Delta { reply, mel } => {
                    let errs = run_mel(app, &mel);
                    if let Some(e) = self.current.and_then(|i| self.entries.get_mut(i)) {
                        e.text = reply;
                        e.mel.extend(mel);
                        e.errors.extend(errs);
                    }
                }
                AiEvent::Reply { reply, mel, model, secs } => {
                    let errs = run_mel(app, &mel);
                    app.end_batch();
                    if let Some(e) = self.current.and_then(|i| self.entries.get_mut(i)) {
                        if !reply.is_empty() {
                            e.text = reply.clone();
                        }
                        e.mel.extend(mel);
                        e.errors.extend(errs);
                        let route = if e.meta.is_empty() { String::new() } else { format!("{} · ", e.meta.lines().next().unwrap_or("")) };
                        let plan = e.meta.lines().nth(1).map(|l| format!("\n{l}")).unwrap_or_default();
                        e.meta = format!("{route}{model} · {secs:.1}s · {} MEL{plan}", e.mel.len());
                        if e.text.is_empty() {
                            e.text = if e.mel.is_empty() { "(no reply)".into() } else { "Done.".into() };
                        }
                        self.history.push(ChatMsg { role: "assistant".into(), content: ai::transcript_json(&e.text, &e.mel) });
                        if self.settings.speak_replies {
                            self.speaker.speak(&e.text);
                        }
                    }
                    self.busy = false;
                    self.current = None;
                }
                AiEvent::TaskDone { task, reply, mel, model, secs } => {
                    self.busy_task = None;
                    self.task_done(app, task, reply, mel, format!("{} · {model} · {secs:.1}s", task.label()));
                }
                AiEvent::Error(e) => {
                    if let Some(t) = self.busy_task.filter(|t| !self.busy || e.starts_with(t.label())) {
                        let _ = t;
                        self.busy_task = None;
                        self.entries.push(Entry::new(Role::System, e));
                        continue;
                    }
                    app.end_batch();
                    if let Some(en) = self.current.and_then(|i| self.entries.get_mut(i)) {
                        en.errors.push(e.clone());
                        if en.text.is_empty() {
                            en.text = if e == "cancelled" { "Stopped.".into() } else { "Something went wrong.".into() };
                        }
                    } else {
                        self.entries.push(Entry::new(Role::System, e));
                    }
                    self.busy = false;
                    self.current = None;
                }
            }
        }
    }

    fn task_done(&mut self, app: &mut App, task: Task, reply: String, mel: Vec<String>, meta: String) {
        let mut e = Entry::new(Role::Assistant, reply.clone());
        e.meta = meta;
        match task {
            Task::GenerateMel | Task::FixMel => {
                app.script_input = mel.join(";\n");
                if !app.script_input.is_empty() {
                    app.script_input.push(';');
                }
                app.panels.script_editor = true;
                app.log.push(format!("// {}: {reply}", task.label()));
                e.text = format!("{reply}\n(Script placed in the Script Editor for review.)");
                e.suggested = mel;
            }
            Task::ExplainMel => {
                app.log.push(format!("// Explain: {reply}"));
            }
            Task::SmartRename | Task::Palette => {
                app.begin_batch();
                e.errors = run_mel(app, &mel);
                app.end_batch();
                e.mel = mel;
            }
            Task::HealthCheck => {
                self.health_ai = Some((reply.clone(), mel.clone()));
                e.suggested = mel;
            }
            Task::Help => e.suggested = mel,
            Task::RenderCheck => {
                e.text = format!("🔍 {reply}");
                e.suggested = mel;
            }
        }
        if self.settings.speak_replies {
            self.speaker.speak(&e.text);
        }
        self.entries.push(e);
        self.open = true;
    }

    /// Lighting / material / rig+animate recipe menus and the render check.
    fn recipe_buttons(&mut self, app: &mut App, ui: &mut egui::Ui) {
        let mut run: Option<(String, String)> = None;
        let mut ask: Option<String> = None;
        ui.menu_button(RichText::new("💡 Light").small(), |ui| {
            for (p, d) in crate::recipes::LIGHT_PRESETS {
                if ui.button(p).on_hover_text(d).clicked() {
                    run = Some((format!("{p} lighting"), format!("lightRig -preset {p}")));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("✨ Let the AI light it…").clicked() {
                ask = Some("Design lighting that suits this scene and its subject: start from the best lightRig preset, then fine-tune individual lights (intensity, colour, angle) with setAttr.".into());
                ui.close();
            }
        });
        ui.menu_button(RichText::new("🎨 Materials").small(), |ui| {
            ui.label(RichText::new("Applies to the selection").small().weak());
            egui::Grid::new("mat_presets").num_columns(3).show(ui, |ui| {
                for (i, p) in crate::recipes::MATERIAL_PRESETS.iter().enumerate() {
                    let c = p.rgb.map(|v| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
                    if ui.button(RichText::new(format!("■ {}", p.name)).color(Color32::from_rgb(c[0], c[1], c[2]))).clicked() {
                        run = Some((format!("make it {}", p.name), format!("materialPreset -p {}", p.name)));
                        ui.close();
                    }
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
            ui.separator();
            if ui.button("✨ Let the AI shade the scene…").clicked() {
                ask = Some("Give every object in the scene a fitting material using materialPreset (judge from names, shapes and colours; tint with -rgb where useful). Share materials between similar parts.".into());
                ui.close();
            }
        });
        ui.menu_button(RichText::new("🏃 Rig & animate").small(), |ui| {
            ui.label(RichText::new("Quick Rigs the selected mesh if needed").small().weak());
            for (a, d) in crate::recipes::ANIMS {
                if ui.button(a).on_hover_text(d).clicked() {
                    let mesh = app.sel.iter().filter_map(|id| app.scene.node(*id)).find(|n| n.mesh().is_some()).map(|n| format!(" {}", n.name)).unwrap_or_default();
                    run = Some((format!("rig and animate: {a}"), format!("rigAnimate -anim {a}{mesh}; play")));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("✨ Ask the AI…").clicked() {
                self.input = "Rig the character and make it ".into();
                self.focus_input = true;
                ui.close();
            }
        });
        if ui.small_button("🔍 Render check").on_hover_text("Render a preview and get a critique with fixes (vision model if available)").clicked() {
            self.entries.push(Entry::new(Role::User, "Render check"));
            self.render_check(app);
        }
        if let Some((label, mel)) = run {
            self.entries.push(Entry::new(Role::User, &label));
            let mut e = Entry::new(Role::Assistant, "");
            app.begin_batch();
            for m in crate::ai::parse::split_statements(&mel) {
                match crate::commands::execute(app, &m) {
                    Ok(r) if !r.is_empty() => e.text = r,
                    Ok(_) => {}
                    Err(err) => e.errors.push(format!("{m}: {err}")),
                }
                e.mel.push(m);
            }
            app.end_batch();
            if e.text.is_empty() {
                e.text = "Done.".into();
            }
            e.meta = "recipe · offline".into();
            self.entries.push(e);
        }
        if let Some(p) = ask {
            self.submit(app, p);
        }
    }

    // ------------------------------------------------------------------------ voice

    fn ensure_voice(&mut self) {
        if self.voice.is_none() {
            let c = self.repaint.clone();
            let v = crate::voice::VoiceEngine::new(move || c.request_repaint());
            v.load(&self.settings.whisper_model);
            self.voice_ui.status = format!("Loading Whisper {}…", self.settings.whisper_model);
            self.voice = Some(v);
        }
    }

    pub fn toggle_mic(&mut self) {
        self.ensure_voice();
        let v = self.voice.as_mut().unwrap();
        if v.recorder.recording {
            let pcm = v.recorder.stop();
            if pcm.len() > 1600 {
                v.transcribe(pcm);
                self.voice_ui.transcribing = true;
            }
        } else {
            self.speaker.stop();
            match v.recorder.start() {
                Ok(()) => {
                    self.voice_ui.error = None;
                    self.open = true;
                }
                Err(e) => self.voice_ui.error = Some(e),
            }
        }
    }

    fn poll_voice(&mut self, app: &mut App) {
        let Some(v) = self.voice.as_mut() else { return };
        if v.recorder.recording && self.settings.hands_free && v.recorder.update_vad(self.settings.vad_threshold, self.settings.silence_secs) {
            let pcm = v.recorder.stop();
            if v.recorder.heard_speech && pcm.len() > 1600 {
                v.transcribe(pcm);
                self.voice_ui.transcribing = true;
            }
        }
        if v.recorder.recording {
            self.repaint.request_repaint();
        }
        let mut transcripts = Vec::new();
        while let Ok(ev) = v.rx.try_recv() {
            use crate::voice::VoiceEvent;
            match ev {
                VoiceEvent::Status(s) => self.voice_ui.status = s,
                VoiceEvent::Download(f, p) => self.voice_ui.download = Some((f, p)),
                VoiceEvent::Ready(m) => {
                    self.voice_ui.ready = Some(m.clone());
                    self.voice_ui.download = None;
                    self.voice_ui.status = format!("Whisper {m} ready");
                }
                VoiceEvent::Transcript { text, secs } => {
                    self.voice_ui.transcribing = false;
                    self.voice_ui.status = format!("Heard in {secs:.1}s");
                    transcripts.push(text);
                }
                VoiceEvent::Error(e) => {
                    self.voice_ui.transcribing = false;
                    self.voice_ui.download = None;
                    self.voice_ui.error = Some(e);
                }
            }
        }
        for t in transcripts {
            let t = t.trim().trim_matches(|c: char| c == '.' || c == '"').to_string();
            // Whisper hallucinates these on silence.
            if t.is_empty() || matches!(t.to_lowercase().as_str(), "you" | "thank you" | "thanks for watching" | "[blank_audio]") {
                continue;
            }
            if self.settings.voice_auto_send {
                self.submit(app, t);
            } else {
                self.input = t;
                self.focus_input = true;
            }
        }
    }

    // ---------------------------------------------------------------------- frame

    pub fn update(&mut self, app: &mut App, ctx: &egui::Context) {
        self.handle_ui_requests(app);
        self.poll(app);
        self.poll_voice(app);
        if self.settings_dirty {
            self.settings.prompt_shelf = app.ai_shelf.clone();
            self.settings.save();
            self.client.set_endpoints(self.settings.endpoints());
            self.settings_dirty = false;
        } else if self.settings.prompt_shelf != app.ai_shelf {
            self.settings_dirty = true;
        }
        self.find_menu(app, ctx);
        self.health_window(app, ctx);
        self.palette_window(app, ctx);
    }

    pub fn recording(&self) -> bool {
        self.voice.as_ref().is_some_and(|v| v.recorder.recording)
    }

    pub fn panel(&mut self, app: &mut App, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("✨ AI Copilot").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.selectable_label(self.show_settings, "⚙").on_hover_text("AI Settings: providers, models, voice").clicked() {
                    self.show_settings = !self.show_settings;
                }
                if ui.small_button("Clear").on_hover_text("Clear the conversation").clicked() {
                    self.entries.truncate(1);
                    self.history.clear();
                }
            });
        });
        let roles = format!(
            "Decision: {}   Builder: {}",
            self.router().map(|r| r.model).unwrap_or_else(|| "off".into()),
            self.builder().map(|b| b.label()).unwrap_or_else(|| "none".into())
        );
        ui.label(RichText::new(roles).small().weak());
        if let Some((m, f)) = &self.vram {
            let col = if *f > 0.95 { Color32::from_rgb(120, 200, 120) } else { Color32::from_rgb(230, 170, 70) };
            ui.label(RichText::new(format!("{m}: {:.0}% in GPU memory", f * 100.0)).small().color(col));
        }
        ui.separator();
        if self.show_settings {
            egui::ScrollArea::vertical().id_salt("ai_settings").max_height(ui.available_height() * 0.6).show(ui, |ui| self.settings_ui(ui));
            ui.separator();
        }
        // Quick actions.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            if ui.small_button("Explain scene").clicked() {
                self.entries.push(Entry::new(Role::User, "Explain the scene"));
                self.run_task(app, Task::Help, "Briefly describe what is in the current scene and suggest one next step.".into());
            }
            if ui.small_button("Health check").clicked() {
                app.ai_requests.push(AiUiRequest::HealthCheck);
            }
            if ui.small_button("Rename selected").clicked() {
                if app.sel.is_empty() {
                    self.entries.push(Entry::new(Role::System, "Select the objects to rename first."));
                } else {
                    let d = context::selection_details(app);
                    self.run_task(app, Task::SmartRename, d);
                }
            }
            if ui.small_button("Color selected…").clicked() {
                self.palette_open = true;
            }
            if ui.small_button("Bounce selected").clicked() {
                self.submit(app, "Animate the selected object bouncing three times over 72 frames, then play.".into());
            }
            self.recipe_buttons(app, ui);
        });
        ui.separator();
        // Input area at the bottom.
        let input_h = 96.0;
        let chat_h = (ui.available_height() - input_h).max(80.0);
        egui::ScrollArea::vertical().id_salt("ai_chat").max_height(chat_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            let mut run: Option<Vec<String>> = None;
            for (i, e) in self.entries.iter().enumerate() {
                let (bg, who) = match e.role {
                    Role::User => (Color32::from_rgb(58, 72, 86), "You"),
                    Role::Assistant => (Color32::from_rgb(56, 56, 56), "AI"),
                    Role::System => (Color32::from_rgb(64, 58, 48), "ℹ"),
                };
                egui::Frame::new().fill(bg).corner_radius(4).inner_margin(6).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(who).strong().small());
                        if e.role == Role::Assistant && e.text.is_empty() && self.current == Some(i) {
                            ui.spinner();
                            ui.label(RichText::new(&self.route_note).small().weak());
                        }
                    });
                    if let Some(t) = &e.image {
                        let w = ui.available_width().min(t.size_vec2().x * 1.5);
                        let h = w * t.size_vec2().y / t.size_vec2().x;
                        ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(w, h)));
                    }
                    if !e.text.is_empty() {
                        ui.label(&e.text);
                    }
                    if !e.meta.is_empty() {
                        ui.label(RichText::new(&e.meta).small().color(Color32::from_rgb(150, 180, 210)));
                    }
                    if !e.mel.is_empty() {
                        egui::CollapsingHeader::new(RichText::new(format!("{} MEL statement(s)", e.mel.len())).small()).id_salt(("mel", i)).show(ui, |ui| {
                            for m in &e.mel {
                                ui.label(RichText::new(m).monospace().small());
                            }
                        });
                    }
                    for err in &e.errors {
                        ui.label(RichText::new(format!("⚠ {err}")).small().color(Color32::from_rgb(240, 170, 70)));
                    }
                    if !e.suggested.is_empty() && ui.small_button(format!("▶ Run {} suggested statement(s)", e.suggested.len())).clicked() {
                        run = Some(e.suggested.clone());
                    }
                });
                ui.add_space(4.0);
            }
            if let Some(mel) = run {
                app.begin_batch();
                let errs = run_mel(app, &mel);
                app.end_batch();
                let mut e = Entry::new(Role::System, format!("Ran {} statement(s).", mel.len()));
                e.errors = errs;
                self.entries.push(e);
            }
        });
        ui.separator();
        // Mic + input.
        let recording = self.recording();
        ui.horizontal(|ui| {
            let mic_label = if recording { RichText::new("⏺ Listening…").color(Color32::from_rgb(240, 80, 80)) } else { RichText::new("🎤 Voice") };
            if ui.add(egui::Button::new(mic_label)).on_hover_text("Speak a command (` key). Hands-free mode stops when you pause.").clicked() {
                self.toggle_mic();
            }
            if recording {
                let lvl = self.voice.as_ref().map(|v| v.recorder.level()).unwrap_or(0.0);
                ui.add(egui::ProgressBar::new((lvl * 8.0).min(1.0)).desired_width(80.0));
            } else if self.voice_ui.transcribing {
                ui.spinner();
                ui.label(RichText::new("transcribing").small());
            } else if let Some((f, p)) = &self.voice_ui.download {
                ui.add(egui::ProgressBar::new(p.max(0.0)).desired_width(90.0).text(f.clone()));
            } else if let Some(e) = &self.voice_ui.error {
                ui.label(RichText::new(e).small().color(Color32::from_rgb(240, 170, 70))).on_hover_text(e);
            } else if !self.voice_ui.status.is_empty() {
                ui.label(RichText::new(&self.voice_ui.status).small().weak());
            }
            if self.speaker.speaking() && ui.small_button("🔇").on_hover_text("Stop speaking").clicked() {
                self.speaker.stop();
            }
        });
        let resp = ui.add(egui::TextEdit::multiline(&mut self.input).desired_rows(2).desired_width(f32::INFINITY).hint_text("Ask or describe… (Enter to send, Shift+Enter new line, ↑ history)").id_salt("ai_prompt"));
        if self.focus_input {
            resp.request_focus();
            self.focus_input = false;
        }
        let (enter, shift, up, down) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.shift, i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
        let mut send = false;
        if resp.has_focus() {
            if enter && !shift {
                send = true;
            }
            let h = &self.settings.prompt_history;
            if up && !h.is_empty() && (self.input.is_empty() || self.hist_cursor.is_some()) {
                let i = self.hist_cursor.map(|c| c.saturating_sub(1)).unwrap_or(h.len() - 1);
                self.hist_cursor = Some(i);
                self.input = h[i].clone();
            } else if down && self.hist_cursor.is_some() {
                let i = self.hist_cursor.unwrap() + 1;
                if i < h.len() {
                    self.hist_cursor = Some(i);
                    self.input = h[i].clone();
                } else {
                    self.hist_cursor = None;
                    self.input.clear();
                }
            }
        }
        ui.horizontal(|ui| {
            if self.busy {
                if ui.button("⏹ Stop").clicked() {
                    self.client.stop();
                }
            } else if ui.button("Send ➤").clicked() {
                send = true;
            }
            if ui.button("⭐").on_hover_text("Save this prompt as a button on the AI shelf").clicked() && !self.input.trim().is_empty() {
                let label: String = self.input.split_whitespace().take(2).collect::<Vec<_>>().join(" ").chars().take(9).collect();
                app.ai_shelf.push((label, self.input.trim().to_string()));
                app.shelf_tab = super::menus::AI_SHELF;
                self.settings_dirty = true;
            }
            if self.busy_task.is_some() {
                ui.spinner();
                ui.label(RichText::new(self.busy_task.map(|t| t.label()).unwrap_or("")).small());
            }
        });
        if send {
            let p = std::mem::take(&mut self.input).trim_end_matches('\n').to_string();
            self.submit(app, p);
            self.focus_input = true;
        }
    }

    // ---------------------------------------------------------------------- settings

    fn model_picker(ui: &mut egui::Ui, id: &str, label: &str, slot: &mut Option<ModelRef>, none_label: &str, ollama: &[String], openrouter: &[String]) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.add_sized([62.0, 18.0], egui::Label::new(label));
            let text = slot.as_ref().map(|m| m.label()).unwrap_or_else(|| none_label.to_string());
            egui::ComboBox::from_id_salt(id).selected_text(text).width(ui.available_width() - 8.0).height(360.0).show_ui(ui, |ui| {
                if ui.selectable_label(slot.is_none(), none_label).clicked() {
                    *slot = None;
                    changed = true;
                }
                ui.label(RichText::new("Ollama (local)").strong().small());
                if ollama.is_empty() {
                    ui.label(RichText::new("  no local models found").weak().small());
                }
                for m in ollama {
                    let r = ModelRef::new(Provider::Ollama, m.clone());
                    if ui.selectable_label(slot.as_ref() == Some(&r), format!("  {m}")).clicked() {
                        *slot = Some(r);
                        changed = true;
                    }
                }
                ui.label(RichText::new("OpenRouter (cloud)").strong().small());
                for m in openrouter {
                    let r = ModelRef::new(Provider::OpenRouter, m.clone());
                    if ui.selectable_label(slot.as_ref() == Some(&r), format!("  {m}")).clicked() {
                        *slot = Some(r);
                        changed = true;
                    }
                }
            });
        });
        changed
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.label(RichText::new("Providers").strong());
        ui.horizontal(|ui| {
            ui.label("Ollama host");
            changed |= ui.add(egui::TextEdit::singleline(&mut self.settings.ollama_host).desired_width(170.0)).changed();
            if ui.small_button("⟳").on_hover_text("Refresh model lists").clicked() {
                self.client.set_endpoints(self.settings.endpoints());
                self.client.send(AiRequest::ListModels);
            }
        });
        match &self.ollama {
            Ok(v) => ui.label(RichText::new(format!("✔ Ollama: {} model(s)", v.len())).small().color(Color32::from_rgb(130, 200, 130))),
            Err(e) => ui.label(RichText::new(format!("✖ {e}")).small().color(Color32::from_rgb(230, 150, 80))),
        };
        ui.horizontal(|ui| {
            ui.label("OpenRouter key");
            changed |= ui.add(egui::TextEdit::singleline(&mut self.settings.openrouter_key).password(true).desired_width(150.0).hint_text("sk-or-…")).changed();
        });
        if self.settings.openrouter_key.is_empty() && std::env::var("OPENROUTER_API_KEY").is_ok() {
            ui.label(RichText::new("Using OPENROUTER_API_KEY from the environment").small().weak());
        }
        match &self.openrouter {
            Ok(v) => ui.label(RichText::new(format!("✔ OpenRouter catalogue: {} models", v.len())).small().color(Color32::from_rgb(130, 200, 130))),
            Err(e) => ui.label(RichText::new(format!("✖ {e}")).small().color(Color32::from_rgb(230, 150, 80))),
        };
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.or_filter).desired_width(90.0).hint_text("qwen, claude…"));
            ui.checkbox(&mut self.or_tools_only, "tools");
            ui.checkbox(&mut self.or_free_only, "free");
        });
        let ollama: Vec<String> = self.installed_ollama();
        let mut or_list: Vec<String> = match &self.openrouter {
            Ok(v) => v
                .iter()
                .filter(|m| (!self.or_tools_only || m.tools) && (!self.or_free_only || m.free) && (self.or_filter.is_empty() || m.id.to_lowercase().contains(&self.or_filter.to_lowercase())))
                .take(150)
                .map(|m| m.id.clone())
                .collect(),
            Err(_) => ai::SUGGESTED_OPENROUTER.iter().map(|s| s.to_string()).collect(),
        };
        if or_list.is_empty() {
            or_list = ai::SUGGESTED_OPENROUTER.iter().map(|s| s.to_string()).collect();
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Model roles").strong());
        changed |= Self::model_picker(ui, "role_router", "Decision", &mut self.settings.router, "Auto (tev1 if installed)", &ollama, &or_list);
        if self.settings.router.is_none() {
            changed |= ui.checkbox(&mut self.settings.auto_router, "Use an installed tev1 automatically").changed();
        }
        changed |= Self::model_picker(ui, "role_builder", "Builder", &mut self.settings.builder, "Auto (best installed)", &ollama, &or_list);
        changed |= Self::model_picker(ui, "role_helper", "Helper", &mut self.settings.helper, "Same as builder", &ollama, &or_list);
        changed |= Self::model_picker(ui, "role_fallback", "Fallback", &mut self.settings.fallback, "None", &ollama, &or_list);
        changed |= Self::model_picker(ui, "role_vision", "Vision", &mut self.settings.vision, "Auto (vision model if available)", &ollama, &or_list);
        ui.label(RichText::new(format!("Render check uses: {}", self.vision_model().map(|m| m.label()).unwrap_or_else(|| "text-only helper".into()))).small().weak());
        ui.horizontal(|ui| {
            ui.label("Temperature");
            changed |= ui.add(egui::Slider::new(&mut self.settings.temperature, 0.0..=1.0)).changed();
        });
        ui.horizontal(|ui| {
            ui.label("Pull");
            ui.add(egui::TextEdit::singleline(&mut self.pull_name).desired_width(110.0));
            if ui.add_enabled(self.pull.is_none(), egui::Button::new("Download")).on_hover_text("ollama pull").clicked() {
                self.client.set_endpoints(self.settings.endpoints());
                self.client.send(AiRequest::Pull(self.pull_name.trim().to_string()));
                self.pull = Some(("starting".into(), -1.0));
            }
        });
        if let Some((s, f)) = &self.pull {
            ui.add(egui::ProgressBar::new(f.max(0.0)).text(s.clone()));
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Voice (Whisper, runs locally)").strong());
        ui.horizontal(|ui| {
            ui.label("Model");
            let before = self.settings.whisper_model.clone();
            egui::ComboBox::from_id_salt("whisper").selected_text(&self.settings.whisper_model).show_ui(ui, |ui| {
                for (n, _) in crate::voice::WHISPER_MODELS {
                    ui.selectable_value(&mut self.settings.whisper_model, n.to_string(), *n);
                }
            });
            if before != self.settings.whisper_model {
                changed = true;
                if let Some(v) = &self.voice {
                    v.load(&self.settings.whisper_model);
                }
            }
            if self.voice.is_none() && ui.small_button("Load").clicked() {
                self.ensure_voice();
            }
        });
        changed |= ui.checkbox(&mut self.settings.hands_free, "Hands-free: stop when I pause").changed();
        changed |= ui.checkbox(&mut self.settings.voice_auto_send, "Send transcripts immediately").changed();
        ui.horizontal(|ui| {
            ui.label("Mic threshold");
            changed |= ui.add(egui::Slider::new(&mut self.settings.vad_threshold, 0.002..=0.1).logarithmic(true)).changed();
        });
        ui.horizontal(|ui| {
            ui.label("Pause (s)");
            changed |= ui.add(egui::Slider::new(&mut self.settings.silence_secs, 0.4..=3.0)).changed();
        });
        changed |= ui.checkbox(&mut self.settings.speak_replies, "Read replies aloud (system TTS)").changed();
        ui.label(RichText::new(format!("Settings file: {}", AiSettings::path().display())).small().weak());
        if changed {
            self.settings_dirty = true;
        }
    }

    // --------------------------------------------------------------------- windows

    fn find_menu(&mut self, app: &mut App, ctx: &egui::Context) {
        if !self.find_open {
            return;
        }
        let mut open = true;
        let mut run: Option<String> = None;
        let mut ask: Option<String> = None;
        egui::Window::new("Find Menu").open(&mut open).collapsible(false).default_width(380.0).anchor(egui::Align2::CENTER_TOP, [0.0, 90.0]).show(ctx, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.find_query).hint_text("Search commands — or ask the AI").desired_width(f32::INFINITY));
            if self.find_focus {
                r.request_focus();
                self.find_focus = false;
            }
            let mut hits: Vec<(i32, &str, &str)> = FIND_ITEMS.iter().filter_map(|(l, m)| fuzzy(&self.find_query, l).map(|s| (s, *l, *m))).collect();
            hits.sort_by(|a, b| b.0.cmp(&a.0));
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            for (i, (_, l, m)) in hits.iter().take(10).enumerate() {
                let resp = ui.add(egui::Button::selectable(i == 0, format!("{l}    ")).right_text(RichText::new(*m).monospace().weak()));
                if resp.clicked() || (i == 0 && enter && !self.find_query.is_empty()) {
                    run = Some(m.to_string());
                }
            }
            if !self.find_query.trim().is_empty() {
                ui.separator();
                if ui.button(format!("✨ Ask AI: \"{}\"", self.find_query.trim())).clicked() || (hits.is_empty() && enter) {
                    ask = Some(self.find_query.trim().to_string());
                }
            }
        });
        if let Some(m) = run {
            let r = crate::commands::execute(app, &m);
            super::report(app, r);
            open = false;
        }
        if let Some(q) = ask {
            let l = q.to_lowercase();
            let question = q.ends_with('?') || ["how", "what", "where", "why", "which", "can i", "is there"].iter().any(|w| l.starts_with(w));
            if question {
                // Help task: answers with hotkeys/menus and offers MEL as a button.
                self.entries.push(Entry::new(Role::User, &q));
                self.run_task(app, Task::Help, q);
                self.open = true;
            } else {
                self.submit(app, q);
            }
            open = false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        self.find_open = open;
    }

    fn health_window(&mut self, app: &mut App, ctx: &egui::Context) {
        if !self.health_open {
            return;
        }
        let mut open = true;
        let mut apply: Option<Vec<String>> = None;
        egui::Window::new("Scene Health Check").open(&mut open).default_size([460.0, 380.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⟳ Re-scan").clicked() {
                    self.health_report = context::health_report(app);
                    self.health_ai = None;
                }
                let busy = self.busy_task == Some(Task::HealthCheck);
                if ui.add_enabled(!busy, egui::Button::new("✨ Ask AI for advice")).clicked() {
                    let report = self.health_report.clone();
                    self.run_task(app, Task::HealthCheck, format!("Automatic scene report:\n{report}"));
                }
                if busy {
                    ui.spinner();
                }
            });
            egui::ScrollArea::vertical().max_height(170.0).id_salt("health_report").show(ui, |ui| {
                for l in self.health_report.lines() {
                    let col = if l.starts_with("[high]") {
                        Color32::from_rgb(240, 100, 90)
                    } else if l.starts_with("[medium]") {
                        Color32::from_rgb(240, 170, 70)
                    } else {
                        Color32::from_rgb(200, 200, 200)
                    };
                    ui.label(RichText::new(l).small().color(col));
                }
            });
            if let Some((advice, mel)) = &self.health_ai {
                ui.separator();
                ui.label(RichText::new("AI advice").strong());
                egui::ScrollArea::vertical().id_salt("health_ai").max_height(140.0).show(ui, |ui| {
                    ui.label(advice);
                    for m in mel {
                        ui.label(RichText::new(m).monospace().small());
                    }
                });
                if !mel.is_empty() && ui.button(format!("▶ Apply {} fix(es) (one undo step)", mel.len())).clicked() {
                    apply = Some(mel.clone());
                }
            }
        });
        if let Some(mel) = apply {
            app.begin_batch();
            let errs = run_mel(app, &mel);
            app.end_batch();
            self.health_report = context::health_report(app);
            let mut e = Entry::new(Role::System, format!("Applied {} health fix(es).", mel.len()));
            e.errors = errs;
            self.entries.push(e);
        }
        self.health_open = open;
    }

    fn palette_window(&mut self, app: &mut App, ctx: &egui::Context) {
        if !self.palette_open {
            return;
        }
        let mut open = true;
        let mut go = false;
        egui::Window::new("AI Color Selected").open(&mut open).collapsible(false).default_width(320.0).show(ctx, |ui| {
            ui.label(format!("{} object(s) selected", app.sel.len()));
            ui.label("Describe a mood or palette:");
            let r = ui.text_edit_singleline(&mut self.palette_prompt);
            ui.horizontal_wrapped(|ui| {
                for p in ["warm sunset", "ocean", "forest", "neon cyberpunk", "pastel", "metallic sci-fi", "autumn"] {
                    if ui.small_button(p).clicked() {
                        self.palette_prompt = p.into();
                    }
                }
            });
            if ui.add_enabled(!app.sel.is_empty(), egui::Button::new("✨ Apply palette")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                go = true;
            }
        });
        if go && !app.sel.is_empty() {
            let d = context::selection_details(app);
            self.run_task(app, Task::Palette, format!("Palette: {}\nSelected objects:\n{d}", self.palette_prompt));
            open = false;
        }
        self.palette_open = open;
    }
}

/// Execute MEL statements; returns error strings.
fn run_mel(app: &mut App, mel: &[String]) -> Vec<String> {
    let mut errs = Vec::new();
    for m in mel {
        if let Err(e) = crate::commands::execute(app, m) {
            app.log.push(format!("// Error: {e}"));
            errs.push(format!("{m}: {e}"));
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_ranks_substrings_first() {
        assert!(fuzzy("cube", "Create Polygon Cube").unwrap() > fuzzy("cpc", "Create Polygon Cube").unwrap());
        assert!(fuzzy("zzz", "Undo").is_none());
    }
}
