//! Floating editors: Script Editor, Graph Editor, Hotkey list, About.

use crate::app::App;
use eframe::egui::{self, Color32, RichText};

pub fn script_editor(app: &mut App, ctx: &egui::Context) {
    let mut open = app.panels.script_editor;
    egui::Window::new("Script Editor").open(&mut open).default_size([620.0, 420.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Execute (Ctrl+Enter)").clicked() {
                run_script(app);
            }
            if ui.button("Clear History").clicked() {
                app.log.clear();
            }
            if ui.button("Save to Shelf").on_hover_text("Adds the script as a button on the Custom shelf").clicked() && !app.script_input.trim().is_empty() {
                let label = app.script_input.split_whitespace().next().unwrap_or("cmd").chars().take(7).collect::<String>();
                app.custom_shelf.push((label, app.script_input.clone()));
                app.shelf_tab = 8;
            }
            if ui.button("Help").clicked() {
                app.log.push(crate::commands::HELP.to_string());
            }
        });
        // AI helpers (Generate / Explain / Fix).
        ui.horizontal(|ui| {
            use crate::ai::prompts::Task;
            use crate::app::AiUiRequest as R;
            ui.label(RichText::new("✨").color(Color32::from_rgb(200, 140, 255)));
            let r = ui.add(egui::TextEdit::singleline(&mut app.script_ai_prompt).desired_width(260.0).hint_text("Describe a script, e.g. 'stack 10 cubes into a tower'"));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Generate").on_hover_text("Write MEL with AI (placed here for review)").clicked() || enter) && !app.script_ai_prompt.trim().is_empty() {
                app.ai_requests.push(R::Task(Task::GenerateMel, app.script_ai_prompt.trim().to_string()));
            }
            if ui.add_enabled(!app.script_input.trim().is_empty(), egui::Button::new("Explain")).clicked() {
                app.ai_requests.push(R::Task(Task::ExplainMel, app.script_input.clone()));
            }
            if ui.add_enabled(app.last_mel_error.is_some(), egui::Button::new("Fix with AI")).on_hover_text(app.last_mel_error.as_ref().map(|e| e.1.clone()).unwrap_or_else(|| "No error yet".into())).clicked() {
                if let Some((script, err)) = app.last_mel_error.clone() {
                    app.ai_requests.push(R::Task(Task::FixMel, format!("Script:\n{script}\n\nError: {err}")));
                }
            }
        });
        let hist_h = ui.available_height() * 0.6;
        egui::Frame::new().fill(Color32::from_rgb(40, 40, 40)).inner_margin(4.0).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(hist_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                for l in &app.log {
                    let col = if l.starts_with("// Warning") {
                        Color32::from_rgb(240, 170, 70)
                    } else if l.starts_with("// Error") {
                        Color32::from_rgb(240, 90, 90)
                    } else if l.starts_with("//") {
                        Color32::from_rgb(150, 200, 150)
                    } else {
                        Color32::from_rgb(210, 210, 210)
                    };
                    ui.label(RichText::new(l).monospace().color(col));
                }
            });
        });
        ui.label(RichText::new("MEL").weak());
        let resp = ui.add(egui::TextEdit::multiline(&mut app.script_input).code_editor().desired_rows(6).desired_width(f32::INFINITY));
        if resp.has_focus() && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter)) {
            run_script(app);
        }
    });
    app.panels.script_editor = open;
}

fn run_script(app: &mut App) {
    let src = app.script_input.replace('\n', ";");
    let r = crate::commands::execute(app, &src);
    super::report(app, r);
}

pub fn hotkeys(app: &mut App, ctx: &egui::Context) {
    let mut open = app.panels.hotkeys;
    egui::Window::new("Hotkey Editor").open(&mut open).default_size([420.0, 520.0]).show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("hk").striped(true).num_columns(2).show(ui, |ui| {
                for (k, d) in [
                    ("Q / W / E / R", "Select / Move / Rotate / Scale tool"),
                    ("Alt + LMB drag", "Tumble (perspective)"),
                    ("Alt + MMB drag / MMB drag", "Track"),
                    ("Alt + RMB drag / wheel", "Dolly"),
                    ("F / A", "Frame selection / frame all"),
                    ("Space", "Toggle single / four view"),
                    ("4 / 5 / 6", "Wireframe / shaded / textured"),
                    ("7", "Use all lights"),
                    ("Alt+X", "X-Ray"),
                    ("Ctrl+R / Ctrl+B / F10", "Insert edge loop / bevel / edge mode"),
                    ("Graph Editor: Ctrl+click curve", "Insert key"),
                    ("Graph Editor / Dope Sheet: Del", "Delete selected keys"),
                    ("Viewport: drag a red trail marker", "Edit a motion-trail key"),
                    ("F8 / F9 / F11", "Object / vertex / face mode"),
                    ("Shift / Ctrl / Ctrl+Shift click", "Toggle / deselect / add"),
                    ("Ctrl+Shift+A", "Select all"),
                    ("Ctrl+D", "Duplicate"),
                    ("Ctrl+G", "Group"),
                    ("P / Shift+P", "Parent / unparent"),
                    ("Delete / Backspace", "Delete"),
                    ("Ctrl+E", "Extrude faces"),
                    ("Ctrl+H / Shift+H", "Hide / show selection"),
                    ("Z, Ctrl+Z / Shift+Z, Ctrl+Y", "Undo / redo"),
                    ("S", "Set key (all channels)"),
                    ("Shift+W / E / R", "Key translate / rotate / scale"),
                    ("Alt+V", "Play / stop"),
                    (", / .", "Previous / next key"),
                    ("Alt+, / Alt+.", "Previous / next frame"),
                    ("X (hold)", "Snap to grid / 15° while dragging"),
                    ("Shift (hold)", "Fine manipulator drag"),
                    ("Ctrl+N / O / S", "New / open / save scene"),
                    ("Ctrl+A", "Toggle Attribute Editor"),
                    ("Enter / Esc (Joint tool)", "Finish and orient the joint chain"),
                    ("F3 / Ctrl+K", "Find Menu (AI fallback)"),
                    ("Ctrl+L", "Focus the AI Copilot prompt"),
                    ("` (backquote)", "Voice command (Whisper)"),
                ] {
                    ui.label(RichText::new(k).monospace());
                    ui.label(d);
                    ui.end_row();
                }
            });
        });
    });
    app.panels.hotkeys = open;
}

pub fn about(app: &mut App, ctx: &egui::Context) {
    let mut open = app.panels.about;
    egui::Window::new("About Rust Maya").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.heading("Rust Maya");
        ui.label("A Maya-style 3D content creation application written in Rust (egui + wgpu).");
        ui.label("Version 0.1 — modeling & keyframe animation core.");
        ui.add_space(6.0);
        ui.label(RichText::new("Not affiliated with Autodesk. Maya is a trademark of Autodesk, Inc.").weak().small());
    });
    app.panels.about = open;
}
