//! UV Editor window: shows the selected meshes' UVs over the 0–1 tile (with the material's
//! texture or the UV checker behind), UV / shell selection, move, and the UV toolbar.

use crate::app::App;
use crate::scene::NodeId;
use crate::uv_ops::Projection;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke};
use glam::Vec2;

#[derive(Clone, Copy)]
struct View {
    /// UV at the canvas centre.
    center: Vec2,
    /// Pixels per UV unit.
    zoom: f32,
}

#[derive(Clone, Default)]
enum Drag {
    #[default]
    None,
    /// (start, current)
    Marquee(Pos2, Pos2),
    Move(Vec2),
    Pan,
}

fn report(app: &mut App, r: Result<usize, String>) {
    if let Err(e) = r {
        app.warn(e);
    }
}

/// Texture shown behind the UVs (cached egui texture of the material map or checker).
fn background(app: &mut App, ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique(("uv_bg", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = app.textures.get(name).ok()?;
    // Our images are stored bottom row first; egui wants top row first.
    let n = crate::texture::TEX_SIZE as usize;
    let lvl = 1; // 512² is plenty for the editor
    let s = n >> lvl;
    let src = &img.mips[lvl];
    let mut rgba = vec![0u8; s * s * 4];
    for y in 0..s {
        rgba[y * s * 4..(y + 1) * s * 4].copy_from_slice(&src[(s - 1 - y) * s * 4..(s - y) * s * 4]);
    }
    let tex = ctx.load_texture(format!("uv_bg_{name}"), egui::ColorImage::from_rgba_unmultiplied([s, s], &rgba), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

pub fn uv_editor_window(app: &mut App, ctx: &egui::Context) {
    if !app.panels_uv_editor {
        return;
    }
    let mut open = true;
    egui::Window::new("UV Editor").open(&mut open).default_size([640.0, 600.0]).resizable(true).show(ctx, |ui| {
        toolbar(app, ui);
        ui.separator();
        canvas(app, ui);
    });
    if !open {
        app.panels_uv_editor = false;
    }
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Create:").strong());
        for (l, p, tip) in [
            ("Planar", Projection::Planar(3), "Planar projection along the best axis"),
            ("Planar X", Projection::Planar(0), "Planar projection along X"),
            ("Planar Y", Projection::Planar(1), "Planar projection along Y (top)"),
            ("Planar Z", Projection::Planar(2), "Planar projection along Z (front)"),
            ("Cylindrical", Projection::Cylindrical, "Cylindrical projection around Y"),
            ("Spherical", Projection::Spherical, "Spherical projection"),
            ("Automatic", Projection::Automatic, "Automatic: 6 planar directions, packed"),
        ] {
            if ui.button(l).on_hover_text(tip).clicked() {
                let r = app.uv_project(p);
                report(app, r);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Edit:").strong());
        if ui.button("Layout").on_hover_text("Pack shells into 0–1 without overlaps").clicked() {
            let r = app.uv_layout();
            report(app, r);
        }
        if ui.button("Normalize").on_hover_text("Fit all UVs into 0–1").clicked() {
            let r = app.uv_normalize();
            report(app, r);
        }
        if ui.button("Flip U").clicked() {
            let r = app.uv_transform(Vec2::ZERO, Vec2::new(-1.0, 1.0), 0.0, None, false);
            report(app, r);
        }
        if ui.button("Flip V").clicked() {
            let r = app.uv_transform(Vec2::ZERO, Vec2::new(1.0, -1.0), 0.0, None, false);
            report(app, r);
        }
        if ui.button("⟲ 90°").clicked() {
            let r = app.uv_transform(Vec2::ZERO, Vec2::ONE, 90.0, None, false);
            report(app, r);
        }
        if ui.button("⟳ 90°").clicked() {
            let r = app.uv_transform(Vec2::ZERO, Vec2::ONE, -90.0, None, false);
            report(app, r);
        }
        if ui.button("Scale ×2").clicked() {
            let r = app.uv_transform(Vec2::ZERO, Vec2::splat(2.0), 0.0, None, false);
            report(app, r);
        }
        if ui.button("Scale ½").clicked() {
            let r = app.uv_transform(Vec2::ZERO, Vec2::splat(0.5), 0.0, None, false);
            report(app, r);
        }
        ui.separator();
        if ui.button("✂ Cut").on_hover_text("Split UVs along the selected edges (edge mode in the viewport, or UVs along an edge here)").clicked() {
            let r = app.uv_cut().map(|n| format!("{n} UVs split"));
            super::report(app, r);
        }
        if ui.button("Sew").on_hover_text("Move and sew: join shells along the selected seam edges").clicked() {
            let r = app.uv_sew().map(|n| format!("{n} edges sewn"));
            super::report(app, r);
        }
        if ui.button("Unfold").on_hover_text("Flatten shells with least-squares conformal mapping (selected UVs' shells, selected faces, or whole meshes)").clicked() {
            let r = app.uv_unfold().map(|n| format!("{n} shells unfolded"));
            super::report(app, r);
        }
        ui.separator();
        if ui.button("Select Shell").on_hover_text("Grow the UV selection to whole shells (or double-click a UV)").clicked() {
            app.uv_select_shell();
        }
        if ui.button("Select All").clicked() {
            select_all(app);
        }
    });
    ui.horizontal(|ui| {
        let v = app.active_view;
        ui.checkbox(&mut app.views[v].uv_checker, "UV checker in viewport");
        ui.checkbox(&mut app.views[v].textured, "Textures in viewport (6)");
        let st: Vec<f32> = targets(app).iter().filter_map(|id| app.scene.node(*id).and_then(|n| n.mesh())).map(crate::uv::stretch).collect();
        if !st.is_empty() {
            let v = st.iter().sum::<f32>() / st.len() as f32;
            let col = if v < 0.1 { Color32::from_rgb(130, 200, 130) } else if v < 0.3 { Color32::from_rgb(230, 190, 90) } else { Color32::from_rgb(230, 120, 90) };
            ui.label(RichText::new(format!("Stretch {v:.2}")).color(col)).on_hover_text("Average area distortion between 3D and UV (0 = none). Cut seams + Unfold to reduce it.");
        }
        ui.label(RichText::new("Drag UVs to move · drag empty space to select · Shift adds · MMB/Alt pans · wheel zooms · F frames").small().weak());
    });
}

fn targets(app: &App) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = app.uv_targets().into_iter().map(|(id, _)| id).collect();
    v.dedup();
    v
}

fn select_all(app: &mut App) {
    app.uv_sel.clear();
    for id in targets(app) {
        if let Some(m) = app.scene.node(id).and_then(|n| n.mesh()).filter(|m| m.has_uvs()) {
            for i in 0..m.uvs.len() as u32 {
                app.uv_sel.insert((id, i));
            }
        }
    }
}

fn canvas(app: &mut App, ui: &mut egui::Ui) {
    let size = ui.available_size().max(egui::vec2(200.0, 200.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let vid = ui.scope_id().with("uv_view");
    let did = ui.scope_id().with("uv_drag");
    let mut view = ui.ctx().data(|d| d.get_temp::<View>(vid)).unwrap_or(View { center: Vec2::splat(0.5), zoom: (rect.width().min(rect.height()) * 0.8).max(50.0) });
    let mut drag: Drag = ui.ctx().data(|d| d.get_temp::<Drag>(did)).unwrap_or_default();
    let to_screen = |uv: Vec2, view: &View| Pos2::new(rect.center().x + (uv.x - view.center.x) * view.zoom, rect.center().y - (uv.y - view.center.y) * view.zoom);
    let to_uv = |p: Pos2, view: &View| Vec2::new(view.center.x + (p.x - rect.center().x) / view.zoom, view.center.y - (p.y - rect.center().y) / view.zoom);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(52, 52, 52));

    // Background texture: the lead mesh's colour map, else the checker when it's on.
    let ids = targets(app);
    let lead_map = ids
        .first()
        .and_then(|id| app.scene.node(*id))
        .and_then(|n| n.material)
        .and_then(|m| app.material(m))
        .and_then(|m| m.color_map.clone());
    let bg_name = lead_map.or_else(|| app.views[app.active_view].uv_checker.then(|| crate::texture::CHECKER.to_string()));
    let tile = Rect::from_two_pos(to_screen(Vec2::ZERO, &view), to_screen(Vec2::ONE, &view));
    if let Some(name) = bg_name
        && let Some(tex) = background(app, ui.ctx(), &name)
    {
        painter.image(tex.id(), tile, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::from_gray(150));
    }
    // Grid: 0.1 steps, 0–1 tile outlined.
    for i in -10..=20 {
        let t = i as f32 * 0.1;
        let c = if i == 0 || i == 10 { Color32::from_gray(150) } else { Color32::from_gray(75) };
        painter.line_segment([to_screen(Vec2::new(t, -1.0), &view), to_screen(Vec2::new(t, 2.0), &view)], Stroke::new(1.0, c));
        painter.line_segment([to_screen(Vec2::new(-1.0, t), &view), to_screen(Vec2::new(2.0, t), &view)], Stroke::new(1.0, c));
    }
    painter.text(to_screen(Vec2::new(0.0, 0.0), &view) + egui::vec2(3.0, -12.0), egui::Align2::LEFT_CENTER, "0,0", egui::FontId::monospace(10.0), Color32::from_gray(170));
    painter.text(to_screen(Vec2::new(1.0, 1.0), &view) + egui::vec2(-3.0, 10.0), egui::Align2::RIGHT_CENTER, "1,1", egui::FontId::monospace(10.0), Color32::from_gray(170));

    // UV wireframe and points.
    let mut no_uv: Vec<String> = Vec::new();
    let mut points: Vec<(NodeId, u32, Pos2)> = Vec::new();
    for &id in &ids {
        let Some(n) = app.scene.node(id) else { continue };
        let Some(m) = n.mesh() else { continue };
        if !m.has_uvs() {
            no_uv.push(n.name.clone());
            continue;
        }
        let edge = Stroke::new(1.0, Color32::from_rgb(110, 170, 255));
        for f in &m.face_uvs {
            for k in 0..f.len() {
                let a = to_screen(m.uvs[f[k] as usize], &view);
                let b = to_screen(m.uvs[f[(k + 1) % f.len()] as usize], &view);
                painter.line_segment([a, b], edge);
            }
        }
        for (i, uv) in m.uvs.iter().enumerate() {
            points.push((id, i as u32, to_screen(*uv, &view)));
        }
    }
    for (id, i, p) in &points {
        if app.uv_sel.contains(&(*id, *i)) {
            painter.circle_filled(*p, 3.0, Color32::from_rgb(255, 140, 40));
        }
    }

    // ---- interaction
    let hover = resp.hover_pos();
    let shift = ui.input(|i| i.modifiers.shift);
    let alt = ui.input(|i| i.modifiers.alt);
    let nearest = |p: Pos2| -> Option<(NodeId, u32)> {
        points.iter().map(|(id, i, q)| ((*id, *i), q.distance(p))).filter(|(_, d)| *d < 8.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
    };
    // Zoom about the cursor.
    if let Some(hp) = hover {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            let before = to_uv(hp, &view);
            view.zoom = (view.zoom * (1.0 + scroll * 0.002)).clamp(20.0, 20_000.0);
            let after = to_uv(hp, &view);
            view.center += before - after;
        }
        if ui.input(|i| i.key_pressed(egui::Key::F)) {
            let (lo, hi) = app.uv_selection_bounds().unwrap_or((Vec2::ZERO, Vec2::ONE));
            view.center = (lo + hi) * 0.5;
            view.zoom = (rect.width().min(rect.height()) * 0.8 / (hi - lo).max_element().max(0.01)).clamp(20.0, 20_000.0);
        }
    }
    let mmb = ui.input(|i| i.pointer.middle_down());
    if resp.drag_started() {
        // Where the button went down (the drag starts a few pixels later).
        let p = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
        drag = if mmb || alt {
            Drag::Pan
        } else if let Some(k) = nearest(p) {
            if !app.uv_sel.contains(&k) {
                if !shift {
                    app.uv_sel.clear();
                }
                app.uv_sel.insert(k);
            }
            app.checkpoint();
            Drag::Move(to_uv(p, &view))
        } else {
            Drag::Marquee(p, p)
        };
    }
    if resp.dragged() {
        let p = resp.interact_pointer_pos().unwrap_or(rect.center());
        match &mut drag {
            Drag::Pan => {
                let d = resp.drag_delta();
                view.center -= Vec2::new(d.x, -d.y) / view.zoom;
            }
            Drag::Move(last) => {
                let now = to_uv(p, &view);
                let d = now - *last;
                *last = now;
                let _ = app.uv_transform(d, Vec2::ONE, 0.0, None, true);
            }
            Drag::Marquee(start, cur) => {
                *cur = p;
                let r = Rect::from_two_pos(*start, p);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            Drag::None => {}
        }
    }
    if resp.drag_stopped() {
        if let Drag::Marquee(start, cur) = drag {
            let end = resp.interact_pointer_pos().unwrap_or(cur);
            let r = Rect::from_two_pos(start, end);
            if !shift {
                app.uv_sel.clear();
            }
            for (id, i, q) in &points {
                if r.contains(*q) {
                    app.uv_sel.insert((*id, *i));
                }
            }
        }
        drag = Drag::None;
    }
    if resp.clicked() {
        let p = resp.interact_pointer_pos().unwrap_or(rect.center());
        match nearest(p) {
            Some(k) => {
                if shift {
                    if !app.uv_sel.remove(&k) {
                        app.uv_sel.insert(k);
                    }
                } else {
                    app.uv_sel.clear();
                    app.uv_sel.insert(k);
                }
            }
            None if !shift => app.uv_sel.clear(),
            None => {}
        }
    }
    if resp.double_clicked() {
        app.uv_select_shell();
    }

    // Status.
    let msg = if ids.is_empty() {
        "Select a polygon object (or faces) to see its UVs.".to_string()
    } else if !no_uv.is_empty() {
        format!("{} has no UVs — use a projection (Automatic is a good start).", no_uv.join(", "))
    } else {
        let shells: usize = ids.iter().filter_map(|id| app.scene.node(*id).and_then(|n| n.mesh())).map(|m| crate::uv::shells(m).len()).sum();
        format!("{} UVs selected · {} shell(s) · {} mesh(es)", app.uv_sel.len(), shells, ids.len())
    };
    painter.text(rect.left_bottom() + egui::vec2(6.0, -8.0), egui::Align2::LEFT_BOTTOM, msg, egui::FontId::proportional(12.0), Color32::from_gray(210));
    ui.ctx().data_mut(|d| {
        d.insert_temp(vid, view);
        d.insert_temp(did, drag);
    });
}
