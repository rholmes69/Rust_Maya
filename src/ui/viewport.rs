//! Model panels: camera navigation, picking, marquee selection, transform
//! manipulators, component display and heads-up display.

use crate::app::{App, CompMode, Shading, Tool};
use crate::camera::{CamKind, Projector};
use crate::render::ViewCallback;
use crate::scene::{Channel, NodeId, Shape};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind};
use eframe::egui_wgpu;
use glam::{Mat4, Quat, Vec2, Vec3};
use std::sync::Arc;

const GIZMO_PX: f32 = 90.0;
const AXIS_COLORS: [Color32; 3] = [
    Color32::from_rgb(230, 60, 60),
    Color32::from_rgb(90, 220, 70),
    Color32::from_rgb(60, 110, 240),
];
const ACTIVE: Color32 = Color32::from_rgb(255, 235, 30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Axis(usize),
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CamOp {
    Tumble,
    Track,
    Dolly,
}

#[derive(Clone, Debug)]
enum DragKind {
    Paint,
    Camera(CamOp),
    Gizmo { tool: Tool, handle: Handle },
    Marquee,
}

#[derive(Clone, Debug)]
pub struct Drag {
    view: usize,
    button: egui::PointerButton,
    kind: DragKind,
    start: Pos2,
    last: Pos2,
    moved: bool,
    // Manipulator data captured at drag start.
    axes: [Vec3; 3],
    pivot: Vec3,
    pivot_s: Vec2,
    screen_dir: Vec2,
    px_len: f32,
    gizmo_len: f32,
    angle_prev: f32,
    angle_acc: f32,
    to_cam: Vec3,
    nodes: Vec<(NodeId, Vec3, Vec3, Vec3, Mat4)>,
    verts: Vec<(NodeId, Mat4, Vec<(u32, Vec3, f32)>)>,
}

#[derive(Default)]
pub struct Interact {
    pub drag: Option<Drag>,
    pub hover: Option<Handle>,
    pub hover_view: Option<usize>,
}

fn to_pos(v: Vec2) -> Pos2 {
    Pos2::new(v.x, v.y)
}
fn to_v2(p: Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

/// Draw all model panels into `rect` (single or four-view layout).
pub fn show_views(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let geom = app.build_geometry();
    let ids: Vec<usize> = if app.quad { vec![1, 0, 2, 3] } else { vec![app.active_view] };
    let rects: Vec<Rect> = if app.quad {
        let gap = 3.0;
        let w = (rect.width() - gap) / 2.0;
        let h = (rect.height() - gap) / 2.0;
        vec![
            Rect::from_min_size(rect.min, egui::vec2(w, h)),
            Rect::from_min_size(rect.min + egui::vec2(w + gap, 0.0), egui::vec2(w, h)),
            Rect::from_min_size(rect.min + egui::vec2(0.0, h + gap), egui::vec2(w, h)),
            Rect::from_min_size(rect.min + egui::vec2(w + gap, h + gap), egui::vec2(w, h)),
        ]
    } else {
        vec![rect]
    };
    for (vid, r) in ids.into_iter().zip(rects) {
        show_view(app, ui, vid, r, geom.clone());
    }
    ui.allocate_rect(rect, egui::Sense::hover());
}

fn view_menu(app: &mut App, ui: &mut egui::Ui, vid: usize) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("View", |ui| {
            if ui.button("Frame All          A").clicked() {
                app.frame(vid, false);
            }
            if ui.button("Frame Selection    F").clicked() {
                app.frame(vid, true);
            }
            ui.separator();
            ui.label("Camera");
            for k in [CamKind::Persp, CamKind::Top, CamKind::Front, CamKind::Side] {
                let cur = app.views[vid].cam.kind;
                if ui.selectable_label(cur == k && app.views[vid].look_through.is_none(), k.name()).clicked() {
                    app.views[vid].look_through = None;
                    app.views[vid].cam.fov_y = 37.8;
                    let cam = &mut app.views[vid].cam;
                    *cam = crate::camera::Camera { kind: k, ..cam.clone() };
                    if k != CamKind::Persp {
                        cam.target = Vec3::ZERO;
                    }
                }
            }
            ui.separator();
            if ui.button("Default Home").clicked() {
                let k = app.views[vid].cam.kind;
                app.views[vid].cam = crate::camera::Camera::new(k);
            }
        });
        ui.menu_button("Shading", |ui| {
            let s = &mut app.views[vid].shading;
            ui.radio_value(s, Shading::Wireframe, "Wireframe                4");
            ui.radio_value(s, Shading::Shaded, "Smooth Shade All         5");
            ui.radio_value(s, Shading::WireOnShaded, "Wireframe on Shaded");
            ui.separator();
            ui.checkbox(&mut app.views[vid].textured, "Hardware Texturing     6");
            ui.checkbox(&mut app.views[vid].uv_checker, "UV Checker");
            ui.checkbox(&mut app.views[vid].xray, "X-Ray                Alt+X");
        });
        ui.menu_button("Lighting", |ui| {
            let v = &mut app.views[vid];
            ui.radio_value(&mut v.all_lights, false, "Use Default Lighting");
            ui.radio_value(&mut v.all_lights, true, "Use All Lights           7");
            ui.separator();
            if ui.checkbox(&mut v.shadows, "Shadows").changed() && v.shadows {
                v.all_lights = true;
            }
            let n_lights = app.scene.nodes.iter().filter(|n| matches!(n.shape, crate::scene::Shape::Light(_))).count();
            let casters = app.scene.nodes.iter().filter(|n| matches!(&n.shape, crate::scene::Shape::Light(l) if l.shadows)).count();
            ui.label(egui::RichText::new(format!("{n_lights} light(s), {casters} casting shadows")).small().weak());
            if n_lights == 0 {
                ui.label(egui::RichText::new("Create › Lights to add one").small().weak());
            }
        });
        ui.menu_button("Show", |ui| {
            ui.checkbox(&mut app.views[vid].show_grid, "Grid");
            ui.checkbox(&mut app.views[vid].show_hud, "Heads Up Display");
        });
        ui.menu_button("Renderer", |ui| {
            let _ = ui.radio(true, "Viewport 2.0 (wgpu)");
        });
        ui.menu_button("Panels", |ui| {
            if ui.radio(!app.quad, "Single Perspective View").clicked() {
                app.quad = false;
            }
            if ui.radio(app.quad, "Four View").clicked() {
                app.quad = true;
            }
            ui.separator();
            if ui.button("Look Through Selected").clicked() {
                match app.lead().filter(|id| app.is_camera(*id)) {
                    Some(c) => {
                        let _ = app.look_through(vid, Some(c));
                    }
                    None => app.warn("Look Through Selected: select a camera."),
                }
            }
            let cams: Vec<(crate::scene::NodeId, String)> =
                app.scene.nodes.iter().filter(|n| matches!(n.shape, crate::scene::Shape::Camera(_))).map(|n| (n.id, n.name.clone())).collect();
            ui.menu_button("Perspective", |ui| {
                if ui.radio(app.views[vid].look_through.is_none() && app.views[vid].cam.kind == CamKind::Persp, "persp").clicked() {
                    let _ = app.look_through(vid, None);
                    app.views[vid].cam.kind = CamKind::Persp;
                }
                for (id, name) in &cams {
                    if ui.radio(app.views[vid].look_through == Some(*id), name).clicked() {
                        let _ = app.look_through(vid, Some(*id));
                    }
                }
            });
        });
        // Panel toolbar shortcuts.
        ui.separator();
        let s = app.views[vid].shading;
        if ui.selectable_label(s == Shading::Wireframe, "▦").on_hover_text("Wireframe (4)").clicked() {
            app.views[vid].shading = Shading::Wireframe;
        }
        if ui.selectable_label(s == Shading::Shaded, "●").on_hover_text("Smooth shade all (5)").clicked() {
            app.views[vid].shading = Shading::Shaded;
        }
        if ui.selectable_label(s == Shading::WireOnShaded, "◉").on_hover_text("Wireframe on shaded").clicked() {
            app.views[vid].shading = Shading::WireOnShaded;
        }
        if ui.selectable_label(app.views[vid].show_grid, "#").on_hover_text("Grid").clicked() {
            app.views[vid].show_grid ^= true;
        }
        ui.separator();
        if ui.selectable_label(app.views[vid].all_lights, "💡").on_hover_text("Use all lights (7)").clicked() {
            app.views[vid].all_lights ^= true;
        }
        if ui.selectable_label(app.views[vid].shadows, "◐").on_hover_text("Shadows").clicked() {
            let v = &mut app.views[vid];
            v.shadows ^= true;
            if v.shadows {
                v.all_lights = true;
            }
        }
        if ui.selectable_label(app.views[vid].xray, "◌").on_hover_text("X-Ray (Alt+X)").clicked() {
            app.views[vid].xray ^= true;
        }
        if ui.selectable_label(app.views[vid].textured, "▣").on_hover_text("Textures (6)").clicked() {
            app.views[vid].textured ^= true;
        }
        if ui.selectable_label(app.views[vid].uv_checker, "▚").on_hover_text("UV checker").clicked() {
            app.views[vid].uv_checker ^= true;
        }
    });
}

