//! Tool Box, Outliner, Channel Box / Layer Editor, Attribute Editor, Tool Settings.

use crate::app::{App, CompMode, RightTab, Tool};
use crate::mesh::Primitive;
use crate::scene::{Channel, NodeId, Shape, Tangent};
use eframe::egui::{self, Color32, RichText, Sense, Stroke, StrokeKind};

pub const HILITE: Color32 = Color32::from_rgb(82, 133, 166);
const KEYED: Color32 = Color32::from_rgb(205, 95, 95);
const KEYED_ON_FRAME: Color32 = Color32::from_rgb(225, 60, 60);

// ------------------------------------------------------------------ Tool Box

fn tool_icon(ui: &mut egui::Ui, selected: bool, glyph: &str, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), Sense::click());
    let bg = if selected {
        HILITE
    } else if resp.hovered() {
        Color32::from_rgb(95, 95, 95)
    } else {
        Color32::from_rgb(75, 75, 75)
    };
    ui.painter().rect_filled(rect, 3.0, bg);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(16.0), Color32::from_rgb(225, 225, 225));
    resp.on_hover_text(tip)
}

pub fn toolbox(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let tools = [
            (Tool::Select, "↖", "Select Tool (Q)"),
            (Tool::Move, "✚", "Move Tool (W)"),
            (Tool::Rotate, "⟳", "Rotate Tool (E)"),
            (Tool::Scale, "⇲", "Scale Tool (R)"),
            (Tool::Joint, "◎", "Create Joints (Rigging)"),
        ];
        for (t, g, tip) in tools {
            if tool_icon(ui, app.tool == t, g, tip).clicked() {
                if t == Tool::Joint {
                    super::rigging::start_joint_tool(app);
                } else {
                    app.tool = t;
                }
            }
        }
        ui.separator();
        ui.label(RichText::new("Layout").small().weak());
        if tool_icon(ui, !app.quad, "□", "Single Perspective View").clicked() {
            app.quad = false;
            app.active_view = 0;
        }
        if tool_icon(ui, app.quad, "⊞", "Four View (Space)").clicked() {
            app.quad = true;
        }
        if tool_icon(ui, app.panels.outliner, "☰", "Toggle Outliner").clicked() {
            app.panels.outliner ^= true;
        }
        if tool_icon(ui, app.panels.graph_editor, "📈", "Graph Editor").clicked() {
            app.panels.graph_editor ^= true;
        }
    });
}

// ------------------------------------------------------------------ Outliner

#[derive(Default, Clone)]
pub struct OutlinerDrag {
    pub dragging: Option<NodeId>,
}

pub fn outliner(app: &mut App, ui: &mut egui::Ui, drag: &mut OutlinerDrag) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Edit", |ui| {
            if ui.button("Select Hierarchy").clicked() {
                app.select_hierarchy();
            }
            if ui.button("Group  Ctrl+G").clicked() {
                app.group_selected();
            }
            if ui.button("Unparent  Shift+P").clicked() {
                app.unparent_selected();
            }
            if ui.button("Delete").clicked() {
                app.delete_selected();
            }
        });
        ui.menu_button("Display", |ui| {
            if ui.button("Expand All").clicked() {
                for n in &mut app.scene.nodes {
                    n.expanded = true;
                }
            }
            if ui.button("Collapse All").clicked() {
                for n in &mut app.scene.nodes {
                    n.expanded = false;
                }
            }
        });
        ui.menu_button("Show", |ui| {
            ui.label("Filters are not available in this build");
        });
        ui.menu_button("Help", |ui| {
            ui.label("Click to select, Shift/Ctrl to modify.\nDouble-click to rename.\nDrag onto another item to parent,\nonto empty space to unparent.");
        });
    });
    ui.horizontal(|ui| {
        ui.label("🔍");
        ui.add(egui::TextEdit::singleline(&mut app.outliner_filter).hint_text("Search...").desired_width(f32::INFINITY));
    });
    ui.separator();
    let mut rows: Vec<(NodeId, egui::Rect)> = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for cam in ["persp", "top", "front", "side"] {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("🎥").color(Color32::from_rgb(140, 140, 140)));
                ui.label(RichText::new(cam).color(Color32::from_rgb(140, 140, 140)));
            });
        }
        let roots = app.scene.children(None);
        for r in roots {
            outliner_row(app, ui, r, 0, drag, &mut rows);
        }
        for set in ["defaultLightSet", "defaultObjectSet"] {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("◈").color(Color32::from_rgb(170, 120, 200)));
                ui.label(RichText::new(set).color(Color32::from_rgb(140, 140, 140)));
            });
        }
        // Empty area: click clears selection, drop unparents.
        let rest = ui.allocate_response(ui.available_size().max(egui::vec2(10.0, 40.0)), Sense::click());
        if rest.clicked() {
            app.sel.clear();
        }
    });
    // Drag & drop parenting.
    if let Some(src) = drag.dragging {
        let (pos, released) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.any_released()));
        if let Some(p) = pos
            && let Some(n) = app.scene.node(src) {
                ui.painter().text(p + egui::vec2(12.0, 0.0), egui::Align2::LEFT_CENTER, &n.name, egui::FontId::proportional(12.0), Color32::YELLOW);
            }
        if released {
            let target = pos.and_then(|p| rows.iter().find(|(_, r)| r.contains(p)).map(|(id, _)| *id));
            match target {
                Some(t) if t != src => {
                    app.checkpoint();
                    if !app.scene.reparent(src, Some(t)) {
                        app.warn("Cannot parent a node under itself or its descendant.");
                    }
                }
                None if pos.is_some_and(|p| ui.clip_rect().contains(p))
                    && app.scene.node(src).is_some_and(|n| n.parent.is_some()) => {
                        app.checkpoint();
                        app.scene.reparent(src, None);
                    }
                _ => {}
            }
            drag.dragging = None;
        }
    }
}

