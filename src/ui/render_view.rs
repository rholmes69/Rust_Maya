//! Render View window: progressive CPU path-traced renders of the active view or a camera.

use crate::app::App;
use crate::raytrace::{Pass, ToneMap};
use crate::render_view::RenderCam;
use crate::scene::Shape;
use eframe::egui::{self, Color32, RichText};

pub struct RenderViewUi {
    tex: Option<egui::TextureHandle>,
    shown: (u64, f32, ToneMap, usize, bool, Pass),
    pass: Pass,
    region_start: Option<egui::Pos2>,
    last_update: std::time::Instant,
    seq_start: i32,
    seq_end: i32,
    seq_path: String,
    seq_open: bool,
    zoom_fit: bool,
}

impl Default for RenderViewUi {
    fn default() -> Self {
        RenderViewUi {
            tex: None,
            shown: (u64::MAX, 0.0, ToneMap::Srgb, 0, false, Pass::Beauty),
            pass: Pass::Beauty,
            region_start: None,
            last_update: std::time::Instant::now(),
            seq_start: 1,
            seq_end: 24,
            seq_path: String::new(),
            seq_open: false,
            zoom_fit: true,
        }
    }
}

const RES: [(u32, u32, &str); 6] = [(320, 180, "320×180"), (640, 360, "640×360"), (960, 540, "960×540 (HD 540)"), (1280, 720, "1280×720 (HD 720)"), (1920, 1080, "1920×1080 (HD 1080)"), (1024, 1024, "1024×1024")];

pub fn render_view_window(app: &mut App, ctx: &egui::Context, st: &mut RenderViewUi) {
    if !app.render.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Render View").open(&mut open).default_size([860.0, 600.0]).resizable(true).show(ctx, |ui| {
        toolbar(app, ui, st);
        ui.separator();
        image(app, ui, st);
    });
    if !open {
        app.render.open = false;
        app.render.ipr = false;
        app.stop_render();
    }
}

