//! Time Slider, Range Slider, Command Line and Help Line.

use crate::app::{App, Tool, CompMode};
use eframe::egui::{self, Color32, Rect, RichText, Sense, Stroke};

pub fn time_slider(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let controls_w = 300.0;
        let w = (ui.available_width() - controls_w).max(100.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, Color32::from_rgb(55, 55, 55));
        let (s, e) = (app.scene.play_start, app.scene.play_end.max(app.scene.play_start + 1.0));
        let span = e - s;
        let x_of = |f: f32| rect.min.x + 8.0 + (f - s) / span * (rect.width() - 16.0);
        // Tick spacing that keeps labels readable.
        let px_per_frame = (rect.width() - 16.0) / span;
        let label_step = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0].into_iter().find(|st| st * px_per_frame >= 40.0).unwrap_or(1000.0);
        let tick_step = if px_per_frame >= 4.0 { 1.0 } else { (label_step / 5.0f32).max(1.0) };
        let mut f = (s / tick_step).ceil() * tick_step;
        while f <= e + 1e-3 {
            let x = x_of(f);
            let major = (f / label_step).fract().abs() < 1e-4;
            let h = if major { 12.0 } else { 5.0 };
            p.line_segment([egui::pos2(x, rect.max.y - h), egui::pos2(x, rect.max.y)], Stroke::new(1.0, Color32::from_rgb(140, 140, 140)));
            if major {
                p.text(egui::pos2(x + 2.0, rect.max.y - 14.0), egui::Align2::LEFT_BOTTOM, format!("{f:.0}"), egui::FontId::proportional(10.5), Color32::from_rgb(190, 190, 190));
            }
            f += tick_step;
        }
        // Keys of the selection.
        for k in app.scene.key_frames(&app.sel) {
            if k >= s && k <= e {
                let x = x_of(k);
                p.rect_filled(Rect::from_min_max(egui::pos2(x - 1.5, rect.min.y + 6.0), egui::pos2(x + 1.5, rect.max.y)), 0.0, Color32::from_rgb(230, 50, 50));
            }
        }
        // Current time.
        let t = app.scene.time;
        let x = x_of(t);
        let cur = Rect::from_min_max(egui::pos2(x - (px_per_frame / 2.0).clamp(1.5, 8.0), rect.min.y), egui::pos2(x + (px_per_frame / 2.0).clamp(1.5, 8.0), rect.max.y));
        p.rect_filled(cur, 0.0, Color32::from_rgba_unmultiplied(200, 200, 200, 120));
        p.text(egui::pos2(cur.max.x + 3.0, rect.min.y + 2.0), egui::Align2::LEFT_TOP, format!("{t:.0}"), egui::FontId::proportional(11.0), Color32::from_rgb(255, 120, 120));
        if (resp.dragged() || resp.clicked())
            && let Some(pos) = resp.interact_pointer_pos() {
                let fr = (s + (pos.x - rect.min.x - 8.0) / (rect.width() - 16.0) * span).round().clamp(s, e);
                if fr != app.scene.time {
                    app.set_time(fr);
                }
            }
        // Current frame field + playback controls.
        let mut tf = app.scene.time;
        if ui.add(egui::DragValue::new(&mut tf).speed(0.2).max_decimals(0)).changed() {
            app.set_time(tf.round());
        }
        let b = |ui: &mut egui::Ui, g: &str, tip: &str| ui.add_sized([24.0, 22.0], egui::Button::new(g)).on_hover_text(tip).clicked();
        if b(ui, "⏮", "Go to start of playback range") {
            let s = app.scene.play_start;
            app.set_time(s);
        }
        if b(ui, "⏪", "Step back one frame (Alt+,)") {
            let t = (app.scene.time - 1.0).max(app.scene.play_start);
            app.set_time(t);
        }
        if b(ui, "◆◀", "Step back to previous key (,)") {
            app.step_key(false);
        }
        if b(ui, if app.playing { "⏸" } else { "▶" }, "Play / stop (Alt+V)") {
            app.playing = !app.playing;
        }
        if b(ui, "▶◆", "Step forward to next key (.)") {
            app.step_key(true);
        }
        if b(ui, "⏩", "Step forward one frame (Alt+.)") {
            let t = (app.scene.time + 1.0).min(app.scene.play_end);
            app.set_time(t);
        }
        if b(ui, "⏭", "Go to end of playback range") {
            let e = app.scene.play_end;
            app.set_time(e);
        }
    });
}