fn outliner_row(app: &mut App, ui: &mut egui::Ui, id: NodeId, depth: usize, drag: &mut OutlinerDrag, rows: &mut Vec<(NodeId, egui::Rect)>) {
    let Some(node) = app.scene.node(id) else { return };
    let name = node.name.clone();
    let kids = app.scene.children(Some(id));
    let expanded = node.expanded;
    let icon = match node.shape {
        Shape::Group => ("⊡", Color32::from_rgb(220, 200, 120)),
        Shape::Locator => ("✚", Color32::from_rgb(120, 170, 255)),
        Shape::Joint { .. } => ("◎", Color32::from_rgb(120, 200, 255)),
        Shape::IkHandle { .. } => ("⚲", Color32::from_rgb(240, 200, 90)),
        Shape::Curve { .. } => ("○", Color32::from_rgb(110, 220, 110)),
        Shape::ClusterHandle => ("C", Color32::from_rgb(230, 230, 120)),
        Shape::Light(_) => ("☀", Color32::from_rgb(250, 220, 110)),
        Shape::Camera(_) => ("📷", Color32::from_rgb(200, 200, 210)),
        Shape::Lattice { .. } => ("▦", Color32::from_rgb(190, 140, 240)),
        Shape::Particles(_) => ("✺", Color32::from_rgb(255, 180, 90)),
        Shape::Field(_) => ("➶", Color32::from_rgb(140, 230, 200)),
        Shape::Hair(_) => ("〰", Color32::from_rgb(220, 170, 120)),
        Shape::Fluid(_) => ("☁", Color32::from_rgb(150, 190, 235)),
        Shape::Poly { .. } => ("◼", Color32::from_rgb(120, 200, 230)),
    };
    let visible = app.scene.is_visible(id);
    let filter = app.outliner_filter.to_lowercase();
    let matches = filter.is_empty() || name.to_lowercase().contains(&filter);
    if matches {
        let selected = app.sel.contains(&id);
        let h = 20.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click_and_drag());
        rows.push((id, rect));
        if selected {
            ui.painter().rect_filled(rect, 0.0, HILITE);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(80, 80, 80));
        }
        if drag.dragging.is_some() && drag.dragging != Some(id) && resp.contains_pointer() {
            ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::YELLOW), StrokeKind::Inside);
        }
        let x0 = rect.min.x + 4.0 + depth as f32 * 14.0;
        if !kids.is_empty() {
            let arrow_rect = egui::Rect::from_min_size(egui::pos2(x0, rect.min.y), egui::vec2(12.0, h));
            let ar = ui.interact(arrow_rect, ui.scope_id().with(("ol_arrow", id)), Sense::click());
            ui.painter().text(arrow_rect.center(), egui::Align2::CENTER_CENTER, if expanded { "⏷" } else { "⏵" }, egui::FontId::proportional(10.0), Color32::LIGHT_GRAY);
            if ar.clicked()
                && let Some(n) = app.scene.node_mut(id) {
                    n.expanded = !n.expanded;
                }
        }
        ui.painter().text(egui::pos2(x0 + 22.0, rect.center().y), egui::Align2::CENTER_CENTER, icon.0, egui::FontId::proportional(12.0), icon.1);
        let renaming = app.rename.as_ref().is_some_and(|(rid, _)| *rid == id);
        if renaming {
            let text_rect = egui::Rect::from_min_max(egui::pos2(x0 + 32.0, rect.min.y + 1.0), egui::pos2(rect.max.x - 4.0, rect.max.y - 1.0));
            let mut buf = app.rename.as_ref().unwrap().1.clone();
            let te = ui.put(text_rect, egui::TextEdit::singleline(&mut buf));
            if !te.has_focus() && !te.lost_focus() {
                te.request_focus();
            }
            app.rename.as_mut().unwrap().1 = buf.clone();
            if te.lost_focus() {
                let commit = !ui.input(|i| i.key_pressed(egui::Key::Escape));
                app.rename = None;
                if commit {
                    app.rename_node(id, &buf);
                }
            }
        } else {
            let col = if !visible { Color32::from_rgb(120, 120, 120) } else if selected { Color32::WHITE } else { Color32::from_rgb(210, 210, 210) };
            let mut label = name.clone();
            if node_animated(app, id) {
                label.push_str("  ◆");
            }
            ui.painter().text(egui::pos2(x0 + 32.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), col);
        }
        if resp.double_clicked() {
            app.rename = Some((id, name.clone()));
        } else if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            if app.comp_mode != CompMode::Object {
                app.set_comp_mode(CompMode::Object);
            }
            app.select_modify(id, m.shift, m.ctrl);
        }
        if resp.drag_started() {
            drag.dragging = Some(id);
        }
        resp.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                app.rename = Some((id, name.clone()));
            }
            if ui.button("Select Hierarchy").clicked() {
                app.sel = vec![id];
                app.select_hierarchy();
            }
            if ui.button("✨ AI Smart Rename").on_hover_text("Rename the selected objects from their shape, size and position").clicked() {
                if !app.sel.contains(&id) {
                    app.sel = vec![id];
                }
                let d = crate::ai::context::selection_details(app);
                app.ai_requests.push(crate::app::AiUiRequest::Task(crate::ai::prompts::Task::SmartRename, d));
            }
            if ui.button("Duplicate").clicked() {
                app.sel = vec![id];
                app.duplicate_selected();
            }
            if ui.button(if visible { "Hide" } else { "Show" }).clicked() {
                app.checkpoint();
                if let Some(n) = app.scene.node_mut(id) {
                    n.visible = !visible;
                }
            }
            if ui.button("Delete").clicked() {
                app.checkpoint();
                app.scene.delete(&[id]);
                app.sel.retain(|s| *s != id);
            }
        });
    }
    if expanded || !filter.is_empty() {
        for k in kids {
            outliner_row(app, ui, k, depth + 1, drag, rows);
        }
    }
}

fn node_animated(app: &App, id: NodeId) -> bool {
    app.scene.node(id).is_some_and(|n| n.is_animated())
}

// ------------------------------------------------------------- right panel

pub fn right_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for (tab, name) in [(RightTab::ChannelBox, "Channel Box / Layer Editor"), (RightTab::AttributeEditor, "Attribute Editor"), (RightTab::ToolSettings, "Tool Settings")] {
            ui.selectable_value(&mut app.right_tab, tab, RichText::new(name).small());
        }
    });
    ui.separator();
    match app.right_tab {
        RightTab::ChannelBox => {
            let avail = ui.available_height();
            let cb_h = (avail * 0.62).max(200.0);
            egui::ScrollArea::vertical().id_salt("cb_scroll").max_height(cb_h).auto_shrink([false, false]).show(ui, |ui| {
                channel_box(app, ui);
            });
            ui.separator();
            layer_editor(app, ui);
        }
        RightTab::AttributeEditor => {
            egui::ScrollArea::vertical().id_salt("ae_scroll").auto_shrink([false, false]).show(ui, |ui| attribute_editor(app, ui));
        }
        RightTab::ToolSettings => tool_settings(app, ui),
    }
}

/// Channel box row; returns new value if edited.
fn channel_row(app: &mut App, ui: &mut egui::Ui, ch: Channel, value: f32, keyed: bool, on_key: bool) -> Option<f32> {
    let mut out = None;
    ui.horizontal(|ui| {
        let selected = app.selected_channels.contains(&ch);
        let label_w = 96.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(label_w, 18.0), Sense::click());
        if selected {
            ui.painter().rect_filled(rect, 0.0, HILITE);
        }
        ui.painter().text(rect.right_center() - egui::vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, ch.label(), egui::FontId::proportional(12.5), Color32::from_rgb(210, 210, 210));
        if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            if !(m.shift || m.ctrl) {
                app.selected_channels.clear();
            }
            if !app.selected_channels.remove(&ch) {
                app.selected_channels.insert(ch);
            }
        }
        resp.context_menu(|ui| channel_menu(app, ui, ch));
        ui.scope(|ui| {
            if keyed {
                let c = if on_key { KEYED_ON_FRAME } else { KEYED };
                let vis = ui.visuals_mut();
                for w in [&mut vis.widgets.inactive, &mut vis.widgets.hovered] {
                    w.weak_bg_fill = c;
                    w.bg_fill = c;
                }
                vis.extreme_bg_color = c;
            }
            if ch == Channel::Visibility {
                let mut on = value >= 0.5;
                let r = ui.add_sized([ui.available_width() - 6.0, 18.0], egui::Button::new(if on { "on" } else { "off" }));
                if r.clicked() {
                    on = !on;
                    out = Some(if on { 1.0 } else { 0.0 });
                }
            } else {
                let mut v = value;
                let speed = match ch {
                    Channel::RotateX | Channel::RotateY | Channel::RotateZ => 0.5,
                    _ => 0.02,
                };
                let r = ui.add_sized([ui.available_width() - 6.0, 18.0], egui::DragValue::new(&mut v).speed(speed).max_decimals(3));
                if r.changed() {
                    out = Some(v);
                }
            }
        });
    });
    out
}