fn show_view(app: &mut App, ui: &mut egui::Ui, vid: usize, rect: Rect, geom: Arc<crate::render::FrameGeometry>) {
    let header_h = 22.0;
    let header = Rect::from_min_size(rect.min, egui::vec2(rect.width(), header_h));
    let vp = Rect::from_min_max(rect.min + egui::vec2(0.0, header_h), rect.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(header), |ui| {
        ui.painter().rect_filled(header, 0.0, Color32::from_rgb(62, 62, 62));
        view_menu(app, ui, vid);
    });

    let id = ui.scope_id().with(("viewport", vid));
    let resp = ui.interact(vp, id, egui::Sense::click_and_drag());
    let painter = ui.painter_at(vp);
    let view = &app.views[vid];
    let pr = Projector::new(&view.cam, to_v2(vp.min), Vec2::new(vp.width(), vp.height()));
    let back = view.cam.back_dir();
    let light = (back + view.cam.right() * -0.35 + view.cam.up() * 0.45).normalize();
    let mut grid = if view.show_grid { crate::app::grid_lines(view.cam.kind) } else { Vec::new() };
    for (cid, lines) in &geom.camera_icons {
        if view.look_through != Some(*cid) {
            grid.extend_from_slice(lines);
        }
    }
    let cb = ViewCallback {
        view_id: vid,
        rect: vp,
        view_proj: pr.view_proj,
        light_dir: light,
        geom,
        grid: Arc::new(grid),
        draw_shaded: view.shading != Shading::Wireframe,
        draw_wire_all: view.shading != Shading::Shaded,
        bg: [0.267, 0.267, 0.267],
        eye: view.cam.eye(),
        all_lights: view.all_lights,
        shadows: view.shadows,
        xray: view.xray,
        textured: view.textured,
        uv_checker: view.uv_checker,
    };
    painter.add(egui_wgpu::Callback::new_paint_callback(vp, cb));

    if resp.hovered() {
        app.active_view = vid;
    }
    handle_input(app, ui, vid, vp, &resp, &pr);
    draw_components(app, &painter, &pr, vid);
    // Multi-Cut preview.
    if !app.cut_points.is_empty() {
        let pts: Vec<Pos2> = app
            .cut_points
            .iter()
            .filter_map(|(id, (a, b), t)| {
                let w = app.scene.world_matrix(*id);
                let mesh = app.scene.node(*id)?.mesh()?;
                let t = if *t <= 0.03 { 0.0 } else if *t >= 0.97 { 1.0 } else { *t };
                let p = w.transform_point3(mesh.positions.get(*a as usize)?.lerp(*mesh.positions.get(*b as usize)?, t));
                pr.project(p).map(to_pos)
            })
            .collect();
        for w in pts.windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(2.0, Color32::from_rgb(255, 120, 200)));
        }
        for p in &pts {
            painter.circle_filled(*p, 4.0, Color32::from_rgb(255, 200, 240));
        }
    }
    draw_tool_overlays(app, &painter, &pr, ui);
    draw_gizmo(app, &painter, vid, vp, &pr);
    if resp.hovered() {
        if let Some(mp) = ui.input(|i| i.pointer.hover_pos()) {
            match app.tool {
                Tool::PaintWeights => {
                    painter.circle_stroke(mp, app.paint.radius, Stroke::new(1.5, Color32::from_rgb(255, 80, 80)));
                }
                Tool::Joint => {
                    painter.text(mp + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, "joint: click to place · Enter to finish", egui::FontId::proportional(11.0), Color32::from_rgb(160, 200, 255));
                }
                Tool::Sculpt => {
                    let col = if app.sculpt.invert || ui.input(|i| i.modifiers.ctrl) { Color32::from_rgb(120, 160, 255) } else { Color32::from_rgb(255, 120, 80) };
                    painter.circle_stroke(mp, app.sculpt.radius_px, Stroke::new(1.5, col));
                    painter.text(mp + egui::vec2(app.sculpt.radius_px + 4.0, 0.0), egui::Align2::LEFT_CENTER, app.sculpt.brush.name(), egui::FontId::proportional(11.0), col);
                }
                Tool::MultiCut => {
                    if let Some((id, _, (a, b), t)) = pick_edge(app, &pr, to_v2(mp), vid, true) {
                        let w = app.scene.world_matrix(id);
                        if let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) {
                            let pa = w.transform_point3(mesh.positions[a as usize]);
                            let pb = w.transform_point3(mesh.positions[b as usize]);
                            let snap_t = if t <= 0.03 { 0.0 } else if t >= 0.97 { 1.0 } else { t };
                            if let Some(sp) = pr.project(pa.lerp(pb, snap_t)) {
                                painter.circle_filled(to_pos(sp), 4.0, Color32::from_rgb(255, 120, 200));
                            }
                        }
                    }
                }
                Tool::EdgeLoop => {
                    if let Some((id, _, (a, b), t)) = pick_edge(app, &pr, to_v2(mp), vid, true) {
                        let w = app.scene.world_matrix(id);
                        if let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) {
                            let pa = w.transform_point3(mesh.positions[a as usize]);
                            let pb = w.transform_point3(mesh.positions[b as usize]);
                            if let (Some(sa), Some(sb), Some(sp)) = (pr.project(pa), pr.project(pb), pr.project(pa.lerp(pb, t))) {
                                painter.line_segment([to_pos(sa), to_pos(sb)], Stroke::new(2.5, Color32::YELLOW));
                                painter.circle_filled(to_pos(sp), 4.0, Color32::YELLOW);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    draw_hud(app, &painter, vid, vp, &pr);

    // Marquee rectangle.
    if let Some(d) = &app.interact.drag
        && d.view == vid && matches!(d.kind, DragKind::Marquee) && d.moved {
            let r = Rect::from_two_pos(d.start, d.last);
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
        }
    // Active panel border (Maya highlights the active panel).
    let border = if app.active_view == vid && app.quad { Color32::from_rgb(82, 133, 166) } else { Color32::from_rgb(40, 40, 40) };
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, border), StrokeKind::Inside);

    resp.context_menu(|ui| marking_menu(app, ui));
}

fn marking_menu(app: &mut App, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Marking Menu").weak());
    if ui.button("Object Mode            F8").clicked() {
        app.set_comp_mode(CompMode::Object);
    }
    if ui.button("Vertex                 F9").clicked() {
        app.set_comp_mode(CompMode::Vertex);
    }
    if ui.button("Face                   F11").clicked() {
        app.set_comp_mode(CompMode::Face);
    }
    ui.separator();
    if app.comp_mode == CompMode::Face && ui.button("Extrude Face     Ctrl+E").clicked() {
        app.extrude();
    }
    if ui.button("Select All").clicked() {
        app.select_all();
    }
    if ui.button("Frame Selection        F").clicked() {
        let v = app.active_view;
        app.frame(v, true);
    }
    if ui.button("Delete").clicked() {
        app.delete_selected();
    }
}

/// Gizmo axes (world space) for the current tool and selection.
fn gizmo_axes(app: &App, tool: Tool) -> [Vec3; 3] {
    let world = [Vec3::X, Vec3::Y, Vec3::Z];
    if app.comp_mode != CompMode::Object {
        return world;
    }
    let Some(id) = app.lead() else { return world };
    let Some(n) = app.scene.node(id) else { return world };
    match tool {
        Tool::Rotate => {
            // Gimbal axes for XYZ order.
            let pm = glam::Mat3::from_mat4(app.scene.parent_matrix(id));
            let r = n.rotate * (std::f32::consts::PI / 180.0);
            let rz = Quat::from_rotation_z(r.z);
            let ry = Quat::from_rotation_y(r.y);
            let ax = rz * ry * Vec3::X;
            let ay = rz * Vec3::Y;
            let az = Vec3::Z;
            [ax, ay, az].map(|a| (pm * a).normalize_or(a))
        }
        Tool::Scale => {
            let w = glam::Mat3::from_mat4(app.scene.world_matrix(id));
            [w.x_axis, w.y_axis, w.z_axis].map(|a| a.normalize_or(Vec3::X))
        }
        _ => world,
    }
}

fn ring_basis(axis: Vec3) -> (Vec3, Vec3) {
    let helper = if axis.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = axis.cross(helper).normalize();
    let v = axis.cross(u).normalize();
    (u, v)
}

fn to_cam_dir(app: &App, vid: usize, p: Vec3) -> Vec3 {
    let cam = &app.views[vid].cam;
    if cam.is_ortho() { cam.back_dir() } else { (cam.eye() - p).normalize() }
}

/// Screen polylines of the front-facing half of a rotate ring.
fn ring_points(pivot: Vec3, axis: Vec3, len: f32, to_cam: Vec3, pr: &Projector) -> Vec<Vec<Vec2>> {
    let (u, v) = ring_basis(axis);
    let mut runs: Vec<Vec<Vec2>> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    let flat = axis.dot(to_cam).abs() > 0.98;
    for k in 0..=72 {
        let a = k as f32 / 72.0 * std::f32::consts::TAU;
        let dir = u * a.cos() + v * a.sin();
        let visible = flat || dir.dot(to_cam) >= -0.08;
        if visible
            && let Some(s) = pr.project(pivot + dir * len) {
                cur.push(s);
                continue;
            }
        if cur.len() > 1 {
            runs.push(std::mem::take(&mut cur));
        } else {
            cur.clear();
        }
    }
    if cur.len() > 1 {
        runs.push(cur);
    }
    runs
}

fn gizmo_len(app: &App, vid: usize, pivot: Vec3, vp: Rect) -> f32 {
    app.views[vid].cam.world_per_pixel(pivot, vp.height()) * GIZMO_PX
}

fn has_transform_target(app: &App) -> bool {
    match app.comp_mode {
        CompMode::Object => app.lead().is_some(),
        _ => !app.comp_sel.is_empty(),
    }
}

fn hit_gizmo(app: &App, vid: usize, vp: Rect, pr: &Projector, mouse: Vec2) -> Option<Handle> {
    if !app.tool.has_gizmo() || !has_transform_target(app) {
        return None;
    }
    let pivot = app.pivot()?;
    let ps = pr.project(pivot)?;
    let len = gizmo_len(app, vid, pivot, vp);
    let axes = gizmo_axes(app, app.tool);
    if app.tool == Tool::Rotate {
        let to_cam = to_cam_dir(app, vid, pivot);
        let mut best = (10.0f32, None);
        for (i, ax) in axes.iter().enumerate() {
            for run in ring_points(pivot, *ax, len, to_cam, pr) {
                for w in run.windows(2) {
                    let d = seg_dist(mouse, w[0], w[1]);
                    if d < best.0 {
                        best = (d, Some(Handle::Axis(i)));
                    }
                }
            }
        }
        return best.1;
    }
    if (mouse - ps).length() < 11.0 {
        return Some(Handle::Center);
    }
    let mut best = (9.0f32, None);
    for (i, ax) in axes.iter().enumerate() {
        if let Some(tip) = pr.project(pivot + *ax * len) {
            let d = seg_dist(mouse, ps, tip);
            if d < best.0 {
                best = (d, Some(Handle::Axis(i)));
            }
        }
    }
    best.1
}

fn handle_input(app: &mut App, ui: &egui::Ui, vid: usize, vp: Rect, resp: &egui::Response, pr: &Projector) {
    use egui::PointerButton as B;
    let (mouse, mods, pressed, released, scroll, x_held) = ui.input(|i| {
        let pressed = [B::Primary, B::Middle, B::Secondary].map(|b| i.pointer.button_pressed(b));
        let released = [B::Primary, B::Middle, B::Secondary].map(|b| i.pointer.button_released(b));
        (i.pointer.hover_pos(), i.modifiers, pressed, released, i.smooth_scroll_delta.y, i.key_down(egui::Key::X))
    });
    let Some(mouse) = mouse.or_else(|| ui.input(|i| i.pointer.interact_pos())) else { return };
    let m = to_v2(mouse);

    // Hover highlight for manipulator handles.
    if app.interact.drag.is_none() {
        if resp.hovered() {
            app.interact.hover = hit_gizmo(app, vid, vp, pr, m);
            app.interact.hover_view = Some(vid);
        } else if app.interact.hover_view == Some(vid) {
            app.interact.hover = None;
        }
    }

    // Wheel dolly.
    if resp.hovered() && scroll != 0.0 {
        app.views[vid].cam.dolly(scroll * 0.5);
    }

    // Insert Edge Loop tool: click an edge to cut a loop through its ring at that point.
    if app.tool == Tool::EdgeLoop && app.interact.drag.is_none() && resp.hovered() && pressed[0] && !mods.alt {
        if let Some((id, _, e, t)) = pick_edge(app, pr, m, vid, true) {
            if !app.sel.contains(&id) {
                app.sel = vec![id];
            }
            let n = app.insert_edge_loop(id, e, t);
            app.info(format!("Inserted edge loop ({n} edges)"));
        }
        return;
    }
    // Multi-Cut: each click adds a point on an edge of the same mesh; right-click/Enter cuts.
    if app.tool == Tool::MultiCut && app.interact.drag.is_none() && resp.hovered() && !mods.alt {
        if pressed[0] {
            if let Some((id, _, e, t)) = pick_edge(app, pr, m, vid, true)
                && app.cut_points.first().is_none_or(|p| p.0 == id)
            {
                if !app.sel.contains(&id) {
                    app.sel = vec![id];
                }
                app.cut_points.push((id, e, t));
            }
            return;
        }
        if pressed[2] {
            let r = app.finish_multi_cut().map(|n| format!("Multi-Cut: {n} face(s) split"));
            super::report(app, r);
            return;
        }
    }
    // Sculpt brushes.
    if app.tool == Tool::Sculpt && !mods.alt && app.interact.drag.is_none() {
        let down = ui.input(|i| i.pointer.primary_down());
        if pressed[0] && resp.hovered() {
            let target = app.lead().filter(|id| app.scene.node(*id).is_some_and(|n| n.mesh().is_some()));
            let hit = sculpt_hit(app, pr, m, target).or_else(|| sculpt_hit(app, pr, m, None));
            if let Some((id, p, _)) = hit {
                app.sculpt.invert = mods.ctrl;
                app.sculpt_begin(id);
                let r = app.sculpt.radius_px * app.views[vid].cam.world_per_pixel(p, vp.height());
                if app.sculpt.brush == crate::sculpt::Brush::Grab {
                    app.sculpt_grab_start(p, r);
                }
            }
            return;
        }
        if let Some(st) = app.sculpt_stroke.clone() {
            if !down {
                app.sculpt_end();
                return;
            }
            if app.sculpt.brush == crate::sculpt::Brush::Grab {
                let (ro, rd) = pr.ray(m);
                let n = app.views[vid].cam.forward();
                let den = rd.dot(n);
                if den.abs() > 1e-6 {
                    let t = (st.anchor - ro).dot(n) / den;
                    app.sculpt_grab_to(ro + rd * t - st.anchor);
                }
            } else if let Some((_, p, nrm)) = sculpt_hit(app, pr, m, Some(st.id)) {
                let r = app.sculpt.radius_px * app.views[vid].cam.world_per_pixel(p, vp.height());
                app.sculpt_dab(p, nrm, r);
            }
            return;
        }
    }
    // Double-click an edge: select its loop.
    if app.comp_mode == CompMode::Edge && resp.hovered() && ui.input(|i| i.pointer.button_double_clicked(B::Primary)) && !mods.alt {
        if let Some((id, idx, _, _)) = pick_edge(app, pr, m, vid, false) {
            app.comp_sel.insert((id, idx as u32));
            app.select_edge_loop(false);
        }
    }

    // CV / EP Curve tools: each click adds a point (on the live surface, else the grid).
    if matches!(app.tool, Tool::CurveCv | Tool::CurveEp) && app.interact.drag.is_none() && resp.hovered() && pressed[0] && !mods.alt {
        let mut p = construction_point(app, vid, pr, m);
        if x_held || app.snap_grid {
            p = p.round();
        }
        app.curve_tool.points.push(p);
        return;
    }
    // Quad Draw.
    if app.tool == Tool::QuadDraw && !mods.alt && app.interact.drag.is_none() {
        let down = ui.input(|i| i.pointer.primary_down());
        if let Some(v) = app.qd.drag {
            if !down || released[0] {
                app.qd.drag = None;
            } else if let Some(p) = qd_surface_point(app, vid, pr, m) {
                app.qd_move_dot(v, p);
            }
            return;
        }
        if let Some((a, b, start)) = app.qd.extend {
            if !down || released[0] {
                app.qd.extend = None;
                if let Some(p) = qd_surface_point(app, vid, pr, m)
                    && p.distance(start) > 1e-4
                {
                    let r = app.qd_extend(a, b, p - start).map(|_| "Quad Draw: extended".to_string());
                    super::report(app, r);
                }
            }
            return;
        }
        if pressed[0] && resp.hovered() {
            let pts = app.qd_points();
            let near_dot = pts.iter().enumerate().filter_map(|(i, p)| pr.project(*p).map(|s| (s.distance(m), i as u32))).filter(|x| x.0 < 9.0).min_by(|a, b| a.0.total_cmp(&b.0)).map(|x| x.1);
            if mods.shift {
                let k = if mods.ctrl { 3 } else { 4 };
                let near = app.qd_nearest_dots(m, k, &|p| pr.project(p));
                let fwd = app.views[vid].cam.forward();
                let r = app.qd_fill(&near, Some(fwd)).map(|_| if k == 4 { "Quad Draw: quad".to_string() } else { "Quad Draw: triangle".to_string() });
                super::report(app, r);
            } else if mods.ctrl {
                if let Some(v) = near_dot {
                    let r = app.qd_delete_dot(v).map(|_| "Quad Draw: dot deleted".to_string());
                    super::report(app, r);
                } else if let Some((a, b)) = qd_border_edge(app, pr, m) {
                    if let Some(p) = qd_surface_point(app, vid, pr, m) {
                        app.qd.extend = Some((a, b, p));
                    }
                } else if let Some(f) = qd_face_under(app, pr, m) {
                    let r = app.qd_delete_face(f).map(|_| "Quad Draw: face deleted".to_string());
                    super::report(app, r);
                }
            } else if let Some(v) = near_dot {
                app.checkpoint();
                app.qd.drag = Some(v);
            } else if let Some(p) = qd_surface_point(app, vid, pr, m) {
                app.qd_add_dot(p);
            }
            return;
        }
        if pressed[2] && resp.hovered() {
            app.qd_finish();
            app.tool = Tool::Select;
            app.info("Quad Draw finished");
            return;
        }
    }

    // Joint tool: each click places a joint (child of the previous one).
    if app.tool == Tool::Joint && app.interact.drag.is_none() && resp.hovered() && pressed[0] && !mods.alt {
        let mut p = joint_plane_point(app, vid, pr, m);
        if x_held || app.snap_grid {
            p = p.round();
        }
        let r = app.joint_radius;
        let id = app.create_joint(p, None, r);
        if app.joint_chain.is_none() {
            let mut root = id;
            while let Some(par) = app.scene.node(root).and_then(|n| n.parent).filter(|p| app.is_joint(*p)) {
                root = par;
            }
            app.joint_chain = Some(root);
        }
        return;
    }

    // Editable motion trails: drag a key marker (in the plane facing the camera).
    if !app.trails.is_empty() && !mods.alt && app.interact.drag.is_none() {
        if let Some((id, frame, p0)) = app.trail_drag {
            if released[0] || !ui.input(|i| i.pointer.primary_down()) {
                app.trail_drag = None;
            } else {
                let (ro, rd) = pr.ray(m);
                let n = app.views[vid].cam.forward();
                let den = rd.dot(n);
                if den.abs() > 1e-6 {
                    let t = (p0 - ro).dot(n) / den;
                    app.set_trail_key(id, frame, ro + rd * t, true);
                }
            }
            return;
        }
        if pressed[0] && resp.hovered() {
            let ids: Vec<NodeId> = app.trails.iter().copied().collect();
            let mut best: Option<(f32, NodeId, f32, Vec3)> = None;
            for id in ids {
                for (f, p) in app.trail_keys(id) {
                    if let Some(sp) = pr.project(p) {
                        let d = sp.distance(m);
                        if d < 8.0 && best.is_none_or(|b| d < b.0) {
                            best = Some((d, id, f, p));
                        }
                    }
                }
            }
            if let Some((_, id, f, p)) = best {
                app.checkpoint();
                app.trail_drag = Some((id, f, p));
                app.info(format!("Editing trail key at frame {f}"));
                return;
            }
        }
    }

    // Begin a drag.
    if app.interact.drag.is_none() && resp.hovered() {
        let button = if pressed[0] { Some(B::Primary) } else if pressed[1] { Some(B::Middle) } else if pressed[2] && mods.alt { Some(B::Secondary) } else { None };
        if let Some(button) = button {
            let kind = if mods.alt {
                match button {
                    B::Primary => DragKind::Camera(CamOp::Tumble),
                    B::Middle => DragKind::Camera(CamOp::Track),
                    _ => DragKind::Camera(CamOp::Dolly),
                }
            } else if app.tool == Tool::PaintWeights && button == B::Primary {
                if app.lead().and_then(|id| app.scene.node(id)).is_some_and(|n| n.skin.is_some()) {
                    app.checkpoint();
                    DragKind::Paint
                } else {
                    app.warn("Paint Skin Weights: select a skinned mesh (Skin > Bind Skin first).");
                    DragKind::Marquee
                }
            } else if button == B::Middle {
                // Middle-drag anywhere drives the active manipulator handle (like Maya), else tracks.
                match app.interact.hover {
                    Some(h) => DragKind::Gizmo { tool: app.tool, handle: h },
                    None => DragKind::Camera(CamOp::Track),
                }
            } else if let Some(h) = app.interact.hover {
                DragKind::Gizmo { tool: app.tool, handle: h }
            } else {
                DragKind::Marquee
            };
            let mut d = Drag {
                view: vid,
                button,
                kind,
                start: mouse,
                last: mouse,
                moved: false,
                axes: [Vec3::X, Vec3::Y, Vec3::Z],
                pivot: Vec3::ZERO,
                pivot_s: Vec2::ZERO,
                screen_dir: Vec2::X,
                px_len: 1.0,
                gizmo_len: 1.0,
                angle_prev: 0.0,
                angle_acc: 0.0,
                to_cam: Vec3::Z,
                nodes: Vec::new(),
                verts: Vec::new(),
            };
            if let DragKind::Gizmo { tool, handle } = d.kind {
                begin_gizmo(app, &mut d, tool, handle, vid, vp, pr, m);
            }
            app.interact.drag = Some(d);
        }
    }

    // Update / finish.
    let Some(mut d) = app.interact.drag.take() else { return };
    if d.view != vid {
        app.interact.drag = Some(d);
        return;
    }
    let delta = mouse - d.last;
    if (mouse - d.start).length() > 3.0 {
        d.moved = true;
    }
    match d.kind.clone() {
        DragKind::Camera(op) => {
            let cam = &mut app.views[vid].cam;
            match op {
                CamOp::Tumble => cam.tumble(delta.x, delta.y),
                CamOp::Track => cam.track(delta.x, delta.y, vp.height()),
                CamOp::Dolly => cam.dolly((delta.x + delta.y) * 1.5),
            }
        }
        DragKind::Gizmo { tool, handle } => {
            apply_gizmo(app, &mut d, tool, handle, vid, vp, m, x_held || app.snap_grid, mods.shift);
        }
        DragKind::Marquee => {}
        DragKind::Paint => paint_weights(app, pr, m),
    }
    d.last = mouse;
    let idx = match d.button {
        B::Primary => 0,
        B::Middle => 1,
        _ => 2,
    };
    if released[idx] || !ui.input(|i| i.pointer.button_down(d.button)) {
        match d.kind {
            DragKind::Marquee => {
                if d.moved {
                    marquee_select(app, pr, Rect::from_two_pos(d.start, mouse), mods);
                } else {
                    click_select(app, pr, m, mods, vid);
                }
            }
            DragKind::Gizmo { .. } => {
                let ids: Vec<NodeId> = d.nodes.iter().map(|n| n.0).collect();
                app.auto_key_touch(&ids);
            }
            _ => {}
        }
        return;
    }
    app.interact.drag = Some(d);
}

#[allow(clippy::too_many_arguments)]
fn begin_gizmo(app: &mut App, d: &mut Drag, tool: Tool, handle: Handle, vid: usize, vp: Rect, pr: &Projector, m: Vec2) {
    let Some(pivot) = app.pivot() else { return };
    app.checkpoint();
    d.pivot = pivot;
    d.pivot_s = pr.project(pivot).unwrap_or(m);
    d.gizmo_len = gizmo_len(app, vid, pivot, vp);
    d.axes = gizmo_axes(app, tool);
    d.to_cam = to_cam_dir(app, vid, pivot);
    if let Handle::Axis(i) = handle
        && let Some(tip) = pr.project(pivot + d.axes[i] * d.gizmo_len) {
            let v = tip - d.pivot_s;
            d.px_len = v.length().max(1e-3);
            d.screen_dir = v / d.px_len;
        }
    let a = m - d.pivot_s;
    d.angle_prev = a.y.atan2(a.x);
    if app.comp_mode == CompMode::Object {
        let roots = app.top_level(&app.sel.clone());
        d.nodes = roots
            .iter()
            .filter_map(|id| {
                let n = app.scene.node(*id)?;
                let wpos = app.scene.world_matrix(*id).transform_point3(Vec3::ZERO);
                Some((*id, wpos, n.rotate, n.scale, app.scene.parent_matrix(*id).inverse()))
            })
            .collect();
    } else {
        // Selected points plus soft-selection falloff (weights).
        let mut per: std::collections::HashMap<NodeId, Vec<(u32, f32)>> = std::collections::HashMap::new();
        for (id, v, wgt) in app.soft_weights() {
            per.entry(id).or_default().push((v, wgt));
        }
        for (id, verts) in per {
            let w = app.scene.world_matrix(id);
            let inv = w.inverse();
            if let Some(pts) = app.scene.node_mut(id).and_then(|n| n.points_mut()) {
                let list = verts.iter().filter_map(|&(v, wgt)| pts.get(v as usize).map(|p| (v, w.transform_point3(*p), wgt))).collect();
                d.verts.push((id, inv, list));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_gizmo(app: &mut App, d: &mut Drag, tool: Tool, handle: Handle, vid: usize, vp: Rect, m: Vec2, snap: bool, fine: bool) {
    let drag = m - to_v2(d.start);
    let fine_mul = if fine { 0.2 } else { 1.0 };
    // World-space transform function applied to points (component mode) and data for objects.
    match tool {
        Tool::Move | Tool::Select | Tool::Joint | Tool::PaintWeights | Tool::EdgeLoop | Tool::MultiCut | Tool::Sculpt | Tool::QuadDraw | Tool::CurveCv | Tool::CurveEp => {
            let delta = match handle {
                Handle::Axis(i) => {
                    let mut amount = drag.dot(d.screen_dir) / d.px_len * d.gizmo_len * fine_mul;
                    if snap && app.comp_mode == CompMode::Object {
                        let cur = d.pivot[i] + amount;
                        amount = cur.round() - d.pivot[i];
                    }
                    d.axes[i] * amount
                }
                Handle::Center => {
                    let cam = &app.views[vid].cam;
                    let wpp = cam.world_per_pixel(d.pivot, vp.height()) * fine_mul;
                    let mut dl = cam.right() * drag.x * wpp - cam.up() * drag.y * wpp;
                    if snap {
                        dl = (d.pivot + dl).round() - d.pivot;
                    }
                    dl
                }
            };
            if app.comp_mode == CompMode::Object {
                for (id, wpos, _, _, pinv) in &d.nodes {
                    let local = pinv.transform_point3(*wpos + delta);
                    if let Some(n) = app.scene.node_mut(*id) {
                        n.translate = local;
                    }
                }
            } else {
                transform_verts(app, d, |p| p + delta);
            }
        }
        Tool::Rotate => {
            let a = m - d.pivot_s;
            let ang = a.y.atan2(a.x);
            let mut da = ang - d.angle_prev;
            if da > std::f32::consts::PI {
                da -= std::f32::consts::TAU;
            } else if da < -std::f32::consts::PI {
                da += std::f32::consts::TAU;
            }
            d.angle_prev = ang;
            d.angle_acc += da * fine_mul;
            let i = match handle {
                Handle::Axis(i) => i,
                Handle::Center => return,
            };
            let sign = if d.axes[i].dot(d.to_cam) > 0.0 { -1.0 } else { 1.0 };
            let mut deg = d.angle_acc.to_degrees() * sign;
            if snap {
                deg = (deg / 15.0).round() * 15.0;
            }
            if app.comp_mode == CompMode::Object {
                let ch = Channel::rotate()[i];
                for (id, _, rot, _, _) in &d.nodes {
                    if let Some(n) = app.scene.node_mut(*id) {
                        n.set(ch, rot[i] + deg);
                    }
                }
            } else {
                let q = Quat::from_axis_angle(d.axes[i], deg.to_radians());
                let c = d.pivot;
                transform_verts(app, d, |p| c + q * (p - c));
            }
        }
        Tool::Scale => {
            let factor = match handle {
                Handle::Axis(_) => 1.0 + drag.dot(d.screen_dir) / d.px_len * fine_mul,
                Handle::Center => 1.0 + (drag.x - drag.y) / 100.0 * fine_mul,
            }
            .max(0.001);
            if app.comp_mode == CompMode::Object {
                for (id, _, _, sc, _) in &d.nodes {
                    if let Some(n) = app.scene.node_mut(*id) {
                        n.scale = match handle {
                            Handle::Axis(i) => {
                                let mut s = *sc;
                                s[i] *= factor;
                                s
                            }
                            Handle::Center => *sc * factor,
                        };
                    }
                }
            } else {
                let c = d.pivot;
                let axes = d.axes;
                transform_verts(app, d, |p| {
                    let v = p - c;
                    match handle {
                        Handle::Axis(i) => {
                            let along = v.dot(axes[i]);
                            c + v + axes[i] * along * (factor - 1.0)
                        }
                        Handle::Center => c + v * factor,
                    }
                });
            }
        }
    }
}

fn transform_verts(app: &mut App, d: &Drag, f: impl Fn(Vec3) -> Vec3) {
    for (id, inv, list) in &d.verts {
        if let Some(pts) = app.scene.node_mut(*id).and_then(|n| n.points_mut()) {
            for (v, start, wgt) in list {
                if let Some(p) = pts.get_mut(*v as usize) {
                    *p = inv.transform_point3(start.lerp(f(*start), *wgt));
                }
            }
        }
    }
}

// ------------------------------------------------------------------ rigging tools

/// Where the joint tool places a joint: on the view plane through the previous joint
/// (or through the origin / the ground for the first joint).
fn joint_plane_point(app: &App, vid: usize, pr: &Projector, m: Vec2) -> Vec3 {
    let cam = &app.views[vid].cam;
    let (ro, rd) = pr.ray(m);
    let prev = app.lead().filter(|id| app.is_joint(*id)).map(|id| crate::rig::world_pos(&app.scene, id));
    let (origin, normal) = match (cam.kind, prev) {
        (CamKind::Persp, None) => (Vec3::ZERO, Vec3::Y),
        (CamKind::Persp, Some(p)) => (p, cam.forward()),
        (_, Some(p)) => (p, cam.back_dir()),
        (_, None) => (Vec3::ZERO, cam.back_dir()),
    };
    let denom = rd.dot(normal);
    if denom.abs() < 1e-6 {
        return prev.unwrap_or(Vec3::ZERO);
    }
    let t = (origin - ro).dot(normal) / denom;
    ro + rd * t.max(0.0)
}

/// Where a click lands for the curve tools: the live surface if one is set, else the ground
/// grid (perspective) or the view plane through the origin (orthographic views).
fn construction_point(app: &App, vid: usize, pr: &Projector, m: Vec2) -> Vec3 {
    if let Some(live) = app.qd.live
        && let Some((_, p, n)) = sculpt_hit(app, pr, m, Some(live))
    {
        return p + n * app.qd.offset * 2.0;
    }
    let cam = &app.views[vid].cam;
    let (ro, rd) = pr.ray(m);
    let normal = if cam.kind == CamKind::Persp { Vec3::Y } else { cam.back_dir() };
    let den = rd.dot(normal);
    if den.abs() < 1e-6 {
        return Vec3::ZERO;
    }
    ro + rd * (-ro.dot(normal) / den).max(0.0)
}

/// Quad Draw surface point under the mouse: live object, else any other visible mesh, else
/// the construction plane.
fn qd_surface_point(app: &App, vid: usize, pr: &Projector, m: Vec2) -> Option<Vec3> {
    if let Some(live) = app.qd.live {
        return sculpt_hit(app, pr, m, Some(live)).map(|h| h.1);
    }
    let (ro, rd) = pr.ray(m);
    let mut best: Option<(f32, Vec3)> = None;
    for n in &app.scene.nodes {
        if Some(n.id) == app.qd.target || !app.scene.is_visible(n.id) {
            continue;
        }
        if let Some((_, p, _)) = sculpt_hit(app, pr, m, Some(n.id)) {
            let d = p.distance(ro);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, p));
            }
        }
    }
    let _ = rd;
    Some(best.map(|b| b.1).unwrap_or_else(|| construction_point(app, vid, pr, m)))
}

/// Border edge of the Quad Draw mesh under the mouse (within 8 px).
fn qd_border_edge(app: &App, pr: &Projector, m: Vec2) -> Option<(u32, u32)> {
    let mesh = app.qd.target.and_then(|t| app.scene.node(t)).and_then(|n| n.mesh())?;
    let pts = app.qd_points();
    crate::quad_draw::boundary_edges(mesh)
        .into_iter()
        .filter_map(|(a, b)| Some((seg_dist(m, pr.project(pts[a as usize])?, pr.project(pts[b as usize])?), (a, b))))
        .filter(|x| x.0 < 8.0)
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|x| x.1)
}

fn qd_face_under(app: &App, pr: &Projector, m: Vec2) -> Option<usize> {
    let t = app.qd.target?;
    let mesh = app.scene.node(t)?.mesh()?;
    let inv = app.scene.world_matrix(t).inverse();
    let (ro, rd) = pr.ray(m);
    mesh.raycast(inv.transform_point3(ro), inv.transform_vector3(rd)).map(|h| h.1)
}

/// Quad Draw and curve-tool overlays.
fn draw_tool_overlays(app: &App, painter: &egui::Painter, pr: &Projector, ui: &egui::Ui) {
    let hover = ui.input(|i| i.pointer.hover_pos()).map(to_v2);
    let mods = ui.input(|i| i.modifiers);
    if matches!(app.tool, Tool::CurveCv | Tool::CurveEp) && !app.curve_tool.points.is_empty() {
        let col = Color32::from_rgb(255, 210, 90);
        if let Some(c) = app.curve_tool_preview() {
            let pts: Vec<Pos2> = c.tessellate(16).iter().filter_map(|p| pr.project(*p).map(to_pos)).collect();
            for w in pts.windows(2) {
                painter.line_segment([w[0], w[1]], Stroke::new(1.5, col));
            }
        }
        let cps: Vec<Pos2> = app.curve_tool.points.iter().filter_map(|p| pr.project(*p).map(to_pos)).collect();
        if app.tool == Tool::CurveCv {
            for w in cps.windows(2) {
                painter.line_segment([w[0], w[1]], Stroke::new(1.0, Color32::from_rgb(140, 120, 70)));
            }
        }
        for p in &cps {
            painter.rect_filled(Rect::from_center_size(*p, egui::vec2(6.0, 6.0)), 0.0, Color32::from_rgb(190, 40, 190));
        }
    }
    if app.tool != Tool::QuadDraw {
        return;
    }
    let pts = app.qd_points();
    let proj: Vec<Option<Pos2>> = pts.iter().map(|p| pr.project(*p).map(to_pos)).collect();
    if let Some(mesh) = app.qd.target.and_then(|t| app.scene.node(t)).and_then(|n| n.mesh()) {
        // Faces drawn on top of the live surface (flat quads dip below a curved surface).
        for f in &mesh.faces {
            let poly: Option<Vec<Pos2>> = f.iter().map(|v| proj[*v as usize]).collect();
            if let Some(poly) = poly.filter(|p| p.len() >= 3) {
                painter.add(egui::Shape::convex_polygon(poly, Color32::from_rgba_unmultiplied(90, 170, 255, 70), Stroke::NONE));
            }
        }
        for f in &mesh.faces {
            for i in 0..f.len() {
                if let (Some(a), Some(b)) = (proj[f[i] as usize], proj[f[(i + 1) % f.len()] as usize]) {
                    painter.line_segment([a, b], Stroke::new(1.5, Color32::from_rgb(90, 200, 255)));
                }
            }
        }
    }
    for p in proj.iter().flatten() {
        painter.circle_filled(*p, 4.0, Color32::from_rgb(110, 255, 140));
    }
    let Some(h) = hover else { return };
    if mods.shift && pts.len() >= 3 {
        let k = if mods.ctrl { 3 } else { 4 };
        let near = app.qd_nearest_dots(h, k, &|p| pr.project(p));
        if near.len() == k {
            // Preview in angular order around the centre (screen space).
            let mut sp: Vec<Vec2> = near.iter().filter_map(|v| pr.project(pts[*v as usize])).collect();
            let c = sp.iter().copied().sum::<Vec2>() / sp.len() as f32;
            sp.sort_by(|a, b| (a.y - c.y).atan2(a.x - c.x).total_cmp(&(b.y - c.y).atan2(b.x - c.x)));
            let poly: Vec<Pos2> = sp.iter().map(|p| to_pos(*p)).collect();
            painter.add(egui::Shape::convex_polygon(poly.clone(), Color32::from_rgba_unmultiplied(110, 255, 140, 50), Stroke::new(1.5, Color32::from_rgb(110, 255, 140))));
        }
    }
    if let Some((a, b, start)) = app.qd.extend
        && let (Some(pa), Some(pb), Some(ps)) = (pr.project(pts[a as usize]), pr.project(pts[b as usize]), pr.project(start))
    {
        let d = h - ps;
        let poly = vec![to_pos(pa), to_pos(pb), to_pos(pb + d), to_pos(pa + d)];
        painter.add(egui::Shape::closed_line(poly, Stroke::new(1.5, Color32::from_rgb(255, 220, 90))));
    }
    if app.qd.live.is_none() {
        painter.text(to_pos(h) + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, "no live object: dots land on meshes / the grid (Modify › Make Live)", egui::FontId::proportional(11.0), Color32::from_rgb(255, 200, 120));
    }
}

/// One dab of the Paint Skin Weights brush at screen position `m`.
fn paint_weights(app: &mut App, pr: &Projector, m: Vec2) {
    use crate::app::PaintOp;
    let Some(id) = app.lead() else { return };
    let Some(pos) = crate::rig::deformed_positions(&app.scene, id) else { return };
    let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()).cloned() else { return };
    let eye_dir = |p: Vec3| -> Vec3 { pr.ray(pr.project(p).unwrap_or(m)).1 };
    // Front-facing vertices only (don't paint through the mesh).
    let deformed = crate::mesh::Mesh { positions: pos.clone(), faces: mesh.faces.clone(), ..Default::default() };
    let mut front = vec![false; pos.len()];
    for f in 0..deformed.face_count() {
        let c = deformed.face_center(f);
        if deformed.face_normal(f).dot(eye_dir(c)) < 0.0 {
            for &v in &deformed.faces[f] {
                front[v as usize] = true;
            }
        }
    }
    let settings = app.paint.clone();
    let mut neighbours: Vec<Vec<u32>> = Vec::new();
    if settings.op == PaintOp::Smooth {
        neighbours = vec![Vec::new(); pos.len()];
        for (a, b) in mesh.edges() {
            neighbours[a as usize].push(b);
            neighbours[b as usize].push(a);
        }
    }
    let Some(skin) = app.scene.node_mut(id).and_then(|n| n.skin.as_mut()) else { return };
    let inf = settings.influence.min(skin.joints.len().saturating_sub(1));
    let current: Vec<f32> = (0..pos.len()).map(|v| skin.weight(v, inf)).collect();
    for (v, p) in pos.iter().enumerate() {
        if !front[v] {
            continue;
        }
        let Some(sp) = pr.project(*p) else { continue };
        let d = (sp - m).length();
        if d > settings.radius {
            continue;
        }
        let falloff = 1.0 - (d / settings.radius).powi(2);
        let w = current[v];
        let nw = match settings.op {
            PaintOp::Replace => w + (settings.value - w) * (0.35 * falloff),
            PaintOp::Add => w + settings.value * 0.08 * falloff,
            PaintOp::Smooth => {
                let nb = &neighbours[v];
                if nb.is_empty() {
                    w
                } else {
                    let avg = nb.iter().map(|n| current[*n as usize]).sum::<f32>() / nb.len() as f32;
                    w + (avg - w) * 0.5 * falloff
                }
            }
        };
        if (nw - w).abs() > 1e-5 {
            skin.set_weight(v, inf, nw);
        }
    }
}

/// Nearest visible edge under the cursor: (node, edge index, edge, fraction from edge.0).
/// With `any_mesh` every visible mesh is considered (Insert Edge Loop tool), otherwise only
/// the objects in component mode.
fn pick_edge(app: &App, pr: &Projector, m: Vec2, vid: usize, any_mesh: bool) -> Option<(NodeId, usize, (u32, u32), f32)> {
    let ids: Vec<NodeId> = if any_mesh {
        app.scene.nodes.iter().filter(|n| n.mesh().is_some() && n.skin.is_none() && pickable(app, n.id)).map(|n| n.id).collect()
    } else {
        app.comp_targets()
    };
    let mut best: Option<(f32, (NodeId, usize, (u32, u32), f32))> = None;
    for id in ids {
        let w = app.scene.world_matrix(id);
        let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) else { continue };
        let (_, vf) = front_flags(app, vid, id);
        for (i, (a, b)) in mesh.edges().into_iter().enumerate() {
            if !(vf[a as usize] || vf[b as usize]) {
                continue;
            }
            let (Some(sa), Some(sb)) = (pr.project(w.transform_point3(mesh.positions[a as usize])), pr.project(w.transform_point3(mesh.positions[b as usize]))) else { continue };
            let d = seg_dist(m, sa, sb);
            if d < 9.0 && best.is_none_or(|(bd, _)| d < bd) {
                let ab = sb - sa;
                let t = ((m - sa).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.02, 0.98);
                best = Some((d, (id, i, (a, b), t)));
            }
        }
    }
    best.map(|b| b.1)
}

