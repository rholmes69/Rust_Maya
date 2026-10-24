//! Animation editors: Graph Editor (key selection, tangent handles, tangent types), Dope Sheet,
//! Pose Library and the Retarget window.

use crate::anim_ops::{auto_map, skeleton, KeyRef};
use crate::app::App;
use crate::scene::{Channel, NodeId, TanKind};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke};

fn channel_color(c: Channel) -> Color32 {
    match c {
        Channel::TranslateX | Channel::RotateX | Channel::ScaleX => Color32::from_rgb(235, 70, 70),
        Channel::TranslateY | Channel::RotateY | Channel::ScaleY => Color32::from_rgb(90, 220, 80),
        Channel::TranslateZ | Channel::RotateZ | Channel::ScaleZ => Color32::from_rgb(80, 130, 240),
        Channel::Visibility => Color32::from_rgb(220, 220, 220),
    }
}

#[derive(Clone)]
enum GDrag {
    Keys { orig: Vec<(KeyRef, f32)>, start: (f32, f32) },
    Tangent { key: KeyRef, out: bool, broken: bool },
    Marquee(Pos2, Pos2),
}

#[derive(Default)]
pub struct GraphState {
    drag: Option<GDrag>,
    pub hidden: std::collections::HashSet<(NodeId, Channel)>,
}

fn is_sel(app: &App, k: &KeyRef) -> bool {
    app.key_sel.iter().any(|s| s.node == k.node && s.ch == k.ch && (s.frame - k.frame).abs() < 1e-4)
}

fn tangent_toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.label(RichText::new("Tangents:").strong());
    for k in TanKind::MENU {
        if ui.button(k.name()).on_hover_text("Set the tangent type of the selected keys (all keys of the selection if none)").clicked() {
            let n = app.set_key_tangents(k);
            app.info(format!("{n} key(s) set to {} tangents", k.name()));
        }
    }
    ui.separator();
    if ui.button("Delete keys").on_hover_text("Delete the selected keys (Del)").clicked() {
        app.delete_selected_keys();
    }
    if ui.button("Copy").clicked() {
        let n = app.copy_keys();
        app.info(format!("{n} key(s) copied"));
    }
    if ui.button("Paste").on_hover_text("Paste at the current frame").clicked() {
        let t = app.scene.time;
        let n = app.paste_keys(t);
        app.info(format!("{n} key(s) pasted at frame {t}"));
    }
}

pub fn graph_editor(app: &mut App, ctx: &egui::Context, st: &mut GraphState) {
    let mut open = app.panels.graph_editor;
    egui::Window::new("Graph Editor").open(&mut open).default_size([820.0, 360.0]).default_pos([300.0, 500.0]).show(ctx, |ui| {
        let curves: Vec<(NodeId, String, Channel)> = app
            .sel
            .iter()
            .filter_map(|id| app.scene.node(*id))
            .flat_map(|n| n.anim.keys().map(move |c| (n.id, n.name.clone(), *c)))
            .collect();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Frame (F)").clicked() {
                app.graph_view = None;
            }
            tangent_toolbar(app, ui);
        });
        ui.label(
            RichText::new("Click/Shift-click/drag-box keys · drag keys to move · drag the yellow handles to edit tangents (Shift breaks) · Ctrl+click a curve to add a key · MMB/RMB pan · wheel zoom (Ctrl: value only) · Del deletes")
                .small()
                .weak(),
        );
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(190.0, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(190.0);
                egui::ScrollArea::vertical().id_salt("ge_list").show(ui, |ui| {
                    if curves.is_empty() {
                        ui.label(RichText::new("Select animated objects").weak());
                    }
                    for (id, name, c) in &curves {
                        let mut shown = !st.hidden.contains(&(*id, *c));
                        if ui.checkbox(&mut shown, RichText::new(format!("{name}.{}", c.long())).color(channel_color(*c))).changed() {
                            if shown {
                                st.hidden.remove(&(*id, *c));
                            } else {
                                st.hidden.insert((*id, *c));
                            }
                        }
                    }
                });
            });
            ui.separator();
            graph_canvas(app, ui, st, &curves);
        });
    });
    app.panels.graph_editor = open;
}