fn channel_menu(app: &mut App, ui: &mut egui::Ui, clicked: Channel) {
    let chans: Vec<Channel> = if app.selected_channels.contains(&clicked) { app.selected_channels.iter().copied().collect() } else { vec![clicked] };
    if ui.button("Key Selected").clicked() {
        app.key_selected(&chans);
    }
    if ui.button("Key All").clicked() {
        app.key_selected(&Channel::ALL);
    }
    if ui.button("Delete Selected (Break Connections)").clicked() {
        app.checkpoint();
        for id in app.sel.clone() {
            if let Some(n) = app.scene.node_mut(id) {
                for c in &chans {
                    n.anim.remove(c);
                }
            }
        }
    }
    if ui.button("Reset to Default").clicked() {
        app.checkpoint();
        for id in app.sel.clone() {
            if let Some(n) = app.scene.node_mut(id) {
                for c in &chans {
                    n.set(*c, c.default_value());
                }
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).strong().color(Color32::from_rgb(180, 180, 180)));
}

pub fn channel_box(app: &mut App, ui: &mut egui::Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Channels", |ui| {
            if ui.button("Key Selected").clicked() {
                let c: Vec<Channel> = app.selected_channels.iter().copied().collect();
                if c.is_empty() { app.key_selected(&Channel::ALL) } else { app.key_selected(&c) }
            }
            if ui.button("Key All          S").clicked() {
                app.key_selected(&Channel::ALL);
            }
            if ui.button("Delete Keys at Current Time").clicked() {
                app.delete_keys_selected(true);
            }
            if ui.button("Break All Connections").clicked() {
                app.delete_keys_selected(false);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.button("Select All Channels").clicked() {
                app.selected_channels = Channel::ALL.into_iter().collect();
            }
            if ui.button("Clear Channel Selection").clicked() {
                app.selected_channels.clear();
            }
        });
        ui.menu_button("Object", |ui| {
            if ui.button("Freeze Transformations").clicked() {
                app.freeze_transforms();
            }
            if ui.button("Reset Transformations").clicked() {
                app.reset_transforms();
            }
        });
        ui.menu_button("Show", |ui| ui.label("All attributes are shown"));
    });
    let Some(lead) = app.lead() else {
        ui.add_space(10.0);
        ui.label(RichText::new("Nothing selected").weak());
        return;
    };
    let node = app.scene.node(lead).unwrap().clone();
    // Name header (click to rename).
    let renaming = app.cb_rename.as_ref().is_some_and(|(id, _)| *id == lead);
    if renaming {
        let mut buf = app.cb_rename.as_ref().unwrap().1.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        app.cb_rename.as_mut().unwrap().1 = buf.clone();
        if r.lost_focus() {
            app.cb_rename = None;
            app.rename_node(lead, &buf);
        }
    } else {
        let r = ui.add(egui::Label::new(RichText::new(&node.name).strong().size(14.0)).sense(Sense::click()));
        if r.clicked() {
            app.cb_rename = Some((lead, node.name.clone()));
        }
        r.on_hover_text("Click to rename");
    }
    let t = app.scene.time;
    for ch in Channel::ALL {
        let curve = node.anim.get(&ch);
        let keyed = curve.is_some_and(|c| !c.keys.is_empty());
        let on_key = curve.is_some_and(|c| c.has_key_at(t));
        if let Some(v) = channel_row(app, ui, ch, node.get(ch), keyed, on_key) {
            app.edit_checkpoint(egui::Id::unique(("cb", ch)));
            let targets = app.sel.clone();
            for id in &targets {
                if let Some(n) = app.scene.node_mut(*id) {
                    n.set(ch, v);
                }
            }
            app.auto_key_touch(&targets);
        }
        if matches!(ch, Channel::TranslateZ | Channel::RotateZ | Channel::ScaleZ) {
            ui.add_space(3.0);
        }
    }
    if let Some(shape) = node.shape_name() {
        section(ui, "SHAPES");
        ui.label(RichText::new(shape).color(Color32::from_rgb(200, 200, 200)));
    }
    let has_inputs = node.history().is_some() || node.blend.is_some() || node.skin.is_some();
    if has_inputs {
        section(ui, "INPUTS");
    }
    if node.skin.is_some() {
        ui.label(RichText::new(format!("skinCluster_{}", node.name)).color(Color32::from_rgb(200, 200, 200)));
    }
    if let Some(b) = &node.blend {
        ui.label(RichText::new(format!("blendShape_{}", node.name)).color(Color32::from_rgb(200, 200, 200)));
        for (i, tg) in b.targets.iter().enumerate() {
            let mut w = b.weights.get(i).copied().unwrap_or(0.0);
            let keyed = b.curves.get(i).is_some_and(|c| c.as_ref().is_some_and(|c| !c.keys.is_empty()));
            let on_key = b.curves.get(i).is_some_and(|c| c.as_ref().is_some_and(|c| c.has_key_at(t)));
            ui.horizontal(|ui| {
                let col = if on_key { Color32::from_rgb(205, 39, 41) } else if keyed { Color32::from_rgb(221, 114, 122) } else { ui.visuals().text_color() };
                ui.add_sized([96.0, 18.0], egui::Label::new(RichText::new(&tg.name).size(12.5).color(col)).truncate());
                if ui.add(egui::DragValue::new(&mut w).speed(0.01).range(0.0..=1.0).max_decimals(3)).changed() {
                    app.edit_checkpoint(egui::Id::unique(("cbbs", lead, i)));
                    app.set_blend_weight(lead, i, w, true);
                }
            });
        }
    }
    if let Some(prim) = node.history().cloned() {
        let hist_name = format!("poly{}1", prim.base_name());
        ui.label(RichText::new(hist_name).color(Color32::from_rgb(200, 200, 200)));
        if let Some(newp) = primitive_editor(ui, &prim) {
            app.edit_checkpoint(egui::Id::unique(("inputs", lead)));
            if let Some(n) = app.scene.node_mut(lead) {
                n.set_history(newp);
            }
        }
    }
}

/// Inline editor for construction history parameters. Returns an edited copy.
pub fn primitive_editor(ui: &mut egui::Ui, prim: &Primitive) -> Option<Primitive> {
    let mut p = prim.clone();
    let mut changed = false;
    let mut fl = |ui: &mut egui::Ui, label: &str, v: &mut f32| {
        ui.horizontal(|ui| {
            ui.add_sized([96.0, 18.0], egui::Label::new(RichText::new(label).size(12.5)));
            changed |= ui.add(egui::DragValue::new(v).speed(0.02).range(0.001..=10_000.0).max_decimals(3)).changed();
        });
    };
    let mut ints: Vec<(&str, &mut u32, u32)> = Vec::new();
    match &mut p {
        Primitive::Cube { width, height, depth, sx, sy, sz } => {
            fl(ui, "Width", width);
            fl(ui, "Height", height);
            fl(ui, "Depth", depth);
            ints.extend([("Subdivisions Width", sx, 1), ("Subdivisions Height", sy, 1), ("Subdivisions Depth", sz, 1)]);
        }
        Primitive::Sphere { radius, axis_div, height_div } => {
            fl(ui, "Radius", radius);
            ints.extend([("Subdivisions Axis", axis_div, 3), ("Subdivisions Height", height_div, 2)]);
        }
        Primitive::Cylinder { radius, height, axis_div, height_div, caps: _ } => {
            fl(ui, "Radius", radius);
            fl(ui, "Height", height);
            ints.extend([("Subdivisions Axis", axis_div, 3), ("Subdivisions Height", height_div, 1)]);
        }
        Primitive::Cone { radius, height, axis_div } => {
            fl(ui, "Radius", radius);
            fl(ui, "Height", height);
            ints.push(("Subdivisions Axis", axis_div, 3));
        }
        Primitive::Plane { width, height, sx, sy } => {
            fl(ui, "Width", width);
            fl(ui, "Height", height);
            ints.extend([("Subdivisions Width", sx, 1), ("Subdivisions Height", sy, 1)]);
        }
        Primitive::Torus { radius, section_radius, axis_div, height_div } => {
            fl(ui, "Radius", radius);
            fl(ui, "Section Radius", section_radius);
            ints.extend([("Subdivisions Axis", axis_div, 3), ("Subdivisions Height", height_div, 3)]);
        }
    }
    for (label, v, min) in ints {
        ui.horizontal(|ui| {
            ui.add_sized([96.0, 18.0], egui::Label::new(RichText::new(label).size(12.5)).truncate());
            changed |= ui.add(egui::DragValue::new(v).speed(0.1).range(min..=200)).changed();
        });
    }
    changed.then_some(p)
}

// ------------------------------------------------------------ layer editor