// ------------------------------------------------------------------ picking

/// Ray-cast for the sculpt brush: (mesh, world hit point, world face normal).
fn sculpt_hit(app: &App, pr: &Projector, m: Vec2, only: Option<NodeId>) -> Option<(NodeId, Vec3, Vec3)> {
    let (ro, rd) = pr.ray(m);
    let mut best: Option<(f32, NodeId, Vec3, Vec3)> = None;
    for n in &app.scene.nodes {
        if only.is_some_and(|o| o != n.id) || !app.scene.is_visible(n.id) {
            continue;
        }
        let Some(mesh) = n.mesh() else { continue };
        let w = app.scene.world_matrix(n.id);
        let inv = w.inverse();
        let (lo, ld) = (inv.transform_point3(ro), inv.transform_vector3(rd));
        if let Some((t, f)) = mesh.raycast(lo, ld) {
            let p = w.transform_point3(lo + ld * t);
            let d = p.distance(ro);
            if best.is_none_or(|b| d < b.0) {
                let nrm = (glam::Mat3::from_mat4(w).inverse().transpose() * mesh.face_normal(f)).normalize_or(Vec3::Y);
                best = Some((d, n.id, p, nrm));
            }
        }
    }
    best.map(|b| (b.1, b.2, b.3))
}