pub fn range_slider(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let sc = &mut app.scene;
        ui.add(egui::DragValue::new(&mut sc.anim_start).speed(0.5).max_decimals(0)).on_hover_text("Animation start");
        ui.add(egui::DragValue::new(&mut sc.play_start).speed(0.5).max_decimals(0)).on_hover_text("Playback start");
        let w = (ui.available_width() - 470.0).max(80.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), Sense::drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, Color32::from_rgb(50, 50, 50));
        let span = (sc.anim_end - sc.anim_start).max(1.0);
        let xa = rect.min.x + (sc.play_start - sc.anim_start) / span * rect.width();
        let xb = rect.min.x + (sc.play_end - sc.anim_start) / span * rect.width();
        let bar = Rect::from_min_max(egui::pos2(xa, rect.min.y + 2.0), egui::pos2(xb.max(xa + 6.0), rect.max.y - 2.0));
        p.rect_filled(bar, 3.0, Color32::from_rgb(120, 120, 120));
        p.text(bar.left_center() + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, format!("{:.0}", sc.play_start), egui::FontId::proportional(10.0), Color32::WHITE);
        p.text(bar.right_center() - egui::vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, format!("{:.0}", sc.play_end), egui::FontId::proportional(10.0), Color32::WHITE);
        if resp.dragged() {
            let df = resp.drag_delta().x / rect.width() * span;
            let len = sc.play_end - sc.play_start;
            let ns = (sc.play_start + df).clamp(sc.anim_start, sc.anim_end - len);
            sc.play_start = ns;
            sc.play_end = ns + len;
        }
        if resp.drag_stopped() {
            sc.play_start = sc.play_start.round();
            sc.play_end = sc.play_end.round();
        }
        ui.add(egui::DragValue::new(&mut sc.play_end).speed(0.5).max_decimals(0)).on_hover_text("Playback end");
        ui.add(egui::DragValue::new(&mut sc.anim_end).speed(0.5).max_decimals(0)).on_hover_text("Animation end");
        sc.play_start = sc.play_start.max(sc.anim_start);
        sc.play_end = sc.play_end.min(sc.anim_end).max(sc.play_start + 1.0);
        ui.separator();
        egui::ComboBox::from_id_salt("fps").selected_text(format!("{} fps", sc.fps)).width(70.0).show_ui(ui, |ui| {
            for f in [12.0, 15.0, 24.0, 25.0, 30.0, 48.0, 50.0, 60.0] {
                ui.selectable_value(&mut sc.fps, f, format!("{f} fps"));
            }
        });
        ui.checkbox(&mut app.loop_playback, "Loop");
        ui.label(RichText::new("No Character Set").weak());
        ui.label(RichText::new("No Anim Layer").weak());
        let key_col = if app.auto_key { Color32::from_rgb(220, 50, 50) } else { Color32::from_rgb(120, 120, 120) };
        if ui.add(egui::Button::new(RichText::new("🔑 Auto").color(key_col))).on_hover_text("Auto Keyframe toggle").clicked() {
            app.auto_key = !app.auto_key;
        }
    });
}