fn layer_editor(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let _ = ui.selectable_label(true, "Display");
        ui.add_enabled(false, egui::Button::selectable(false, "Anim"));
    });
    ui.horizontal(|ui| {
        ui.menu_button("Layers", |ui| {
            if ui.button("Create Empty Layer").clicked() {
                app.new_layer(false);
            }
            if ui.button("Create Layer from Selected").clicked() {
                app.new_layer(true);
            }
        });
        ui.menu_button("Options", |ui| ui.label("No options"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("⊕").on_hover_text("Create a new layer and assign selected objects").clicked() {
                app.new_layer(true);
            }
            if ui.small_button("+").on_hover_text("Create a new empty layer").clicked() {
                app.new_layer(false);
            }
        });
    });
    let layers = app.scene.layers.clone();
    let mut remove = None;
    for l in layers.iter().rev() {
        ui.horizontal(|ui| {
            let mut vis = l.visible;
            if ui.add_sized([22.0, 18.0], egui::Button::selectable(vis, "V")).on_hover_text("Visibility").clicked() {
                vis = !vis;
                app.checkpoint();
                if let Some(x) = app.scene.layers.iter_mut().find(|x| x.id == l.id) {
                    x.visible = vis;
                }
            }
            if ui.add_sized([22.0, 18.0], egui::Button::selectable(l.template, "T")).on_hover_text("Template (unselectable, grey wireframe)").clicked() {
                app.checkpoint();
                if let Some(x) = app.scene.layers.iter_mut().find(|x| x.id == l.id) {
                    x.template = !x.template;
                }
            }
            let c = Color32::from_rgb((l.color[0] * 255.0) as u8, (l.color[1] * 255.0) as u8, (l.color[2] * 255.0) as u8);
            let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, c);
            let count = app.scene.nodes.iter().filter(|n| n.layer == Some(l.id)).count();
            let resp = ui.add(egui::Label::new(format!("{}  ({count})", l.name)).sense(Sense::click()));
            resp.context_menu(|ui| {
                if ui.button("Add Selected Objects").clicked() {
                    app.checkpoint();
                    for id in app.sel.clone() {
                        if let Some(n) = app.scene.node_mut(id) {
                            n.layer = Some(l.id);
                        }
                    }
                }
                if ui.button("Remove Selected Objects").clicked() {
                    app.checkpoint();
                    for id in app.sel.clone() {
                        if let Some(n) = app.scene.node_mut(id)
                            && n.layer == Some(l.id) {
                                n.layer = None;
                            }
                    }
                }
                if ui.button("Select Objects").clicked() {
                    app.sel = app.scene.nodes.iter().filter(|n| n.layer == Some(l.id)).map(|n| n.id).collect();
                }
                if ui.button("Delete Layer").clicked() {
                    remove = Some(l.id);
                }
            });
            if resp.double_clicked() {
                app.sel = app.scene.nodes.iter().filter(|n| n.layer == Some(l.id)).map(|n| n.id).collect();
            }
        });
    }
    if let Some(id) = remove {
        app.checkpoint();
        app.scene.layers.retain(|l| l.id != id);
        for n in &mut app.scene.nodes {
            if n.layer == Some(id) {
                n.layer = None;
            }
        }
    }
    if layers.is_empty() {
        ui.label(RichText::new("No display layers").weak());
    }
}

// --------------------------------------------------------- attribute editor

fn vec3_row(ui: &mut egui::Ui, label: &str, v: &mut glam::Vec3, speed: f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new(label));
        for i in 0..3 {
            changed |= ui.add(egui::DragValue::new(&mut v[i]).speed(speed).max_decimals(3)).changed();
        }
    });
    changed
}