fn pickable(app: &App, id: NodeId) -> bool {
    app.scene.is_visible(id) && !app.scene.is_template(id)
}

fn pick_object(app: &App, pr: &Projector, m: Vec2) -> Option<NodeId> {
    let (ro, rd) = pr.ray(m);
    let mut best: Option<(f32, NodeId)> = None;
    // Wire-type objects (joints, curves, locators, IK handles) win over surfaces when the
    // cursor is on them, like Maya's joint/curve selection priority.
    let mut best_wire: Option<(f32, NodeId)> = None;
    let near_poly = |pts: &[Vec3], closed: bool, tol: f32| -> Option<f32> {
        let mut bestd: Option<f32> = None;
        let n = pts.len();
        let segs = if closed { n } else { n.saturating_sub(1) };
        for i in 0..segs {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            if let (Some(sa), Some(sb)) = (pr.project(a), pr.project(b)) {
                if seg_dist(m, sa, sb) < tol {
                    let d = (a - ro).length().min((b - ro).length());
                    bestd = Some(bestd.map_or(d, |x: f32| x.min(d)));
                }
            }
        }
        if n == 1 {
            if pr.project(pts[0]).is_some_and(|s| (s - m).length() < tol + 4.0) {
                return Some((pts[0] - ro).length());
            }
        }
        bestd
    };
    for n in &app.scene.nodes {
        if !pickable(app, n.id) {
            continue;
        }
        let w = app.scene.world_matrix(n.id);
        let origin = w.transform_point3(Vec3::ZERO);
        let mut wire = |d: Option<f32>| {
            if let Some(t) = d {
                if best_wire.is_none_or(|(bt, _)| t < bt) {
                    best_wire = Some((t, n.id));
                }
            }
        };
        match &n.shape {
            Shape::Poly { mesh, .. } => {
                let hit = match crate::rig::deformed_positions(&app.scene, n.id) {
                    Some(pos) => crate::mesh::Mesh { positions: pos, faces: mesh.faces.clone(), ..Default::default() }.raycast(ro, rd),
                    None => {
                        let inv = w.inverse();
                        mesh.raycast(inv.transform_point3(ro), inv.transform_vector3(rd))
                    }
                };
                if let Some((t, _)) = hit
                    && best.is_none_or(|(bt, _)| t < bt) {
                        best = Some((t, n.id));
                    }
            }
            Shape::Locator | Shape::IkHandle { .. } | Shape::ClusterHandle => wire(near_poly(&[origin], false, 8.0)),
            Shape::Particles(_) | Shape::Field(_) => wire(near_poly(&[origin], false, 12.0)),
            Shape::Hair(h) => {
                for st in h.strands().iter().take(400) {
                    wire(near_poly(st, false, 5.0));
                }
            }
            Shape::Light(_) | Shape::Camera(_) => wire(near_poly(&[origin], false, 14.0)),
            Shape::Lattice { div, points } => {
                for (a, b) in crate::deform::lattice_lines(*div, points) {
                    wire(near_poly(&[w.transform_point3(a), w.transform_point3(b)], false, 5.0));
                }
            }
            Shape::Joint { .. } => {
                wire(near_poly(&[origin], false, 8.0));
                for k in crate::rig::child_joints(&app.scene, n.id) {
                    let kp = crate::rig::world_pos(&app.scene, k);
                    wire(near_poly(&[origin, kp], false, 5.0));
                }
            }
            Shape::Curve { closed, .. } => {
                let c = crate::nurbs::Curve::from_shape(&n.shape).unwrap();
                let pts: Vec<Vec3> = c.tessellate(12).iter().map(|p| w.transform_point3(*p)).collect();
                wire(near_poly(&pts, *closed, 6.0));
            }
            Shape::Group => {}
        }
    }
    best_wire.or(best).map(|b| b.1)
}

