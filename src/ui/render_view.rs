//! Render View window: progressive CPU path-traced renders of the active view or a camera.

use crate::app::App;
use crate::raytrace::ToneMap;
use crate::render_view::RenderCam;
use crate::scene::Shape;
use eframe::egui::{self, Color32, RichText};

pub struct RenderViewUi {
    tex: Option<egui::TextureHandle>,
    shown: (u64, f32, ToneMap, usize),
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
            shown: (u64::MAX, 0.0, ToneMap::Srgb, 0),
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
        ui.label("Clamp");
        ui.add(egui::DragValue::new(&mut s.clamp).range(0.1..=1000.0).speed(0.1));
        ui.checkbox(&mut st.zoom_fit, "Fit");
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
    if let Some(job) = app.render.job.as_mut() {
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
        let key = (job.version(), exp, tm, id);
        let stale = key != st.shown;
        let due = st.last_update.elapsed().as_secs_f32() > 0.16 || done || key.1 != st.shown.1 || key.2 != st.shown.2;
        if stale && due {
            let rgba = job.rgba8(exp, tm);
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
    let (rect, _) = ui.allocate_exact_size(avail.max(egui::vec2(64.0, 64.0)), egui::Sense::hover());
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
}