pub fn attribute_editor(app: &mut App, ui: &mut egui::Ui) {
    let Some(lead) = app.lead() else {
        ui.label(RichText::new("Select an object to edit its attributes.").weak());
        return;
    };
    let mut node = app.scene.node(lead).unwrap().clone();
    ui.horizontal_wrapped(|ui| {
        let _ = ui.selectable_label(true, &node.name);
        if let Some(s) = node.shape_name() {
            ui.label(RichText::new(s).weak());
        }
        if let Some(p) = node.history() {
            ui.label(RichText::new(format!("poly{}1", p.base_name())).weak());
        }
        if node.mesh().is_some() {
            let mn = node.material.and_then(|m| app.material(m)).map(|m| m.name.clone()).unwrap_or("lambert1".into());
            ui.label(RichText::new(mn).weak());
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("transform:");
        ui.label(RichText::new(&node.name).strong());
    });
    let mut changed = false;
    egui::CollapsingHeader::new("Transform Attributes").default_open(true).show(ui, |ui| {
        changed |= vec3_row(ui, "Translate", &mut node.translate, 0.02);
        changed |= vec3_row(ui, "Rotate", &mut node.rotate, 0.5);
        changed |= vec3_row(ui, "Scale", &mut node.scale, 0.02);
        ui.horizontal(|ui| {
            ui.add_sized([70.0, 18.0], egui::Label::new("Rotate Order"));
            ui.label("xyz");
        });
    });
    egui::CollapsingHeader::new("Display").default_open(true).show(ui, |ui| {
        changed |= ui.checkbox(&mut node.visible, "Visibility").changed();
        let layer_name = node.layer.and_then(|l| app.scene.layers.iter().find(|x| x.id == l)).map(|l| l.name.clone()).unwrap_or("defaultLayer".into());
        ui.label(format!("Display layer: {layer_name}"));
    });
    if changed {
        app.edit_checkpoint(egui::Id::unique(("ae_xform", lead)));
        if let Some(n) = app.scene.node_mut(lead) {
            n.translate = node.translate;
            n.rotate = node.rotate;
            n.scale = node.scale;
            n.visible = node.visible;
        }
        app.auto_key_touch(&[lead]);
    }
    if let Some(m) = node.mesh() {
        egui::CollapsingHeader::new(format!("Shape: {}", node.shape_name().unwrap_or_default())).default_open(true).show(ui, |ui| {
            egui::Grid::new("mesh_stats").num_columns(2).show(ui, |ui| {
                ui.label("Vertices");
                ui.label(m.vertex_count().to_string());
                ui.end_row();
                ui.label("Edges");
                ui.label(m.edges().len().to_string());
                ui.end_row();
                ui.label("Faces");
                ui.label(m.face_count().to_string());
                ui.end_row();
                ui.label("Triangles");
                ui.label(m.tri_count().to_string());
                ui.end_row();
                ui.label("History");
                ui.label(if node.history().is_some() || node.surf.is_some() { "live" } else { "deleted" });
                ui.end_row();
            });
        });
    }
    if let Some(p) = node.history().cloned() {
        egui::CollapsingHeader::new(format!("poly{}1 (construction history)", p.base_name())).default_open(true).show(ui, |ui| {
            if let Some(np) = primitive_editor(ui, &p) {
                app.edit_checkpoint(egui::Id::unique(("ae_inputs", lead)));
                if let Some(n) = app.scene.node_mut(lead) {
                    n.set_history(np);
                }
            }
        });
    }
    if let Shape::Lattice { div, .. } = &node.shape {
        let div = *div;
        egui::CollapsingHeader::new("ffdLattice").default_open(true).show(ui, |ui| {
            ui.label(format!("Divisions: {} × {} × {}", div[0], div[1], div[2]));
            let bound = app.scene.nodes.iter().filter(|n| n.lattices.iter().any(|b| b.lattice == lead)).map(|n| n.name.clone()).collect::<Vec<_>>();
            ui.label(format!("Deforms: {}", if bound.is_empty() { "—".into() } else { bound.join(", ") }));
            ui.label(RichText::new("Vertex mode (F9) edits lattice points; move the lattice to move the shape.").weak());
            if ui.button("Reset Lattice").clicked() {
                app.reset_lattice(lead);
            }
        });
    }
    if let Shape::Particles(ps) = &node.shape {
        let mut st = ps.settings.clone();
        let live = ps.current().len();
        let mut changed = false;
        egui::CollapsingHeader::new("nParticle").default_open(true).show(ui, |ui| {
            let src = st.source.and_then(|i| app.scene.node(i)).map(|n| n.name.clone());
            ui.label(format!("{} emitter{} · {live} live particles", st.kind.name(), src.map(|s| format!(" from {s}")).unwrap_or_default()));
            let mut row = |ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, lo: f32, hi: f32| {
                ui.horizontal(|ui| {
                    ui.label(label);
                    changed |= ui.add(egui::DragValue::new(v).speed(speed).range(lo..=hi)).changed();
                });
            };
            row(ui, "Rate (/s)", &mut st.rate, 1.0, 0.0, 100_000.0);
            row(ui, "Speed", &mut st.speed, 0.05, -1000.0, 1000.0);
            row(ui, "Speed random", &mut st.speed_random, 0.01, 0.0, 1.0);
            row(ui, "Spread (°)", &mut st.spread, 0.5, 0.0, 180.0);
            row(ui, "Lifespan (s, 0 = ∞)", &mut st.lifespan, 0.05, 0.0, 1000.0);
            row(ui, "Lifespan random", &mut st.lifespan_random, 0.01, 0.0, 1.0);
            row(ui, "Radius", &mut st.radius, 0.002, 0.0005, 10.0);
            row(ui, "Drag", &mut st.drag, 0.01, 0.0, 100.0);
            row(ui, "Turbulence", &mut st.turbulence, 0.05, 0.0, 1000.0);
            row(ui, "Turbulence freq.", &mut st.turbulence_freq, 0.05, 0.0, 100.0);
            row(ui, "Bounce", &mut st.bounce, 0.01, 0.0, 1.0);
            row(ui, "Friction", &mut st.friction, 0.01, 0.0, 1.0);
            row(ui, "Start frame", &mut st.start, 1.0, -100_000.0, 100_000.0);
            ui.horizontal(|ui| {
                ui.label("Gravity");
                for k in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut st.gravity[k]).speed(0.1)).changed();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Wind");
                for k in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut st.wind[k]).speed(0.1)).changed();
                }
            });
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut st.ground, "Ground plane").changed();
                changed |= ui.add(egui::DragValue::new(&mut st.ground_height).speed(0.05)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Colour start / end");
                changed |= ui.color_edit_button_rgb(&mut st.color).changed();
                changed |= ui.color_edit_button_rgb(&mut st.color_end).changed();
            });
            let cols: Vec<String> = st.colliders.iter().filter_map(|i| app.scene.node(*i)).map(|n| n.name.clone()).collect();
            ui.label(format!("Colliders: {}", if cols.is_empty() { "—".into() } else { cols.join(", ") }));
        });
        if changed {
            app.edit_checkpoint(egui::Id::unique(("ae_particles", lead)));
            if let Some(Shape::Particles(p)) = app.scene.node_mut(lead).map(|n| &mut n.shape) {
                p.settings = st;
            }
        }
    }
    if let Shape::Field(f) = &node.shape {
        let mut st = f.clone();
        let mut changed = false;
        egui::CollapsingHeader::new("Field").default_open(true).show(ui, |ui| {
            egui::ComboBox::from_label("Type").selected_text(st.kind.name()).show_ui(ui, |ui| {
                for k in crate::fx::FieldKind::ALL {
                    if ui.selectable_label(st.kind == k, k.name()).clicked() && st.kind != k {
                        let keep = st.targets.clone();
                        st = crate::fx::Field::new(k);
                        st.targets = keep;
                        changed = true;
                    }
                }
            });
            changed |= ae_row(ui, "Magnitude", &mut st.magnitude, 0.05, -10_000.0, 10_000.0);
            changed |= ae_row(ui, "Attenuation", &mut st.attenuation, 0.05, 0.0, 10.0);
            changed |= ae_row(ui, "Max distance (0 = ∞)", &mut st.max_distance, 0.05, 0.0, 100_000.0);
            match st.kind {
                crate::fx::FieldKind::Turbulence => {
                    changed |= ae_row(ui, "Frequency", &mut st.frequency, 0.05, 0.0, 100.0);
                    changed |= ae_row(ui, "Phase speed", &mut st.speed, 0.05, 0.0, 100.0);
                }
                crate::fx::FieldKind::Air => changed |= ae_row(ui, "Speed (how fast things reach the wind)", &mut st.speed, 0.05, 0.0, 100.0),
                _ => {}
            }
            if !matches!(st.kind, crate::fx::FieldKind::Radial | crate::fx::FieldKind::Newton | crate::fx::FieldKind::Turbulence | crate::fx::FieldKind::Drag) {
                ui.horizontal(|ui| {
                    ui.label(if st.kind == crate::fx::FieldKind::Vortex { "Axis" } else { "Direction" });
                    for k in 0..3 {
                        changed |= ui.add(egui::DragValue::new(&mut st.direction[k]).speed(0.05)).changed();
                    }
                });
            }
            let names: Vec<String> = st.targets.iter().filter_map(|i| app.scene.node(*i)).map(|n| n.name.clone()).collect();
            ui.label(format!("Acts on: {}", if names.is_empty() { "every simulation".into() } else { names.join(", ") }));
            if !st.targets.is_empty() && ui.button("Act on every simulation").clicked() {
                st.targets.clear();
                changed = true;
            }
        });
        if changed {
            app.edit_checkpoint(egui::Id::unique(("ae_field", lead)));
            if let Some(Shape::Field(f)) = app.scene.node_mut(lead).map(|n| &mut n.shape) {
                *f = st;
            }
        }
    }
    if let Shape::Hair(h) = &node.shape {
        let mut st = h.settings.clone();
        let strands = h.strands().len();
        let mut changed = false;
        egui::CollapsingHeader::new("Hair").default_open(true).show(ui, |ui| {
            let src = st.source.and_then(|i| app.scene.node(i)).map(|n| n.name.clone()).unwrap_or_else(|| "—".into());
            let dynamic_curve = st.output.is_some();
            ui.label(if dynamic_curve { format!("Dynamic curve from {src}") } else { format!("{strands} strands on {src}") });
            if !dynamic_curve {
                ui.horizontal(|ui| {
                    ui.label("Count");
                    changed |= ui.add(egui::DragValue::new(&mut st.count).range(1..=200_000)).changed();
                });
                changed |= ae_row(ui, "Length", &mut st.length, 0.005, 0.0001, 1000.0);
                changed |= ae_row(ui, "Length random", &mut st.length_random, 0.01, 0.0, 1.0);
                changed |= ae_row(ui, "Tilt (°)", &mut st.tilt, 0.5, 0.0, 90.0);
                changed |= ae_row(ui, "Curl (°)", &mut st.curl, 0.5, 0.0, 720.0);
                changed |= ae_row(ui, "Comb amount", &mut st.comb_amount, 0.01, 0.0, 1.0);
                ui.horizontal(|ui| {
                    ui.label("Comb direction");
                    for k in 0..3 {
                        changed |= ui.add(egui::DragValue::new(&mut st.comb[k]).speed(0.05)).changed();
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("Segments");
                changed |= ui.add(egui::DragValue::new(&mut st.segments).range(1..=64)).changed();
            });
            changed |= ae_row(ui, "Stiffness (root)", &mut st.stiffness, 0.005, 0.0, 1.0);
            changed |= ae_row(ui, "Stiffness (tip)", &mut st.tip_stiffness, 0.005, 0.0, 1.0);
            changed |= ae_row(ui, "Damping", &mut st.damping, 0.005, 0.0, 1.0);
            changed |= ae_row(ui, "Air drag", &mut st.drag, 0.05, 0.0, 100.0);
            changed |= ae_row(ui, "Collision thickness", &mut st.thickness, 0.001, 0.0, 10.0);
            changed |= ae_row(ui, "Width (root)", &mut st.width, 0.0005, 0.00001, 10.0);
            changed |= ae_row(ui, "Width (tip)", &mut st.tip_width, 0.0005, 0.0, 10.0);
            changed |= ae_row(ui, "Start frame", &mut st.start, 1.0, -100_000.0, 100_000.0);
            ui.horizontal(|ui| {
                ui.label("Substeps");
                changed |= ui.add(egui::DragValue::new(&mut st.substeps).range(1..=32)).changed();
                ui.label("Iterations");
                changed |= ui.add(egui::DragValue::new(&mut st.iterations).range(1..=32)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Gravity");
                for k in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut st.gravity[k]).speed(0.1)).changed();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Wind");
                for k in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut st.wind[k]).speed(0.1)).changed();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Colour root / tip");
                changed |= ui.color_edit_button_rgb(&mut st.color).changed();
                changed |= ui.color_edit_button_rgb(&mut st.tip_color).changed();
            });
            ui.horizontal(|ui| {
                if dynamic_curve {
                    changed |= ui.checkbox(&mut st.pin_tip, "Pin the tip too").changed();
                } else {
                    changed |= ui.checkbox(&mut st.collide_source, "Collide with the mesh").changed();
                }
                changed |= ui.checkbox(&mut st.ground, "Ground plane").changed();
            });
            let cols: Vec<String> = st.colliders.iter().filter_map(|i| app.scene.node(*i)).map(|n| n.name.clone()).collect();
            ui.label(format!("Colliders: {}", if cols.is_empty() { "—".into() } else { cols.join(", ") }));
        });
        if changed {
            app.edit_checkpoint(egui::Id::unique(("ae_hair", lead)));
            if let Some(Shape::Hair(h)) = app.scene.node_mut(lead).map(|n| &mut n.shape) {
                h.settings = st;
            }
        }
    }
    if let Shape::Fluid(fl) = &node.shape {
        let mut st = fl.settings.clone();
        let (dims, _) = st.dims();
        let stats = fl.current().map(|c| (c.dmax, c.tmax));
        let mut changed = false;
        let mut sim_changed = false;
        egui::CollapsingHeader::new("Fluid (smoke / fire)").default_open(true).show(ui, |ui| {
            ui.label(format!("{} × {} × {} voxels{}", dims[0], dims[1], dims[2], stats.map(|(d, t)| format!("  ·  max density {d:.2}, temperature {t:.2}")).unwrap_or_default()));
            ui.horizontal(|ui| {
                ui.label("Resolution");
                sim_changed |= ui.add(egui::DragValue::new(&mut st.res).range(4..=256)).on_hover_text("Voxels along the longest side").changed();
                ui.label("Size");
                for k in 0..3 {
                    sim_changed |= ui.add(egui::DragValue::new(&mut st.size[k]).speed(0.05).range(0.01..=1000.0)).changed();
                }
            });
            sim_changed |= ae_row(ui, "Buoyancy", &mut st.buoyancy, 0.05, -100.0, 100.0);
            sim_changed |= ae_row(ui, "Weight", &mut st.weight, 0.01, -10.0, 10.0);
            sim_changed |= ae_row(ui, "Cooling", &mut st.cooling, 0.01, 0.0, 20.0);
            sim_changed |= ae_row(ui, "Dissipation", &mut st.dissipation, 0.01, 0.0, 20.0);
            sim_changed |= ae_row(ui, "Vorticity", &mut st.vorticity, 0.05, 0.0, 50.0);
            sim_changed |= ae_row(ui, "Turbulence", &mut st.turbulence, 0.05, 0.0, 100.0);
            sim_changed |= ae_row(ui, "Turbulence frequency", &mut st.turbulence_freq, 0.05, 0.0, 50.0);
            sim_changed |= ae_row(ui, "Drag", &mut st.drag, 0.01, 0.0, 10.0);
            sim_changed |= ae_row(ui, "Start frame", &mut st.start, 1.0, -100_000.0, 100_000.0);
            ui.horizontal(|ui| {
                ui.label("Substeps");
                sim_changed |= ui.add(egui::DragValue::new(&mut st.substeps).range(1..=16)).changed();
                sim_changed |= ui.checkbox(&mut st.open, "Open boundaries").on_hover_text("Smoke leaves through the sides and top; the bottom is always closed").changed();
            });
            ui.separator();
            ui.label("Sources");
            let mut remove = None;
            let n_src = st.sources.len();
            for (k, src) in st.sources.iter_mut().enumerate() {
                let nm = app.scene.node(src.node).map(|n| n.name.clone()).unwrap_or_else(|| "?".into());
                egui::CollapsingHeader::new(nm.clone()).id_salt(("fluid_src", lead, k)).default_open(n_src <= 2).show(ui, |ui| {
                    sim_changed |= ae_row(ui, "Density / s", &mut src.density, 0.05, 0.0, 10_000.0);
                    sim_changed |= ae_row(ui, "Heat / s", &mut src.heat, 0.05, 0.0, 10_000.0);
                    sim_changed |= ae_row(ui, "Radius", &mut src.radius, 0.01, 0.001, 1000.0);
                    sim_changed |= ae_row(ui, "Speed", &mut src.speed, 0.05, -1000.0, 1000.0);
                    sim_changed |= ae_row(ui, "Inherit motion", &mut src.inherit, 0.01, 0.0, 1.0);
                    sim_changed |= ae_row(ui, "Duration (s, 0 = always)", &mut src.duration, 0.01, 0.0, 10_000.0);
                    sim_changed |= ae_row(ui, "Expansion", &mut src.expansion, 0.05, 0.0, 1000.0);
                    if ui.small_button("Remove source").clicked() {
                        remove = Some(k);
                    }
                });
            }
            if let Some(k) = remove {
                st.sources.remove(k);
                sim_changed = true;
            }
            let cols: Vec<String> = st.colliders.iter().filter_map(|i| app.scene.node(*i)).map(|n| n.name.clone()).collect();
            ui.label(format!("Colliders: {}", if cols.is_empty() { "—".into() } else { cols.join(", ") }));
            ui.separator();
            ui.label("Look (renders without re-simulating)");
            ui.horizontal(|ui| {
                ui.label("Smoke colour");
                changed |= ui.color_edit_button_rgb(&mut st.smoke_color).changed();
            });
            changed |= ae_row(ui, "Density scale", &mut st.density_scale, 0.05, 0.0, 1000.0);
            changed |= ae_row(ui, "Anisotropy", &mut st.anisotropy, 0.01, -0.95, 0.95);
            changed |= ae_row(ui, "Fire intensity", &mut st.fire_intensity, 0.05, 0.0, 1000.0);
            ui.horizontal(|ui| {
                ui.label("Fire colour");
                changed |= ui.color_edit_button_rgb(&mut st.fire_color).changed();
            });
            changed |= ae_row(ui, "Fire starts at temperature", &mut st.fire_temp, 0.01, 0.0, 100.0);
        });
        if changed || sim_changed {
            app.edit_checkpoint(egui::Id::unique(("ae_fluid", lead)));
            if let Some(Shape::Fluid(f)) = app.scene.node_mut(lead).map(|n| &mut n.shape) {
                f.settings = st;
            }
        }
    }
    if let Some(cl) = node.cloth.clone() {
        let mut st = cl.settings.clone();
        let mut changed = false;
        egui::CollapsingHeader::new("nCloth").default_open(true).show(ui, |ui| {
            let mut row = |ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, lo: f32, hi: f32| {
                ui.horizontal(|ui| {
                    ui.label(label);
                    changed |= ui.add(egui::DragValue::new(v).speed(speed).range(lo..=hi)).changed();
                });
            };
            row(ui, "Stretch resistance", &mut st.stretch, 0.01, 0.0, 1.0);
            row(ui, "Bend resistance", &mut st.bend, 0.01, 0.0, 1.0);
            row(ui, "Damping", &mut st.damping, 0.005, 0.0, 1.0);
            row(ui, "Air drag", &mut st.drag, 0.05, 0.0, 100.0);
            row(ui, "Thickness", &mut st.thickness, 0.001, 0.0, 10.0);
            row(ui, "Friction", &mut st.friction, 0.01, 0.0, 1.0);
            row(ui, "Start frame", &mut st.start, 1.0, -100_000.0, 100_000.0);
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut st.self_collide, "Self collision").changed();
                ui.label("radius (0 = auto)");
                changed |= ui.add(egui::DragValue::new(&mut st.self_thickness).speed(0.001).range(0.0..=10.0)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Substeps");
                changed |= ui.add(egui::DragValue::new(&mut st.substeps).range(1..=64)).changed();
                ui.label("Iterations");
                changed |= ui.add(egui::DragValue::new(&mut st.iterations).range(1..=32)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Gravity");
                for k in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut st.gravity[k]).speed(0.1)).changed();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Wind");
                for k in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut st.wind[k]).speed(0.1)).changed();
                }
            });
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut st.ground, "Ground plane").changed();
                changed |= ui.add(egui::DragValue::new(&mut st.ground_height).speed(0.05)).changed();
            });
            ui.label(format!("{} pinned vertices", st.pins.len()));
            let current = st.pin_to.and_then(|i| app.scene.node(i)).map(|n| n.name.clone()).unwrap_or_else(|| "(its own transform)".into());
            let candidates: Vec<(NodeId, String)> = app.scene.nodes.iter().filter(|n| n.id != lead && !matches!(n.shape, Shape::Poly { .. })).map(|n| (n.id, n.name.clone())).collect();
            egui::ComboBox::from_label("Pins follow").selected_text(current).show_ui(ui, |ui| {
                if ui.selectable_label(st.pin_to.is_none(), "(its own transform)").clicked() {
                    st.pin_to = None;
                    changed = true;
                }
                for (id, n) in &candidates {
                    if ui.selectable_label(st.pin_to == Some(*id), n).clicked() {
                        st.pin_to = Some(*id);
                        changed = true;
                    }
                }
            });
            let cols: Vec<String> = st.colliders.iter().filter_map(|i| app.scene.node(*i)).map(|n| n.name.clone()).collect();
            ui.label(format!("Colliders: {}", if cols.is_empty() { "—".into() } else { cols.join(", ") }));
            if ui.button("Remove nCloth").clicked() {
                app.remove_cloth(&[lead]);
            }
        });
        if changed {
            app.edit_checkpoint(egui::Id::unique(("ae_cloth", lead)));
            if let Some(c) = app.scene.node_mut(lead).and_then(|n| n.cloth.as_mut()) {
                c.settings = st;
            }
        }
    }
    if let Some(c) = crate::nurbs::Curve::from_shape(&node.shape) {
        egui::CollapsingHeader::new("nurbsCurve").default_open(true).show(ui, |ui| {
            let form = if c.closed { "periodic" } else { "open" };
            ui.label(format!("Degree {} · {} CVs · {} spans · {form}", c.degree, c.cvs.len(), c.spans()));
            if let Some(w) = crate::nurbs::world_curve(&app.scene, lead) {
                let (a, b) = w.domain();
                ui.label(format!("Length {:.3} · parameter range {a:.2} … {b:.2}", w.length()));
            }
            let users: Vec<String> = app.scene.nodes.iter().filter(|n| n.surf.as_ref().is_some_and(|h| h.inputs.contains(&lead)) || n.wires.iter().any(|w| w.curve == lead)).map(|n| n.name.clone()).collect();
            if !users.is_empty() {
                ui.label(format!("Drives: {}", users.join(", ")));
            }
            ui.horizontal(|ui| {
                if ui.button("Rebuild").clicked() {
                    let (sp, d) = (app.curve_opts.rebuild_spans as usize, app.curve_opts.rebuild_degree as usize);
                    let r = app.rebuild_curves(&[lead], sp, d).map(|_| String::new());
                    super::report(app, r);
                }
                ui.add(egui::DragValue::new(&mut app.curve_opts.rebuild_spans).range(1..=256).suffix(" spans"));
                if ui.button("Reverse").clicked() {
                    let _ = app.reverse_curves(&[lead]);
                }
                if ui.button(if c.closed { "Open" } else { "Close" }).clicked() {
                    let _ = app.open_close_curves(&[lead]);
                }
            });
            ui.label(RichText::new("Vertex mode (F9) edits CVs.").weak());
        });
    }
    if let Some(h) = node.surf.clone() {
        egui::CollapsingHeader::new(h.op_name()).default_open(true).show(ui, |ui| {
            let ins: Vec<String> = h.inputs.iter().filter_map(|i| app.scene.node(*i)).map(|n| n.name.clone()).collect();
            ui.label(format!("Inputs: {}", ins.join(", ")));
            let mut op = h.op.clone();
            let mut samples = h.samples;
            let mut changed = false;
            match &mut op {
                crate::nurbs::SurfaceOp::Revolve { sections, sweep, .. } => {
                    ui.horizontal(|ui| {
                        ui.label("Sections");
                        changed |= ui.add(egui::DragValue::new(sections).range(3..=256)).changed();
                        ui.label("Sweep");
                        changed |= ui.add(egui::DragValue::new(sweep).range(-360.0..=360.0).suffix("°")).changed();
                    });
                }
                crate::nurbs::SurfaceOp::Loft { sections, close } => {
                    ui.horizontal(|ui| {
                        ui.label("Section spans");
                        changed |= ui.add(egui::DragValue::new(sections).range(1..=64)).changed();
                        changed |= ui.checkbox(close, "Close").changed();
                    });
                }
                crate::nurbs::SurfaceOp::Extrude { length, .. } => {
                    if h.inputs.len() < 2 {
                        ui.horizontal(|ui| {
                            ui.label("Length");
                            changed |= ui.add(egui::DragValue::new(length).speed(0.05)).changed();
                        });
                    }
                }
                crate::nurbs::SurfaceOp::Planar => {}
            }
            ui.horizontal(|ui| {
                ui.label("Samples per curve");
                changed |= ui.add(egui::DragValue::new(&mut samples).range(3..=512)).changed();
            });
            if changed {
                app.edit_checkpoint(egui::Id::unique(("ae_surf", lead)));
                if let Some(s) = app.scene.node_mut(lead).and_then(|n| n.surf.as_mut()) {
                    s.op = op;
                    s.samples = samples;
                    s.key = 0;
                }
            }
            if ui.button("Delete History").clicked() {
                app.checkpoint();
                if let Some(n) = app.scene.node_mut(lead) {
                    n.surf = None;
                }
            }
            ui.label(RichText::new("Editing the input curves rebuilds this mesh.").weak());
        });
    }
    if !node.lattices.is_empty() || node.wrap.is_some() || !node.wires.is_empty() {
        egui::CollapsingHeader::new("Deformers").default_open(true).show(ui, |ui| {
            for b in &node.lattices {
                ui.label(format!("ffd → {}", app.scene.node(b.lattice).map(|n| n.name.as_str()).unwrap_or("?")));
            }
            if let Some(w) = &node.wrap {
                ui.label(format!("wrap → {}", app.scene.node(w.driver).map(|n| n.name.as_str()).unwrap_or("?")));
            }
            for (k, w) in node.wires.iter().enumerate() {
                let cname = app.scene.node(w.curve).map(|n| n.name.clone()).unwrap_or("?".into());
                ui.label(format!("wire → {cname} ({} vertices, dropoff {:.2})", w.binds.len(), w.dropoff));
                let (mut en, mut rot) = (w.envelope, w.rotation);
                let (c1, c2) = ui.horizontal(|ui| {
                    ui.label("Envelope");
                    let c1 = ui.add(egui::Slider::new(&mut en, 0.0..=1.0)).changed();
                    ui.label("Rotation");
                    let c2 = ui.add(egui::Slider::new(&mut rot, 0.0..=1.0)).changed();
                    (c1, c2)
                }).inner;
                if c1 || c2 {
                    app.edit_checkpoint(egui::Id::unique(("ae_wire", lead, k)));
                    if let Some(x) = app.scene.node_mut(lead).and_then(|n| n.wires.get_mut(k)) {
                        x.envelope = en;
                        x.rotation = rot;
                    }
                }
            }
            if ui.button("Remove Lattice / Wrap / Wire").clicked() {
                app.remove_lattice_and_wrap(&[lead]);
            }
        });
    }
    super::lookdev::attribute_sections(app, ui, lead);
    super::rigging::attribute_sections(app, ui, lead);
    egui::CollapsingHeader::new("Animation").default_open(true).show(ui, |ui| {
        if node.anim.is_empty() {
            ui.label(RichText::new("No animation curves. Press S to key all channels.").weak());
        }
        let curves: Vec<(Channel, usize, Tangent)> = node.anim.iter().map(|(c, cv)| (*c, cv.keys.len(), cv.tangent)).collect();
        for (ch, count, tangent) in curves {
            ui.horizontal(|ui| {
                ui.label(format!("{}_{}", node.name, ch.long()));
                ui.label(RichText::new(format!("{count} keys")).weak());
                let mut t = tangent;
                egui::ComboBox::from_id_salt(("tan", lead, ch)).selected_text(format!("{t:?}")).width(70.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut t, Tangent::Auto, "Auto");
                    ui.selectable_value(&mut t, Tangent::Linear, "Linear");
                    ui.selectable_value(&mut t, Tangent::Stepped, "Stepped");
                });
                if t != tangent {
                    app.checkpoint();
                    if let Some(c) = app.scene.node_mut(lead).and_then(|n| n.anim.get_mut(&ch)) {
                        c.tangent = t;
                    }
                    app.scene.evaluate();
                }
            });
        }
    });
    node.expanded = true;
}