fn apply_comp_pick(app: &mut App, hits: Vec<(NodeId, u32)>, mods: egui::Modifiers, replace_if_plain: bool) {
    if !mods.shift && !mods.ctrl && replace_if_plain {
        app.comp_sel.clear();
    }
    for h in hits {
        if mods.shift && mods.ctrl {
            app.comp_sel.insert(h);
        } else if mods.shift {
            if !app.comp_sel.remove(&h) {
                app.comp_sel.insert(h);
            }
        } else if mods.ctrl {
            app.comp_sel.remove(&h);
        } else {
            app.comp_sel.insert(h);
        }
    }
}

fn click_select(app: &mut App, pr: &Projector, m: Vec2, mods: egui::Modifiers, vid: usize) {
    match app.comp_mode {
        CompMode::Object => match pick_object(app, pr, m) {
            Some(id) => app.select_modify(id, mods.shift, mods.ctrl),
            None => {
                if !mods.shift && !mods.ctrl {
                    app.sel.clear();
                }
            }
        },
        CompMode::Vertex => {
            let mut best: Option<(f32, (NodeId, u32))> = None;
            for id in app.comp_targets() {
                let w = app.scene.world_matrix(id);
                let Some(pts) = app.scene.node(id).and_then(|n| n.points()) else { continue };
                let (_, vf) = front_flags(app, vid, id);
                for (i, p) in pts.iter().enumerate() {
                    if !vf.get(i).copied().unwrap_or(true) {
                        continue;
                    }
                    if let Some(s) = pr.project(w.transform_point3(*p)) {
                        let dd = (s - m).length();
                        if dd < 12.0 && best.is_none_or(|(b, _)| dd < b) {
                            best = Some((dd, (id, i as u32)));
                        }
                    }
                }
            }
            apply_comp_pick(app, best.map(|b| vec![b.1]).unwrap_or_default(), mods, true);
        }
        CompMode::Edge => {
            let hit = pick_edge(app, pr, m, vid, false).map(|h| vec![(h.0, h.1 as u32)]).unwrap_or_default();
            apply_comp_pick(app, hit, mods, true);
        }
        CompMode::Face => {
            let (ro, rd) = pr.ray(m);
            let mut best: Option<(f32, (NodeId, u32))> = None;
            for id in app.comp_targets() {
                let inv = app.scene.world_matrix(id).inverse();
                let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) else { continue };
                if let Some((t, f)) = mesh.raycast(inv.transform_point3(ro), inv.transform_vector3(rd))
                    && best.is_none_or(|(b, _)| t < b) {
                        best = Some((t, (id, f as u32)));
                    }
            }
            apply_comp_pick(app, best.map(|b| vec![b.1]).unwrap_or_default(), mods, true);
        }
    }
}