fn toolbar(app: &mut App, ui: &mut egui::Ui, st: &mut RenderViewUi) {
    let running = app.render.job.as_ref().is_some_and(|j| !j.is_done());
    ui.horizontal_wrapped(|ui| {
        if ui.button(RichText::new("🎬 Render").strong()).on_hover_text("Render the current frame").clicked() {
            app.start_render();
        }
        let ipr = app.render.ipr;
        if ui.selectable_label(ipr, "IPR").on_hover_text("Interactive: re-render whenever the scene, camera or settings change").clicked() {
            app.render.ipr = !ipr;
            if app.render.ipr {
                app.start_render();
            }
        }
        if ui.add_enabled(running || app.render.seq.is_some(), egui::Button::new("■ Stop")).clicked() {
            app.render.ipr = false;
            app.stop_render();
        }
        if ui.add_enabled(app.render.job.is_some(), egui::Button::new("💾 Save…")).clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("render.png").save_file()
        {
            let r = app.save_render(&p.to_string_lossy()).map(|_| format!("Saved {}", p.display()));
            super::report(app, r);
        }
        ui.separator();
        ui.label("Camera");
        let cams: Vec<(crate::scene::NodeId, String)> = app.scene.nodes.iter().filter(|n| matches!(n.shape, Shape::Camera(_))).map(|n| (n.id, n.name.clone())).collect();
        let cur = match app.render.camera {
            RenderCam::ActiveView => "Active view".to_string(),
            RenderCam::Node(id) => cams.iter().find(|c| c.0 == id).map(|c| c.1.clone()).unwrap_or("Active view".into()),
        };
        egui::ComboBox::from_id_salt("rv_cam").selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.render.camera, RenderCam::ActiveView, "Active view");
            for (id, n) in &cams {
                ui.selectable_value(&mut app.render.camera, RenderCam::Node(*id), n);
            }
        });
        let s = &mut app.render.settings;
        egui::ComboBox::from_id_salt("rv_res").selected_text(format!("{}×{}", s.width, s.height)).show_ui(ui, |ui| {
            for (w, h, l) in RES {
                if ui.selectable_label(s.width == w && s.height == h, l).clicked() {
                    s.width = w;
                    s.height = h;
                }
            }
        });
        ui.label("Samples");
        ui.add(egui::DragValue::new(&mut s.spp).range(1..=16384).speed(4.0));
        ui.label("Bounces");
        ui.add(egui::DragValue::new(&mut s.bounces).range(0..=32));
        if ui.add_enabled(app.render.job.is_some(), egui::Button::new("Save passes…")).on_hover_text("Beauty, albedo, normal, depth and object-ID PNGs").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("render.png").save_file()
        {
            let r = app.save_render_passes(&p.to_string_lossy()).map(|f| format!("Saved {} passes ({} …)", f.len(), f.first().cloned().unwrap_or_default()));
            super::report(app, r);
        }
        if ui.selectable_label(st.seq_open, "Sequence…").on_hover_text("Render an image sequence").clicked() {
            st.seq_open ^= true;
            if st.seq_path.is_empty() {
                st.seq_start = app.scene.play_start as i32;
                st.seq_end = app.scene.play_end as i32;
                let dir = app.file_path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                st.seq_path = dir.join("images").join("render").to_string_lossy().to_string();
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let s = &mut app.render.settings;
        ui.label("Exposure");
        ui.add(egui::Slider::new(&mut s.exposure, -5.0..=5.0).step_by(0.1));
        egui::ComboBox::from_id_salt("rv_tm").selected_text(if s.tonemap == ToneMap::Filmic { "Filmic" } else { "sRGB" }).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.tonemap, ToneMap::Srgb, "sRGB (viewport match)");
            ui.selectable_value(&mut s.tonemap, ToneMap::Filmic, "Filmic (ACES)");
        });
        ui.label("Sky");
        ui.add(egui::Slider::new(&mut s.env, 0.0..=2.0).step_by(0.05));
        ui.checkbox(&mut s.env_background, "Sky background");
        ui.checkbox(&mut s.denoise, "Denoise").on_hover_text("Edge-aware denoiser guided by albedo and normals (applied to the display and saved images)");
        ui.label("Clamp");
        ui.add(egui::DragValue::new(&mut s.clamp).range(0.1..=1000.0).speed(0.1));
        ui.checkbox(&mut st.zoom_fit, "Fit");
        egui::ComboBox::from_id_salt("rv_pass").selected_text(st.pass.name()).show_ui(ui, |ui| {
            for p in Pass::ALL {
                ui.selectable_value(&mut st.pass, p, p.name());
            }
        });
        match app.render.settings.region {
            Some(_) => {
                if ui.button("✖ Region").on_hover_text("Render the whole frame again").clicked() {
                    app.render.settings.region = None;
                }
            }
            None => {
                ui.label(RichText::new("Shift-drag: render region").small().weak());
            }
        }
    });
    if st.seq_open {
        ui.horizontal_wrapped(|ui| {
            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut st.seq_start));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut st.seq_end));
            ui.label("Output");
            ui.add(egui::TextEdit::singleline(&mut st.seq_path).desired_width(280.0));
            if ui.button("…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("render.png").save_file()
            {
                st.seq_path = p.with_extension("").to_string_lossy().to_string();
            }
            ui.label(RichText::new(format!("→ {}", crate::render_view::frame_path(&st.seq_path, st.seq_start))).weak());
            if ui.add_enabled(app.render.seq.is_none() && !st.seq_path.is_empty(), egui::Button::new("Render Sequence")).clicked() {
                app.render.ipr = false;
                app.start_sequence(st.seq_start, st.seq_end, st.seq_path.clone());
            }
        });
    }
}