pub fn command_line(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let lang = if app.cmd_ai_mode { RichText::new("AI").color(Color32::from_rgb(200, 140, 255)) } else { RichText::new("MEL") };
        let r = ui.button(lang).on_hover_text("Switch the command line between MEL and AI prompts (like Maya's MEL/Python toggle). Right-click for the Script Editor.");
        if r.clicked() {
            app.cmd_ai_mode = !app.cmd_ai_mode;
        }
        if r.secondary_clicked() {
            app.panels.script_editor ^= true;
        }
        let w = (ui.available_width() * 0.6).max(200.0);
        let hint = if app.cmd_ai_mode { "describe it: make a table with four legs" } else { "polyCube -w 2; move 0 1 0" };
        let resp = ui.add(egui::TextEdit::singleline(&mut app.cmd_input).desired_width(w).font(egui::TextStyle::Monospace).hint_text(hint));
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let cmd = std::mem::take(&mut app.cmd_input);
            if app.cmd_ai_mode {
                app.ai_requests.push(crate::app::AiUiRequest::Prompt(cmd));
            } else {
                let r = crate::commands::execute(app, &cmd);
                super::report(app, r);
            }
        }
        let col = if app.cmd_result.starts_with("Warning") || app.cmd_result.starts_with("Error") { Color32::from_rgb(240, 170, 70) } else { Color32::from_rgb(200, 200, 200) };
        let first = app.cmd_result.lines().next().unwrap_or("").to_string();
        ui.add(egui::Label::new(RichText::new(first).monospace().color(col)).truncate());
    });
}

pub fn help_line(app: &App, ui: &mut egui::Ui) {
    let msg = match (app.tool, app.comp_mode) {
        (_, CompMode::Vertex) => "Vertex mode: click or drag to select vertices; use W/E/R to transform. F8 returns to object mode.",
        (_, CompMode::Face) => "Face mode: click or drag to select faces. Ctrl+E extrudes, Delete removes. F8 returns to object mode.",
        (Tool::EdgeLoop, _) => "Insert Edge Loop: click an edge to cut a loop through the edge ring at that point. Q to exit.",
        (Tool::MultiCut, _) => "Multi-Cut: click points on edges (near a vertex snaps to it); Enter cuts, Esc cancels. Q to exit.",
        (Tool::Sculpt, _) => "Sculpt: LMB-drag on a mesh. Ctrl inverts. Brush, size, strength and X symmetry in Tool Settings.",
        (Tool::QuadDraw, _) => "Quad Draw: click to place dots on the live surface · drag a dot to move it · Shift+click fills a quad from the 4 nearest dots (Ctrl+Shift: triangle) · Ctrl+drag a border edge to extend · Ctrl+click deletes · Enter finishes.",
        (Tool::CurveCv, _) => "CV Curve Tool: click to place control vertices (on the grid, or on the live surface) · Backspace removes the last · Enter creates the curve · Esc cancels.",
        (Tool::CurveEp, _) => "EP Curve Tool: click to place edit points — the curve passes through each one · Backspace removes the last · Enter creates the curve · Esc cancels.",
        (_, CompMode::Edge) => "Edge mode: click or drag to select edges, double-click for a loop. Delete dissolves edges; Bevel/Insert Edge Loop in Edit Mesh.",
        (Tool::Joint, _) => "Create Joints: click in a view to place joints (each becomes the child of the last). Enter finishes and orients the chain. Hold X to snap.",
        (Tool::PaintWeights, _) => "Paint Skin Weights: LMB-drag on the skinned mesh. Pick the influence and Replace/Add/Smooth in Tool Settings. White = 1, black = 0.",
        (Tool::Select, _) => "Select Tool: select an object. Alt+LMB tumble, Alt+MMB track, Alt+RMB / wheel dolly.",
        (Tool::Move, _) => "Move Tool: Use manipulator to move object(s). Hold X to snap to grid, Shift for fine moves.",
        (Tool::Rotate, _) => "Rotate Tool: Use manipulator to rotate object(s) (gimbal axes). Hold X for 15° snapping.",
        (Tool::Scale, _) => "Scale Tool: Use manipulator to scale object(s). Center handle scales uniformly.",
    };
    ui.label(RichText::new(msg).small().color(Color32::from_rgb(180, 180, 180)));
}