fn marquee_select(app: &mut App, pr: &Projector, r: Rect, mods: egui::Modifiers) {
    let inside = |p: Vec3| pr.project(p).is_some_and(|s| r.contains(to_pos(s)));
    match app.comp_mode {
        CompMode::Object => {
            let mut hits = Vec::new();
            for n in &app.scene.nodes {
                if !pickable(app, n.id) || matches!(n.shape, Shape::Group) {
                    continue;
                }
                let w = app.scene.world_matrix(n.id);
                let hit = match n.mesh() {
                    Some(mesh) => mesh.positions.iter().any(|p| inside(w.transform_point3(*p))),
                    None => inside(w.transform_point3(Vec3::ZERO)),
                };
                if hit {
                    hits.push(n.id);
                }
            }
            if !mods.shift && !mods.ctrl {
                app.sel = hits;
            } else {
                for id in hits {
                    app.select_modify(id, mods.shift, mods.ctrl);
                }
            }
        }
        CompMode::Vertex | CompMode::Face | CompMode::Edge => {
            let mut hits = Vec::new();
            for id in app.comp_targets() {
                let w = app.scene.world_matrix(id);
                if app.comp_mode == CompMode::Vertex {
                    if let Some(pts) = app.scene.node(id).and_then(|n| n.points()) {
                        for (i, p) in pts.iter().enumerate() {
                            if inside(w.transform_point3(*p)) {
                                hits.push((id, i as u32));
                            }
                        }
                    }
                    continue;
                }
                let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) else { continue };
                if app.comp_mode == CompMode::Edge {
                    for (i, (a, b)) in mesh.edges().into_iter().enumerate() {
                        if inside(w.transform_point3(mesh.positions[a as usize])) && inside(w.transform_point3(mesh.positions[b as usize])) {
                            hits.push((id, i as u32));
                        }
                    }
                } else if app.comp_mode == CompMode::Vertex {
                    for (i, p) in mesh.positions.iter().enumerate() {
                        if inside(w.transform_point3(*p)) {
                            hits.push((id, i as u32));
                        }
                    }
                } else {
                    for f in 0..mesh.face_count() {
                        if inside(w.transform_point3(mesh.face_center(f))) {
                            hits.push((id, f as u32));
                        }
                    }
                }
            }
            apply_comp_pick(app, hits, mods, true);
        }
    }
}