fn graph_canvas(app: &mut App, ui: &mut egui::Ui, st: &mut GraphState, curves: &[(NodeId, String, Channel)]) {
    let size = ui.available_size().max(egui::vec2(100.0, 100.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_rgb(52, 52, 52));
    let visible: Vec<(NodeId, Channel)> = curves.iter().filter(|(id, _, c)| !st.hidden.contains(&(*id, *c))).map(|(i, _, c)| (*i, *c)).collect();
    let view = app.graph_view.unwrap_or_else(|| {
        let (mut t0, mut t1) = (app.scene.play_start, app.scene.play_end);
        let (mut v0, mut v1) = (f32::MAX, f32::MIN);
        for (id, c) in &visible {
            if let Some(cv) = app.scene.node(*id).and_then(|n| n.anim.get(c)) {
                for k in &cv.keys {
                    t0 = t0.min(k.frame);
                    t1 = t1.max(k.frame);
                    v0 = v0.min(k.value);
                    v1 = v1.max(k.value);
                }
            }
        }
        if v0 > v1 {
            v0 = -1.0;
            v1 = 1.0;
        }
        let pad = ((v1 - v0) * 0.15).max(0.5);
        (t0 - 2.0, t1 + 2.0, v0 - pad, v1 + pad)
    });
    let (t0, t1, v0, v1) = view;
    let to_s = |t: f32, v: f32| Pos2::new(rect.min.x + (t - t0) / (t1 - t0) * rect.width(), rect.max.y - (v - v0) / (v1 - v0) * rect.height());
    let from_s = |s: Pos2| (t0 + (s.x - rect.min.x) / rect.width() * (t1 - t0), v0 + (rect.max.y - s.y) / rect.height() * (v1 - v0));
    // Pixels per unit (for tangent handle directions).
    let (px_t, px_v) = (rect.width() / (t1 - t0), rect.height() / (v1 - v0));
    let nice = |range: f32| {
        let raw = range / 8.0;
        let mag = 10f32.powf(raw.log10().floor());
        [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag)
    };
    let (ts, vs) = (nice(t1 - t0), nice(v1 - v0));
    let mut t = (t0 / ts).ceil() * ts;
    while t <= t1 {
        let x = to_s(t, 0.0).x;
        p.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], Stroke::new(1.0, Color32::from_rgb(64, 64, 64)));
        p.text(Pos2::new(x + 2.0, rect.max.y - 2.0), egui::Align2::LEFT_BOTTOM, format!("{t:.0}"), egui::FontId::proportional(10.0), Color32::GRAY);
        t += ts;
    }
    let mut v = (v0 / vs).ceil() * vs;
    while v <= v1 {
        let y = to_s(0.0, v).y;
        let col = if v.abs() < vs * 0.01 { Color32::from_rgb(90, 90, 90) } else { Color32::from_rgb(64, 64, 64) };
        p.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], Stroke::new(1.0, col));
        p.text(Pos2::new(rect.min.x + 2.0, y - 1.0), egui::Align2::LEFT_BOTTOM, format!("{v:.2}"), egui::FontId::proportional(10.0), Color32::GRAY);
        v += vs;
    }
    let xt = to_s(app.scene.time, 0.0).x;
    p.line_segment([Pos2::new(xt, rect.min.y), Pos2::new(xt, rect.max.y)], Stroke::new(1.5, Color32::from_rgb(230, 60, 60)));

    // Curves, keys and tangent handles of selected keys.
    let mouse = resp.hover_pos();
    let mut hover_key: Option<KeyRef> = None;
    let mut handles: Vec<(KeyRef, bool, Pos2)> = Vec::new();
    let mut hover_curve: Option<(NodeId, Channel, f32)> = None;
    let hlen = 45.0;
    for (id, c) in &visible {
        let Some(cv) = app.scene.node(*id).and_then(|n| n.anim.get(c)) else { continue };
        let col = channel_color(*c);
        let steps = rect.width().max(50.0) as usize / 3;
        let pts: Vec<Pos2> = (0..=steps).map(|i| {
            let t = t0 + (t1 - t0) * i as f32 / steps as f32;
            to_s(t, cv.evaluate(t).unwrap_or(0.0))
        }).collect();
        if let Some(m) = mouse
            && pts.windows(2).any(|w| crate::ui::uv_dist(m, w[0], w[1]) < 4.0)
        {
            hover_curve = Some((*id, *c, from_s(m).0.round()));
        }
        p.add(egui::Shape::line(pts, Stroke::new(1.5, col)));
        for (ki, k) in cv.keys.iter().enumerate() {
            let s = to_s(k.frame, k.value);
            let kr = KeyRef { node: *id, ch: *c, frame: k.frame };
            let sel = is_sel(app, &kr);
            if sel {
                // Tangent handles in screen space.
                for (out, slope) in [(false, cv.in_slope(ki)), (true, cv.out_slope(ki))] {
                    let d = egui::vec2(px_t, -slope * px_v).normalized() * hlen * if out { 1.0 } else { -1.0 };
                    let hp = s + d;
                    p.line_segment([s, hp], Stroke::new(1.0, Color32::from_rgb(200, 200, 120)));
                    p.circle_filled(hp, 3.5, Color32::from_rgb(240, 220, 90));
                    handles.push((kr, out, hp));
                }
            }
            let hov = mouse.is_some_and(|m| m.distance(s) < 6.0);
            if hov {
                hover_key = Some(kr);
            }
            let fill = if sel { Color32::YELLOW } else if hov { Color32::from_rgb(255, 240, 160) } else { Color32::BLACK };
            p.rect_filled(Rect::from_center_size(s, egui::vec2(6.0, 6.0)), 0.0, fill);
            p.rect_stroke(Rect::from_center_size(s, egui::vec2(6.0, 6.0)), 0.0, Stroke::new(1.0, col), egui::StrokeKind::Middle);
        }
    }

    // ---- interaction
    let (wheel, mods) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers));
    let press = ui.input(|i| i.pointer.press_origin());
    if resp.drag_started_by(egui::PointerButton::Primary) {
        let at = press.unwrap_or(rect.center());
        let near_handle = handles.iter().filter(|h| h.2.distance(at) < 7.0).min_by(|a, b| a.2.distance(at).total_cmp(&b.2.distance(at))).map(|h| (h.0, h.1));
        let key_at = |app: &App| {
            visible.iter().find_map(|(id, c)| {
                let cv = app.scene.node(*id)?.anim.get(c)?;
                cv.keys.iter().find(|k| to_s(k.frame, k.value).distance(at) < 6.0).map(|k| KeyRef { node: *id, ch: *c, frame: k.frame })
            })
        };
        st.drag = if let Some((k, out)) = near_handle {
            app.checkpoint();
            Some(GDrag::Tangent { key: k, out, broken: mods.shift })
        } else if let Some(k) = key_at(app) {
            if !is_sel(app, &k) {
                if !mods.shift {
                    app.key_sel.clear();
                }
                app.key_sel.push(k);
            }
            app.checkpoint();
            let orig = app.key_sel.iter().filter_map(|k| app.key_value(k).map(|v| (*k, v))).collect();
            Some(GDrag::Keys { orig, start: from_s(at) })
        } else {
            Some(GDrag::Marquee(at, at))
        };
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(m) = resp.interact_pointer_pos()
    {
        match &mut st.drag {
            Some(GDrag::Keys { orig, start }) => {
                let (t, v) = from_s(m);
                let (dt, dv) = (t - start.0, if mods.ctrl { 0.0 } else { v - start.1 });
                let o = orig.clone();
                app.move_keys(&o, dt, dv);
            }
            Some(GDrag::Tangent { key, out, broken }) => {
                if let Some(cv) = app.scene.node_mut(key.node).and_then(|n| n.anim.get_mut(&key.ch))
                    && let Some(i) = cv.keys.iter().position(|k| (k.frame - key.frame).abs() < 1e-4)
                {
                    let ks = to_s(cv.keys[i].frame, cv.keys[i].value);
                    let mut d = m - ks;
                    if !*out {
                        d = -d;
                    }
                    if d.x.abs() < 1.0 {
                        d.x = 1.0;
                    }
                    let slope = (-d.y / px_v) / (d.x / px_t);
                    let (mut inn, mut outs) = (cv.in_slope(i), cv.out_slope(i));
                    if *broken {
                        if *out { outs = slope } else { inn = slope }
                    } else {
                        inn = slope;
                        outs = slope;
                    }
                    cv.set_key_slopes(i, inn, outs);
                }
                app.scene.evaluate();
            }
            Some(GDrag::Marquee(_, cur)) => *cur = m,
            None => {}
        }
    }
    if let Some(GDrag::Marquee(a, b)) = &st.drag {
        let r = Rect::from_two_pos(*a, *b);
        p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
    }
    if resp.drag_stopped() {
        if let Some(GDrag::Marquee(a, b)) = st.drag.take() {
            let r = Rect::from_two_pos(a, b);
            if !mods.shift {
                app.key_sel.clear();
            }
            for (id, c) in &visible {
                if let Some(cv) = app.scene.node(*id).and_then(|n| n.anim.get(c)) {
                    for k in &cv.keys {
                        if r.contains(to_s(k.frame, k.value)) {
                            app.key_sel.push(KeyRef { node: *id, ch: *c, frame: k.frame });
                        }
                    }
                }
            }
        }
        st.drag = None;
    }
    if resp.clicked() {
        if let Some(k) = hover_key {
            if mods.shift {
                if is_sel(app, &k) {
                    app.key_sel.retain(|s| !(s.node == k.node && s.ch == k.ch && (s.frame - k.frame).abs() < 1e-4));
                } else {
                    app.key_sel.push(k);
                }
            } else {
                app.key_sel = vec![k];
            }
        } else if mods.ctrl && let Some((id, c, f)) = hover_curve {
            // Insert a key on the curve, keeping its shape.
            app.checkpoint();
            if let Some(cv) = app.scene.node_mut(id).and_then(|n| n.anim.get_mut(&c)) {
                let v = cv.evaluate(f).unwrap_or(0.0);
                cv.set_key(f, v);
            }
            app.key_sel = vec![KeyRef { node: id, ch: c, frame: f }];
        } else if let Some(m) = resp.interact_pointer_pos() {
            if !mods.shift {
                app.key_sel.clear();
            }
            app.set_time(from_s(m).0.round());
        }
    }
    if resp.hovered() && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
        app.delete_selected_keys();
    }
    let mut nv = view;
    if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
        let d = resp.drag_delta();
        let dt = -d.x / rect.width() * (t1 - t0);
        let dv = d.y / rect.height() * (v1 - v0);
        nv = (t0 + dt, t1 + dt, v0 + dv, v1 + dv);
    }
    if resp.hovered() && wheel != 0.0 {
        let f = (-wheel * 0.003).exp();
        let (mt, mv) = mouse.map(from_s).unwrap_or(((t0 + t1) / 2.0, (v0 + v1) / 2.0));
        nv = if mods.ctrl { (t0, t1, mv + (v0 - mv) * f, mv + (v1 - mv) * f) } else { (mt + (t0 - mt) * f, mt + (t1 - mt) * f, mv + (v0 - mv) * f, mv + (v1 - mv) * f) };
    }
    if resp.hovered() && ui.input(|i| i.key_pressed(egui::Key::F)) {
        app.graph_view = None;
    } else if nv != view || app.graph_view.is_some() {
        app.graph_view = Some(nv);
    }
}