// ------------------------------------------------------------ tool settings

fn tool_settings(app: &mut App, ui: &mut egui::Ui) {
    let name = app.tool.name();
    ui.heading(name);
    ui.separator();
    super::rigging::tool_settings(app, ui);
    if app.tool == Tool::Sculpt {
        egui::CollapsingHeader::new("Sculpt Brush").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for b in crate::sculpt::Brush::ALL {
                    ui.selectable_value(&mut app.sculpt.brush, b, b.name());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Size (px)");
                ui.add(egui::Slider::new(&mut app.sculpt.radius_px, 2.0..=400.0));
            });
            ui.horizontal(|ui| {
                ui.label("Strength");
                ui.add(egui::Slider::new(&mut app.sculpt.strength, 0.0..=1.0));
            });
            ui.checkbox(&mut app.sculpt.symmetry_x, "Symmetry X");
            ui.checkbox(&mut app.sculpt.invert, "Invert (or hold Ctrl)");
        });
    }
    if app.tool == Tool::QuadDraw {
        egui::CollapsingHeader::new("Quad Draw").default_open(true).show(ui, |ui| {
            let live = app.qd.live.and_then(|l| app.scene.node(l)).map(|n| n.name.clone()).unwrap_or_else(|| "none".into());
            ui.horizontal(|ui| {
                ui.label(format!("Live object: {live}"));
                if ui.button("Make selected live").clicked() {
                    let id = app.lead().filter(|i| Some(*i) != app.qd.target);
                    let r = app.qd_make_live(id).map(|_| String::new());
                    super::report(app, r);
                }
            });
            let (nv, nf) = app.qd.target.and_then(|t| app.scene.node(t)).and_then(|n| n.mesh()).map(|m| (m.positions.len(), m.faces.len())).unwrap_or((0, 0));
            ui.label(format!("{nv} dots, {nf} faces"));
            ui.horizontal(|ui| {
                ui.label("Surface offset");
                ui.add(egui::DragValue::new(&mut app.qd.offset).speed(0.0002).range(0.0..=0.05));
            });
            ui.horizontal(|ui| {
                if ui.button("Relax").clicked() {
                    let n = app.qd.relax_iters;
                    let r = app.qd_relax(n).map(|n| format!("Relaxed {n} dot(s)"));
                    super::report(app, r);
                }
                ui.add(egui::DragValue::new(&mut app.qd.relax_iters).range(1..=100).suffix(" iterations"));
            });
            ui.horizontal(|ui| {
                if ui.button("New mesh").clicked() {
                    app.qd.target = None;
                }
                if ui.button("Finish").clicked() {
                    app.qd_finish();
                    app.tool = Tool::Select;
                }
            });
            ui.label(RichText::new("Click: dot · drag dot: move · Shift+click: quad from the 4 nearest dots (Ctrl+Shift: triangle) · Ctrl+drag border edge: extend · Ctrl+click: delete dot/face · Enter or RMB: finish").weak());
        });
    }
    if matches!(app.tool, Tool::CurveCv | Tool::CurveEp) {
        egui::CollapsingHeader::new(if app.tool == Tool::CurveCv { "CV Curve" } else { "EP Curve" }).default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Degree");
                ui.radio_value(&mut app.curve_tool.degree, 1, "1 Linear");
                ui.radio_value(&mut app.curve_tool.degree, 3, "3 Cubic");
            });
            ui.label(format!("{} point(s)", app.curve_tool.points.len()));
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    let r = app.finish_curve_tool().map(|_| String::new());
                    super::report(app, r);
                }
                if ui.button("Clear").clicked() {
                    app.curve_tool.points.clear();
                }
            });
            ui.label(RichText::new("Points land on the live object (Modify › Make Live) or the grid; hold X to snap. Backspace removes the last point.").weak());
        });
    }
    if app.tool == Tool::MultiCut {
        egui::CollapsingHeader::new("Multi-Cut").default_open(true).show(ui, |ui| {
            ui.label(format!("{} cut point(s)", app.cut_points.len()));
            ui.label(RichText::new("LMB on edges adds points (near a vertex snaps to it). Enter or RMB cuts, Esc cancels.").weak());
            ui.horizontal(|ui| {
                if ui.button("Cut").clicked() {
                    let r = app.finish_multi_cut().map(|n| format!("{n} face(s) split"));
                    super::report(app, r);
                }
                if ui.button("Clear").clicked() {
                    app.cut_points.clear();
                }
            });
        });
    }
    egui::CollapsingHeader::new("Soft Selection").default_open(app.soft_select).show(ui, |ui| {
        ui.checkbox(&mut app.soft_select, "Soft Select (B)");
        ui.horizontal(|ui| {
            ui.label("Falloff radius");
            ui.add(egui::DragValue::new(&mut app.soft_radius).speed(0.02).range(0.01..=1000.0));
        });
        ui.label(RichText::new("Move/rotate/scale of components pulls nearby vertices (yellow → red = weight).").weak());
    });
    egui::CollapsingHeader::new("Modeling").default_open(matches!(app.tool, Tool::EdgeLoop) || app.comp_mode != CompMode::Object).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Bevel width");
            ui.add(egui::DragValue::new(&mut app.bevel_width).speed(0.005).range(0.001..=100.0));
        });
        ui.horizontal(|ui| {
            ui.label("Mirror axis");
            for (i, n) in ["X", "Y", "Z"].iter().enumerate() {
                ui.radio_value(&mut app.mirror_axis, i, *n);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Merge distance");
            ui.add(egui::DragValue::new(&mut app.merge_dist).speed(0.0005).range(0.0..=10.0));
        });
    });
    egui::CollapsingHeader::new("Common Selection Options").default_open(true).show(ui, |ui| {
        ui.label("Selection mode:");
        ui.horizontal(|ui| {
            for (m, n) in [(CompMode::Object, "Object"), (CompMode::Vertex, "Vertex"), (CompMode::Edge, "Edge"), (CompMode::Face, "Face")] {
                if ui.selectable_label(app.comp_mode == m, n).clicked() {
                    app.set_comp_mode(m);
                }
            }
        });
    });
    egui::CollapsingHeader::new("Step Snap").default_open(true).show(ui, |ui| {
        ui.checkbox(&mut app.snap_grid, "Snap to grid units (move) / 15° (rotate)");
        ui.label(RichText::new("Hold X while dragging to snap temporarily. Hold Shift for fine control.").weak());
    });
    egui::CollapsingHeader::new("Extrude").default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Offset");
            ui.add(egui::DragValue::new(&mut app.extrude_offset).speed(0.01).range(-100.0..=100.0));
        });
    });
    egui::CollapsingHeader::new("Axis Orientation").default_open(true).show(ui, |ui| {
        ui.label(match app.tool {
            Tool::Move => "Move: World",
            Tool::Rotate => "Rotate: Gimbal (object) / World (components)",
            Tool::Scale => "Scale: Object (object) / World (components)",
            _ => "—",
        });
    });
}

/// One labelled drag-value row; true when changed.
fn ae_row(ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, lo: f32, hi: f32) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).speed(speed).range(lo..=hi)).changed()
    })
    .inner
}