// ------------------------------------------------------------------ drawing

/// Per-face and per-vertex "faces the camera" flags (for component display / picking).
fn front_flags(app: &App, vid: usize, id: NodeId) -> (Vec<bool>, Vec<bool>) {
    if let Some(Shape::Lattice { points, .. } | Shape::Curve { points, .. }) = app.scene.node(id).map(|n| &n.shape) {
        return (Vec::new(), vec![true; points.len()]);
    }
    let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) else { return (Vec::new(), Vec::new()) };
    let w = app.scene.world_matrix(id);
    let nm = glam::Mat3::from_mat4(w).inverse().transpose();
    let cam = &app.views[vid].cam;
    let mut vf = vec![false; mesh.vertex_count()];
    let mut ff = Vec::with_capacity(mesh.face_count());
    for f in 0..mesh.face_count() {
        let c = w.transform_point3(mesh.face_center(f));
        let n = nm * mesh.face_normal(f);
        let to_cam = if cam.is_ortho() { cam.back_dir() } else { cam.eye() - c };
        let front = n.dot(to_cam) > 0.0;
        ff.push(front);
        if front {
            for &v in &mesh.faces[f] {
                vf[v as usize] = true;
            }
        }
    }
    (ff, vf)
}

fn draw_components(app: &App, painter: &egui::Painter, pr: &Projector, vid: usize) {
    if app.comp_mode == CompMode::Object {
        return;
    }
    let soft: std::collections::HashMap<(NodeId, u32), f32> =
        if app.soft_select && app.comp_mode != CompMode::Object { app.soft_weights().into_iter().map(|(n, v, w)| ((n, v), w)).collect() } else { Default::default() };
    for id in app.comp_targets() {
        let w = app.scene.world_matrix(id);
        if app.comp_mode == CompMode::Vertex {
            let Some(pts) = app.scene.node(id).and_then(|n| n.points()) else { continue };
            // Curve hull (lines between CVs).
            if let Some(Shape::Curve { closed, degree, .. }) = app.scene.node(id).map(|n| &n.shape)
                && *degree > 1
            {
                let sp: Vec<Option<Pos2>> = pts.iter().map(|p| pr.project(w.transform_point3(*p)).map(to_pos)).collect();
                let n = sp.len();
                let segs = if *closed { n } else { n.saturating_sub(1) };
                for i in 0..segs {
                    if let (Some(a), Some(b)) = (sp[i], sp[(i + 1) % n]) {
                        painter.line_segment([a, b], Stroke::new(1.0, Color32::from_rgb(150, 90, 150)));
                    }
                }
            }
            let (_, vf) = front_flags(app, vid, id);
            for (i, p) in pts.iter().enumerate() {
                let sel = app.comp_sel.contains(&(id, i as u32));
                if !vf.get(i).copied().unwrap_or(true) && !sel {
                    continue;
                }
                if let Some(s) = pr.project(w.transform_point3(*p)) {
                    let (sz, col) = if sel {
                        (3.5, Color32::from_rgb(255, 240, 30))
                    } else if let Some(wt) = soft.get(&(id, i as u32)) {
                        // Soft selection: yellow (1) → red → purple (0).
                        (3.0, Color32::from_rgb(230, (60.0 + 180.0 * wt) as u8, (40.0 * (1.0 - wt)) as u8))
                    } else {
                        (2.5, Color32::from_rgb(190, 40, 190))
                    };
                    painter.rect_filled(Rect::from_center_size(to_pos(s), egui::vec2(sz * 2.0, sz * 2.0)), 0.0, col);
                }
            }
            continue;
        }
        let Some(mesh) = app.scene.node(id).and_then(|n| n.mesh()) else { continue };
        let (ff, vf) = front_flags(app, vid, id);
        match app.comp_mode {
            CompMode::Vertex => {}
            CompMode::Edge => {
                for (i, (a, b)) in mesh.edges().into_iter().enumerate() {
                    let sel = app.comp_sel.contains(&(id, i as u32));
                    if !sel && !(vf[a as usize] && vf[b as usize]) {
                        continue;
                    }
                    if let (Some(sa), Some(sb)) = (pr.project(w.transform_point3(mesh.positions[a as usize])), pr.project(w.transform_point3(mesh.positions[b as usize]))) {
                        let st = if sel { Stroke::new(2.5, Color32::from_rgb(255, 150, 40)) } else { Stroke::new(1.0, Color32::from_rgb(120, 170, 255)) };
                        painter.line_segment([to_pos(sa), to_pos(sb)], st);
                    }
                }
            }
            CompMode::Face => {
                for f in 0..mesh.face_count() {
                    if !ff[f] && !app.comp_sel.contains(&(id, f as u32)) {
                        continue;
                    }
                    if let Some(s) = pr.project(w.transform_point3(mesh.face_center(f))) {
                        let sel = app.comp_sel.contains(&(id, f as u32));
                        let col = if sel { Color32::from_rgb(255, 150, 60) } else { Color32::from_rgb(70, 110, 220) };
                        painter.circle_filled(to_pos(s), 2.5, col);
                    }
                }
            }
            CompMode::Object => {}
        }
    }
}