// ------------------------------------------------------------------- Dope Sheet

#[derive(Default)]
pub struct DopeState {
    drag: Option<(Vec<(KeyRef, f32)>, f32)>,
    marquee: Option<(Pos2, Pos2)>,
    view: Option<(f32, f32)>,
}

pub fn dope_sheet(app: &mut App, ctx: &egui::Context, st: &mut DopeState) {
    let mut open = app.panels_dope_sheet;
    egui::Window::new("Dope Sheet").open(&mut open).default_size([820.0, 300.0]).default_pos([300.0, 560.0]).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| tangent_toolbar(app, ui));
        ui.label(RichText::new("Click/drag-box keys · drag to retime (whole frames) · summary rows move all channels · click the ruler to set the time · Del deletes").small().weak());
        // Rows: per object a summary row, then its channels.
        let mut rows: Vec<(String, NodeId, Option<Channel>)> = Vec::new();
        for &id in &app.sel {
            let Some(n) = app.scene.node(id) else { continue };
            if n.anim.is_empty() {
                continue;
            }
            rows.push((n.name.clone(), id, None));
            for c in n.anim.keys() {
                rows.push((format!("   {}", c.long()), id, Some(*c)));
            }
        }
        let row_h = 18.0;
        let label_w = 170.0;
        let ruler_h = 20.0;
        let height = (ruler_h + rows.len().max(1) as f32 * row_h + 4.0).max(80.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, Color32::from_rgb(52, 52, 52));
        let area = Rect::from_min_max(Pos2::new(rect.min.x + label_w, rect.min.y), rect.max);
        let (f0, f1) = st.view.unwrap_or((app.scene.play_start - 1.0, app.scene.play_end + 1.0));
        let to_x = |f: f32| area.min.x + (f - f0) / (f1 - f0) * area.width();
        let from_x = |x: f32| f0 + (x - area.min.x) / area.width() * (f1 - f0);
        // Ruler.
        let step = ((f1 - f0) / 20.0).max(1.0).ceil();
        let mut f = (f0 / step).ceil() * step;
        while f <= f1 {
            let x = to_x(f);
            p.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], Stroke::new(1.0, Color32::from_rgb(64, 64, 64)));
            p.text(Pos2::new(x + 2.0, rect.min.y + 2.0), egui::Align2::LEFT_TOP, format!("{f:.0}"), egui::FontId::proportional(10.0), Color32::GRAY);
            f += step;
        }
        let xt = to_x(app.scene.time);
        p.line_segment([Pos2::new(xt, rect.min.y), Pos2::new(xt, rect.max.y)], Stroke::new(1.5, Color32::from_rgb(230, 60, 60)));
        if rows.is_empty() {
            p.text(area.center(), egui::Align2::CENTER_CENTER, "Select animated objects", egui::FontId::proportional(13.0), Color32::GRAY);
        }
        // Keys.
        let mouse = resp.hover_pos();
        let mut hover: Option<Vec<KeyRef>> = None;
        let mut all_keys: Vec<(KeyRef, Pos2)> = Vec::new();
        for (ri, (label, id, ch)) in rows.iter().enumerate() {
            let y = rect.min.y + ruler_h + ri as f32 * row_h + row_h * 0.5;
            if ri % 2 == 0 {
                p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, y - row_h * 0.5), Pos2::new(rect.max.x, y + row_h * 0.5)), 0.0, Color32::from_rgb(56, 56, 56));
            }
            p.text(Pos2::new(rect.min.x + 4.0, y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.0), if ch.is_none() { Color32::WHITE } else { ch.map(channel_color).unwrap() });
            let Some(n) = app.scene.node(*id) else { continue };
            let keys: Vec<(f32, Vec<KeyRef>)> = match ch {
                Some(c) => n.anim.get(c).map(|cv| cv.keys.iter().map(|k| (k.frame, vec![KeyRef { node: *id, ch: *c, frame: k.frame }])).collect()).unwrap_or_default(),
                None => {
                    let mut m: std::collections::BTreeMap<i64, Vec<KeyRef>> = Default::default();
                    for (c, cv) in &n.anim {
                        for k in &cv.keys {
                            m.entry((k.frame * 1000.0).round() as i64).or_default().push(KeyRef { node: *id, ch: *c, frame: k.frame });
                        }
                    }
                    m.into_iter().map(|(f, v)| (f as f32 / 1000.0, v)).collect()
                }
            };
            for (frame, refs) in keys {
                let c = Pos2::new(to_x(frame), y);
                if !area.contains(c) {
                    continue;
                }
                let sel = refs.iter().all(|r| is_sel(app, r));
                let hov = mouse.is_some_and(|m| m.distance(c) < 6.0);
                if hov {
                    hover = Some(refs.clone());
                }
                let r = 5.0;
                let pts = vec![c + egui::vec2(0.0, -r), c + egui::vec2(r, 0.0), c + egui::vec2(0.0, r), c + egui::vec2(-r, 0.0)];
                let fill = if sel { Color32::YELLOW } else if ch.is_none() { Color32::from_rgb(200, 200, 200) } else { Color32::from_rgb(170, 170, 170) };
                p.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.0, if hov { Color32::WHITE } else { Color32::BLACK })));
                if ch.is_some() {
                    all_keys.push((refs[0], c));
                }
            }
        }
        // Interaction.
        let mods = ui.input(|i| i.modifiers);
        let press = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let at = press.unwrap_or(rect.center());
            if let Some(refs) = hover.clone().filter(|_| mouse.is_some_and(|m| m.distance(at) < 10.0)) {
                if !refs.iter().all(|r| is_sel(app, r)) {
                    if !mods.shift {
                        app.key_sel.clear();
                    }
                    app.key_sel.extend(refs);
                }
                app.checkpoint();
                let orig = app.key_sel.iter().filter_map(|k| app.key_value(k).map(|v| (*k, v))).collect();
                st.drag = Some((orig, from_x(at.x)));
            } else {
                st.marquee = Some((at, at));
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(m) = resp.interact_pointer_pos()
        {
            if let Some((orig, start)) = &st.drag {
                let o = orig.clone();
                app.move_keys(&o, from_x(m.x) - start, 0.0);
            }
            if let Some((_, b)) = &mut st.marquee {
                *b = m;
            }
        }
        if let Some((a, b)) = st.marquee {
            p.rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
        }
        if resp.drag_stopped() {
            if let Some((a, b)) = st.marquee.take() {
                let r = Rect::from_two_pos(a, b);
                if !mods.shift {
                    app.key_sel.clear();
                }
                for (k, c) in &all_keys {
                    if r.contains(*c) {
                        app.key_sel.push(*k);
                    }
                }
            }
            st.drag = None;
        }
        if resp.clicked() {
            match hover {
                Some(refs) => {
                    if !mods.shift {
                        app.key_sel.clear();
                    }
                    app.key_sel.extend(refs);
                }
                None => {
                    if let Some(m) = resp.interact_pointer_pos()
                        && m.x > area.min.x
                    {
                        if !mods.shift {
                            app.key_sel.clear();
                        }
                        app.set_time(from_x(m.x).round());
                    }
                }
            }
        }
        if resp.hovered() {
            let (wheel, del) = ui.input(|i| (i.smooth_scroll_delta.y, i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)));
            if del {
                app.delete_selected_keys();
            }
            if wheel != 0.0 {
                let fm = mouse.map(|m| from_x(m.x)).unwrap_or((f0 + f1) / 2.0);
                let k = (-wheel * 0.003).exp();
                st.view = Some((fm + (f0 - fm) * k, fm + (f1 - fm) * k));
            }
            if ui.input(|i| i.key_pressed(egui::Key::F)) {
                st.view = None;
            }
        }
        if resp.dragged_by(egui::PointerButton::Middle) {
            let d = resp.drag_delta().x / area.width() * (f1 - f0);
            st.view = Some((f0 - d, f1 - d));
        }
    });
    app.panels_dope_sheet = open;
}