fn image(app: &mut App, ui: &mut egui::Ui, st: &mut RenderViewUi) {
    let ctx = ui.ctx().clone();
    let (exp, tm) = (app.render.settings.exposure, app.render.settings.tonemap);
    // Status line.
    let mut status = String::new();
    let mut progress = None;
    let denoise = app.render.settings.denoise;
    if let Some(job) = app.render.job.as_mut() {
        job.settings.denoise = denoise;
        let (w, h) = job.size();
        let done = job.is_done();
        let secs = job.elapsed().as_secs_f32();
        let spp = job.samples();
        status = format!("{w}×{h}   {spp}/{} spp   {secs:.1}s   {} triangles", job.settings.spp, job.tri_count);
        if done && job.was_cancelled() {
            status.push_str("   (stopped)");
        }
        if !done {
            progress = Some(job.progress());
        }
        // Refresh the image at most ~6×/s while rendering (and on exposure / tone-map edits).
        let id = job.id as usize;
        let key = (job.version(), exp, tm, id, denoise, st.pass);
        let stale = key != st.shown;
        let interval = if denoise { 0.7 } else { 0.16 };
        let due = st.last_update.elapsed().as_secs_f32() > interval || done || key.1 != st.shown.1 || key.2 != st.shown.2 || key.4 != st.shown.4 || key.5 != st.shown.5;
        if stale && due {
            let rgba = job.pass_rgba8(st.pass, exp, tm);
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            match &mut st.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => st.tex = Some(ctx.load_texture("render_view", img, egui::TextureOptions::LINEAR)),
            }
            st.shown = key;
            st.last_update = std::time::Instant::now();
        }
    }
    ui.horizontal(|ui| {
        if let Some(p) = progress {
            ui.add(egui::ProgressBar::new(p).desired_width(160.0).show_percentage());
        }
        if let Some(seq) = &app.render.seq {
            ui.label(RichText::new(format!("Sequence: frame {} of {}–{}", seq.frame, seq.start, seq.end)).color(Color32::from_rgb(240, 200, 90)));
        }
        ui.label(RichText::new(status).monospace());
        if !app.render.last_message.is_empty() {
            ui.label(RichText::new(&app.render.last_message).weak());
        }
    });
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(avail.max(egui::vec2(64.0, 64.0)), egui::Sense::drag());
    let painter = ui.painter_at(rect);
    // Checkerboard behind transparent pixels.
    painter.rect_filled(rect, 0.0, Color32::from_rgb(30, 30, 30));
    let Some(tex) = &st.tex else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Click 🎬 Render (or IPR) to render the active view.", egui::FontId::proportional(14.0), Color32::GRAY);
        return;
    };
    let size = tex.size_vec2();
    let scale = if st.zoom_fit { (rect.width() / size.x).min(rect.height() / size.y) } else { 1.0 };
    let r = egui::Rect::from_center_size(rect.center(), size * scale);
    let cell = 12.0;
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = r.left() + if row % 2 == 0 { 0.0 } else { cell };
        while x < r.right() {
            let c = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell)).intersect(r);
            painter.rect_filled(c, 0.0, Color32::from_rgb(52, 52, 52));
            x += cell * 2.0;
        }
        y += cell;
        row += 1;
    }
    painter.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    // Render region: Shift-drag on the image.
    let to_norm = |p: egui::Pos2| egui::pos2(((p.x - r.left()) / r.width()).clamp(0.0, 1.0), ((p.y - r.top()) / r.height()).clamp(0.0, 1.0));
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.drag_started() && shift {
        st.region_start = resp.interact_pointer_pos();
    }
    let mut live: Option<egui::Rect> = None;
    if let (Some(a), Some(b)) = (st.region_start, resp.interact_pointer_pos()) {
        live = Some(egui::Rect::from_two_pos(a, b));
        if resp.drag_stopped() {
            st.region_start = None;
            let (na, nb) = (to_norm(a), to_norm(b));
            if (nb.x - na.x).abs() > 0.01 && (nb.y - na.y).abs() > 0.01 {
                app.render.settings.region = Some([na.x.min(nb.x), na.y.min(nb.y), na.x.max(nb.x), na.y.max(nb.y)]);
                app.start_render();
            }
        }
    }
    let reg = live.or_else(|| app.render.settings.region.map(|[x0, y0, x1, y1]| egui::Rect::from_min_max(egui::pos2(r.left() + x0 * r.width(), r.top() + y0 * r.height()), egui::pos2(r.left() + x1 * r.width(), r.top() + y1 * r.height()))));
    if let Some(rr) = reg {
        painter.rect_stroke(rr, 0.0, egui::Stroke::new(1.5, Color32::from_rgb(255, 210, 60)), egui::StrokeKind::Middle);
    }
}