fn draw_gizmo(app: &App, painter: &egui::Painter, vid: usize, vp: Rect, pr: &Projector) {
    if !app.tool.has_gizmo() || !has_transform_target(app) {
        return;
    }
    let Some(pivot) = app.pivot() else { return };
    let Some(ps) = pr.project(pivot) else { return };
    let len = gizmo_len(app, vid, pivot, vp);
    let axes = gizmo_axes(app, app.tool);
    let active = match &app.interact.drag {
        Some(Drag { kind: DragKind::Gizmo { handle, .. }, .. }) => Some(*handle),
        _ if app.interact.hover_view == Some(vid) => app.interact.hover,
        _ => None,
    };
    let col = |h: Handle, base: Color32| if active == Some(h) { ACTIVE } else { base };
    let ps_p = to_pos(ps);
    match app.tool {
        Tool::Move | Tool::Scale => {
            for (i, ax) in axes.iter().enumerate() {
                let Some(tip) = pr.project(pivot + *ax * len) else { continue };
                let c = col(Handle::Axis(i), AXIS_COLORS[i]);
                let tip_p = to_pos(tip);
                painter.line_segment([ps_p, tip_p], Stroke::new(2.0, c));
                let dir = (tip - ps).normalize_or_zero();
                if app.tool == Tool::Move {
                    let perp = Vec2::new(-dir.y, dir.x);
                    let base = tip - dir * 14.0;
                    let pts = vec![to_pos(tip + dir * 2.0), to_pos(base + perp * 5.0), to_pos(base - perp * 5.0)];
                    painter.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
                } else {
                    painter.rect_filled(Rect::from_center_size(tip_p, egui::vec2(9.0, 9.0)), 0.0, c);
                }
            }
            let c = col(Handle::Center, Color32::from_rgb(90, 230, 230));
            if app.tool == Tool::Move {
                painter.rect_stroke(Rect::from_center_size(ps_p, egui::vec2(12.0, 12.0)), 0.0, Stroke::new(2.0, c), StrokeKind::Middle);
            } else {
                painter.rect_filled(Rect::from_center_size(ps_p, egui::vec2(11.0, 11.0)), 0.0, c);
            }
        }
        Tool::Rotate => {
            let to_cam = to_cam_dir(app, vid, pivot);
            // Outer silhouette circle.
            painter.circle_stroke(ps_p, GIZMO_PX * 1.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 200, 200, 90)));
            for (i, ax) in axes.iter().enumerate() {
                let c = col(Handle::Axis(i), AXIS_COLORS[i]);
                for run in ring_points(pivot, *ax, len, to_cam, pr) {
                    painter.add(egui::Shape::line(run.into_iter().map(to_pos).collect(), Stroke::new(2.0, c)));
                }
            }
            if let Some(Drag { kind: DragKind::Gizmo { .. }, angle_acc, .. }) = &app.interact.drag {
                painter.text(
                    ps_p + egui::vec2(0.0, -GIZMO_PX - 14.0),
                    egui::Align2::CENTER_CENTER,
                    format!("{:.1}°", angle_acc.to_degrees().abs()),
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        }
        _ => {}
    }
}

fn draw_hud(app: &App, painter: &egui::Painter, vid: usize, vp: Rect, pr: &Projector) {
    let view = &app.views[vid];
    let text = Color32::from_rgb(210, 210, 210);
    // Camera name.
    painter.text(
        Pos2::new(vp.center().x, vp.max.y - 14.0),
        egui::Align2::CENTER_CENTER,
        view.look_through.and_then(|c| app.scene.node(c)).map(|n| n.name.as_str()).unwrap_or(view.cam.kind.name()),
        egui::FontId::proportional(13.0),
        text,
    );
    // Axis triad.
    let o = Pos2::new(vp.min.x + 34.0, vp.max.y - 34.0);
    let vm = view.cam.view();
    for (i, (ax, label)) in [(Vec3::X, "X"), (Vec3::Y, "Y"), (Vec3::Z, "Z")].into_iter().enumerate() {
        let d = vm.transform_vector3(ax);
        let tip = o + egui::vec2(d.x, -d.y) * 22.0;
        painter.line_segment([o, tip], Stroke::new(2.0, AXIS_COLORS[i]));
        painter.text(tip + egui::vec2(d.x, -d.y) * 6.0, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), AXIS_COLORS[i]);
    }
    if !view.show_hud {
        return;
    }
    // Poly count HUD (Display > Heads Up Display > Poly Count).
    let (mut v, mut e, mut f, mut t) = (0usize, 0usize, 0usize, 0usize);
    let (mut sv, mut se, mut sf, mut st) = (0usize, 0usize, 0usize, 0usize);
    for n in &app.scene.nodes {
        if !app.scene.is_visible(n.id) {
            continue;
        }
        if let Some(m) = n.mesh() {
            let ec = m.edges().len();
            v += m.vertex_count();
            e += ec;
            f += m.face_count();
            t += m.tri_count();
            if app.sel.contains(&n.id) {
                sv += m.vertex_count();
                se += ec;
                sf += m.face_count();
                st += m.tri_count();
            }
        }
    }
    let lines = [
        format!("Verts:    {v:>7} {sv:>7}"),
        format!("Edges:    {e:>7} {se:>7}"),
        format!("Faces:    {f:>7} {sf:>7}"),
        format!("Tris:     {t:>7} {st:>7}"),
    ];
    for (i, l) in lines.iter().enumerate() {
        painter.text(
            vp.min + egui::vec2(10.0, 10.0 + i as f32 * 15.0),
            egui::Align2::LEFT_TOP,
            l,
            egui::FontId::monospace(11.5),
            text,
        );
    }
    // Current frame HUD.
    painter.text(
        Pos2::new(vp.max.x - 10.0, vp.min.y + 10.0),
        egui::Align2::RIGHT_TOP,
        format!("Frame {:.0}   {}", app.scene.time, match app.comp_mode {
            CompMode::Object => "",
            CompMode::Vertex => "| Vertex",
            CompMode::Face => "| Face",
            CompMode::Edge => "| Edge",
        }),
        egui::FontId::proportional(12.0),
        text,
    );
    // Locator labels / view cube substitute: name of lead object near pivot.
    if let Some(id) = app.lead()
        && let (Some(n), Some(p)) = (app.scene.node(id), app.pivot())
            && app.tool == Tool::Select
                && let Some(s) = pr.project(p) {
                    painter.text(to_pos(s) + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, &n.name, egui::FontId::proportional(11.0), Color32::from_rgb(160, 230, 190));
                }
}