// ----------------------------------------------------------------- Pose Library

#[derive(Default)]
pub struct PoseState {
    name: String,
    hierarchy: bool,
    weight: f32,
    key: bool,
}

pub fn pose_library(app: &mut App, ctx: &egui::Context, st: &mut PoseState) {
    let mut open = app.panels_pose_lib;
    if st.weight == 0.0 {
        st.weight = 1.0;
    }
    egui::Window::new("Pose Library").open(&mut open).default_size([380.0, 360.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.name).hint_text("pose name").desired_width(140.0));
            ui.checkbox(&mut st.hierarchy, "hierarchy").on_hover_text("Include everything below the selected nodes (select the rig root)");
            if ui.button("Save pose").clicked() {
                let n = st.name.clone();
                match app.save_pose(&n, st.hierarchy) {
                    Ok(c) => {
                        app.info(format!("Pose saved ({c} nodes)"));
                        st.name.clear();
                    }
                    Err(e) => app.warn(e),
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Blend");
            ui.add(egui::Slider::new(&mut st.weight, 0.0..=1.0));
            ui.checkbox(&mut st.key, "Key on apply");
        });
        ui.separator();
        if app.scene.poses.is_empty() {
            ui.label(RichText::new("No poses yet. Select controls (or a rig root + 'hierarchy') and Save pose.").weak());
        }
        let poses: Vec<(String, usize)> = app.scene.poses.iter().map(|p| (p.name.clone(), p.values.len() / 9)).collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (name, n) in poses {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&name).strong());
                    ui.label(RichText::new(format!("{n} nodes")).small().weak());
                    if ui.button("Apply").clicked() {
                        let r = app.apply_pose(&name, st.weight, st.key, false);
                        if let Err(e) = r {
                            app.warn(e);
                        }
                    }
                    if ui.button("Mirror").on_hover_text("Apply to the opposite side (L_ ↔ R_, TX/RY/RZ flipped)").clicked() {
                        let r = app.apply_pose(&name, st.weight, st.key, true);
                        if let Err(e) = r {
                            app.warn(e);
                        }
                    }
                    if ui.small_button("✖").on_hover_text("Delete pose").clicked() {
                        app.delete_pose(&name);
                    }
                });
            }
        });
    });
    app.panels_pose_lib = open;
}

// --------------------------------------------------------------------- Retarget

#[derive(Default)]
pub struct RetargetState {
    src: Option<NodeId>,
    dst: Option<NodeId>,
    range: Option<(f32, f32)>,
}

/// Root joints (joints without a joint parent).
fn skeleton_roots(app: &App) -> Vec<(NodeId, String)> {
    app.scene
        .nodes
        .iter()
        .filter(|n| app.is_joint(n.id) && !n.parent.is_some_and(|p| app.is_joint(p)))
        .map(|n| (n.id, n.name.clone()))
        .collect()
}

pub fn retarget_window(app: &mut App, ctx: &egui::Context, st: &mut RetargetState) {
    let mut open = app.panels_retarget;
    egui::Window::new("Retarget Animation").open(&mut open).default_size([460.0, 420.0]).show(ctx, |ui| {
        let roots = skeleton_roots(app);
        if roots.len() < 2 {
            ui.label(RichText::new("Needs two skeletons: import an animated character (File › Import glTF) and build or Quick Rig another.").weak());
        }
        let name_of = |id: Option<NodeId>| id.and_then(|i| app.scene.node(i)).map(|n| n.name.clone()).unwrap_or("—".into());
        egui::Grid::new("rt_grid").num_columns(2).show(ui, |ui| {
            ui.label("Source (animated)");
            egui::ComboBox::from_id_salt("rt_src").selected_text(name_of(st.src)).show_ui(ui, |ui| {
                for (id, n) in &roots {
                    ui.selectable_value(&mut st.src, Some(*id), n);
                }
            });
            ui.end_row();
            ui.label("Target");
            egui::ComboBox::from_id_salt("rt_dst").selected_text(name_of(st.dst)).show_ui(ui, |ui| {
                for (id, n) in &roots {
                    ui.selectable_value(&mut st.dst, Some(*id), n);
                }
            });
            ui.end_row();
            let (mut a, mut b) = st.range.unwrap_or((app.scene.play_start, app.scene.play_end));
            ui.label("Frames");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut a).speed(1.0));
                ui.label("to");
                ui.add(egui::DragValue::new(&mut b).speed(1.0));
            });
            st.range = Some((a, b));
            ui.end_row();
        });
        if let (Some(s), Some(d)) = (st.src, st.dst)
            && s != d
            && app.scene.node(s).is_some()
            && app.scene.node(d).is_some()
        {
            let map = auto_map(&app.scene, &skeleton(&app.scene, s), &skeleton(&app.scene, d));
            ui.separator();
            ui.label(RichText::new(format!("Joint mapping ({} matched; first pair carries the hip motion)", map.len())).strong());
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                egui::Grid::new("rt_map").striped(true).show(ui, |ui| {
                    for (a, b) in &map {
                        ui.label(name_of(Some(*a)));
                        ui.label("→");
                        ui.label(name_of(Some(*b)));
                        ui.end_row();
                    }
                });
            });
            ui.label(RichText::new("Both skeletons should be in a similar rest pose (their bind pose is used). IK handles and constraints on the target joints are removed so the baked keys play back.").small().weak());
            if ui.button("Retarget (bake keys)").clicked() {
                let (a, b) = st.range.unwrap();
                let r = app.retarget(s, d, a, b);
                crate::ui::report(app, r);
            }
        }
    });
    app.panels_retarget = open;
}
