//! Rigging UI: Rigging menu set menus, selection-based actions, tool settings, Attribute
//! Editor rig sections and the Set Driven Key window.

use crate::app::{App, PaintOp, Tool};
use crate::rig::ConstraintKind;
use crate::scene::{Channel, NodeId, Shape};
use eframe::egui::{self, Color32, RichText};

fn report(app: &mut App, r: Result<String, String>) {
    match r {
        Ok(s) if !s.is_empty() => app.info(s),
        Ok(_) => {}
        Err(e) => app.warn(e),
    }
}

// ------------------------------------------------------------- selection-based actions

pub fn start_joint_tool(app: &mut App) {
    if app.tool != Tool::Joint {
        app.tool = Tool::Joint;
        // Start a fresh chain unless a joint is selected (then the chain continues from it).
        if !app.lead().is_some_and(|id| app.is_joint(id)) {
            app.sel.clear();
        }
        app.info("Create Joints: click to place joints, Enter to finish");
    }
}

pub fn paint_weights_tool(app: &mut App) {
    if !app.lead().and_then(|id| app.scene.node(id)).is_some_and(|n| n.skin.is_some()) {
        app.warn("Paint Skin Weights: select a skinned mesh first.");
        return;
    }
    app.tool = Tool::PaintWeights;
    app.right_tab = crate::app::RightTab::ToolSettings;
    app.panels.right = true;
}

pub fn bind_selected(app: &mut App) {
    let sel = app.sel.clone();
    let (joints, meshes): (Vec<NodeId>, Vec<NodeId>) = sel.iter().partition(|id| app.is_joint(**id));
    let r = app.bind_skin(&joints, &meshes, false, 4);
    report(app, r);
}

pub fn unbind_selected(app: &mut App) {
    let meshes: Vec<NodeId> = app.sel.iter().copied().filter(|id| app.scene.node(*id).is_some_and(|n| n.skin.is_some())).collect();
    if meshes.is_empty() {
        app.warn("Unbind Skin: select a skinned mesh.");
        return;
    }
    app.unbind_skin(&meshes);
}

pub fn ik_from_selection(app: &mut App) {
    let joints: Vec<NodeId> = app.sel.iter().copied().filter(|id| app.is_joint(*id)).collect();
    if joints.len() < 2 {
        app.warn("IK Handle: select the start joint, then the end joint.");
        return;
    }
    let r = app.create_ik(joints[0], *joints.last().unwrap(), None).map(|id| app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default());
    report(app, r);
}

pub fn constrain_selected(app: &mut App, kind: ConstraintKind, maintain_offset: bool) {
    let sel = app.sel.clone();
    let r = app.constrain(kind, &sel, maintain_offset, None, None);
    report(app, r);
}

pub fn pole_from_selection(app: &mut App) {
    if app.sel.len() != 2 {
        app.warn("Pole Vector: select the pole object, then the IK handle.");
        return;
    }
    let (p, h) = (app.sel[0], app.sel[1]);
    let r = app.pole_vector(p, h).map(|_| String::new());
    report(app, r);
}

pub fn control_at_selection(app: &mut App, shape: &str) {
    let at = app.lead().map(|id| crate::rig::world_pos(&app.scene, id));
    let normal = if shape == "circle" && app.lead().is_some_and(|id| app.is_joint(id)) {
        // Circle facing down the joint's X axis, Maya-style FK control.
        app.lead().map(|id| crate::rig::world_rotation(&app.scene, id) * glam::Vec3::X)
    } else {
        None
    };
    match app.create_control(shape, 1.0, normal, None) {
        Ok(id) => {
            if let Some(p) = at {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
        }
        Err(e) => app.warn(e),
    }
}

pub fn mirror_joints_selected(app: &mut App) {
    let roots: Vec<NodeId> = app.sel.iter().copied().filter(|id| app.is_joint(*id)).collect();
    if roots.is_empty() {
        app.warn("Mirror Joint: select the root joint of the chain to mirror.");
        return;
    }
    let (axis, s, r) = (app.mirror_axis, app.mirror_search.clone(), app.mirror_replace.clone());
    app.begin_batch();
    let mut out = Vec::new();
    for j in roots {
        match app.mirror_joints(j, axis, &s, &r) {
            Ok(id) => out.push(id),
            Err(e) => app.warn(e),
        }
    }
    app.end_batch();
    if !out.is_empty() {
        app.sel = out;
    }
}

pub fn mirror_weights_selected(app: &mut App, positive_to_negative: bool) {
    let meshes: Vec<NodeId> = app.sel.iter().copied().filter(|id| app.scene.node(*id).is_some_and(|n| n.skin.is_some())).collect();
    if meshes.is_empty() {
        app.warn("Mirror Skin Weights: select a skinned mesh.");
        return;
    }
    let axis = app.mirror_axis;
    let mut n = 0;
    for m in meshes {
        match app.mirror_skin_weights(m, axis, positive_to_negative) {
            Ok(c) => n += c,
            Err(e) => app.warn(e),
        }
    }
    app.info(format!("Mirrored weights on {n} vertices"));
}

pub fn quick_rig_selected(app: &mut App) {
    let mesh = app.sel.iter().copied().find(|id| app.scene.node(*id).is_some_and(|n| n.mesh().is_some()));
    let r = app.quick_rig(mesh);
    report(app, r);
}

pub fn blend_shape_selected(app: &mut App) {
    if app.sel.len() < 2 {
        app.warn("Blend Shape: select the target mesh(es), then the base mesh last.");
        return;
    }
    let (base, targets) = app.sel.split_last().map(|(b, t)| (*b, t.to_vec())).unwrap();
    let r = app.create_blend_shape(&targets, base);
    if r.is_ok() {
        app.sel = vec![base];
        app.right_tab = crate::app::RightTab::AttributeEditor;
    }
    report(app, r);
}

pub fn cluster_selected(app: &mut App) {
    let fo = app.cluster_falloff;
    let r = app.create_cluster(None, fo).map(|id| app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default());
    report(app, r);
}

/// The Deform menu (shared by every menu set).
pub fn deform_menu(app: &mut App, ui: &mut egui::Ui) {
    ui.menu_button("Deform", |ui| {
        if item(ui, "Blend Shape", "targets, base") {
            blend_shape_selected(app);
        }
        if item(ui, "Cluster", "verts / meshes") {
            cluster_selected(app);
        }
        ui.horizontal(|ui| {
            ui.label("Cluster falloff");
            ui.add(egui::DragValue::new(&mut app.cluster_falloff).speed(0.02).range(0.0..=100.0));
        });
        ui.separator();
        if item(ui, "Remove Blend Shape", "") {
            let sel = app.sel.clone();
            app.remove_blend_shape(&sel);
        }
        if item(ui, "Remove Clusters", "") {
            app.checkpoint();
            for id in app.sel.clone() {
                if let Some(n) = app.scene.node_mut(id) {
                    n.clusters.clear();
                }
            }
        }
        ui.separator();
        if item(ui, "Lattice", "meshes") {
            let r = app.create_lattice(app.lattice_div, None).map(|id| app.scene.node(id).map(|n| format!("Created {}", n.name)).unwrap_or_default());
            report(app, r);
        }
        ui.horizontal(|ui| {
            ui.label("Divisions");
            for d in app.lattice_div.iter_mut() {
                ui.add(egui::DragValue::new(d).range(2..=20));
            }
        });
        if item(ui, "Wrap", "targets, then driver") {
            let mut sel = app.sel.clone();
            let r = match sel.pop() {
                Some(d) => app.create_wrap(d, &sel).map(|n| format!("{n} mesh(es) wrapped")),
                None => Err("Wrap: select the meshes to deform, then the influence mesh.".into()),
            };
            report(app, r);
        }
        if item(ui, "Remove Lattice / Wrap / Wire", "") {
            let sel = app.sel.clone();
            app.remove_lattice_and_wrap(&sel);
        }
        ui.separator();
        if item(ui, "Wire", "meshes, then curve") {
            let mut sel = app.sel.clone();
            let curve = sel.iter().rposition(|i| app.scene.node(*i).is_some_and(|n| matches!(n.shape, crate::scene::Shape::Curve { .. }))).map(|i| sel.remove(i));
            let r = match curve {
                Some(c) => app.create_wire(c, &sel, app.curve_opts.wire_dropoff, 1.0, 1.0).map(|n| format!("Wire: {n} vertices bound")),
                None => Err("Wire: select the meshes to deform, then the curve.".into()),
            };
            report(app, r);
        }
        ui.horizontal(|ui| {
            ui.label("Dropoff (0 = auto)");
            ui.add(egui::DragValue::new(&mut app.curve_opts.wire_dropoff).speed(0.02).range(0.0..=1000.0));
        });
    });
}

// ------------------------------------------------------------------------------ menus

fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    ui.add(egui::Button::new(label).shortcut_text(shortcut)).clicked()
}

pub fn rigging_menus(app: &mut App, ui: &mut egui::Ui) {
    ui.menu_button("Skeleton", |ui| {
        if item(ui, "Create Joints", "") {
            start_joint_tool(app);
        }
        if item(ui, "Orient Joint", "") {
            let sel = app.sel.clone();
            app.orient_joints(&sel);
        }
        if item(ui, "Create IK Handle", "start, end") {
            ik_from_selection(app);
        }
        if item(ui, "Mirror Joints", &format!("{}→{}", app.mirror_search, app.mirror_replace)) {
            mirror_joints_selected(app);
        }
        ui.horizontal(|ui| {
            ui.label("Mirror across");
            for (i, l) in ["YZ", "XZ", "XY"].iter().enumerate() {
                ui.radio_value(&mut app.mirror_axis, i, *l);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Replace");
            ui.add(egui::TextEdit::singleline(&mut app.mirror_search).desired_width(40.0));
            ui.label("with");
            ui.add(egui::TextEdit::singleline(&mut app.mirror_replace).desired_width(40.0));
        });
        ui.separator();
        if item(ui, "Quick Rig (one-click biped)", "") {
            quick_rig_selected(app);
        }
        ui.label(RichText::new("Select a character mesh: stands on Y, faces +Z").small().weak());
    });
    ui.menu_button("Skin", |ui| {
        if item(ui, "Bind Skin", "joints + mesh") {
            bind_selected(app);
        }
        if item(ui, "Unbind Skin", "") {
            unbind_selected(app);
        }
        if item(ui, "Go to Bind Pose", "") {
            let sel = app.sel.clone();
            app.goto_bind_pose(&sel);
        }
        ui.separator();
        if item(ui, "Paint Skin Weights", "") {
            paint_weights_tool(app);
        }
        if item(ui, "Mirror Skin Weights", "+X → −X") {
            mirror_weights_selected(app, true);
        }
        if item(ui, "Mirror Skin Weights (inverse)", "−X → +X") {
            mirror_weights_selected(app, false);
        }
    });
    deform_menu(app, ui);
    ui.menu_button("Constrain", |ui| {
        let mo = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::unique("constrain_mo"))).unwrap_or(true);
        let mut mo2 = mo;
        ui.checkbox(&mut mo2, "Maintain offset");
        if mo2 != mo {
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("constrain_mo"), mo2));
        }
        ui.label(RichText::new("Select drivers, then the object to constrain").small().weak());
        for (label, kind) in [("Parent", ConstraintKind::Parent), ("Point", ConstraintKind::Point), ("Orient", ConstraintKind::Orient), ("Scale", ConstraintKind::Scale), ("Aim", ConstraintKind::Aim)] {
            if ui.button(label).clicked() {
                constrain_selected(app, kind, mo2 && kind != ConstraintKind::Aim);
            }
        }
        if ui.button("Pole Vector").on_hover_text("Select the pole object, then the IK handle").clicked() {
            pole_from_selection(app);
        }
        ui.separator();
        if ui.button("Remove Constraints").clicked() {
            let sel = app.sel.clone();
            app.remove_constraints(&sel);
        }
    });
    ui.menu_button("Control", |ui| {
        ui.label(RichText::new("Created at the selected object").small().weak());
        for (label, shape) in [("Circle", "circle"), ("Square", "square"), ("Cross", "cross"), ("Arrow", "arrow"), ("Cube", "cube"), ("Sphere", "sphere")] {
            if ui.button(format!("{label} Control")).clicked() {
                control_at_selection(app, shape);
            }
        }
        ui.separator();
        if item(ui, "Set Driven Key…", "") {
            app.panels_sdk = true;
        }
    });
}

// ----------------------------------------------------------------------- tool settings

pub fn tool_settings(app: &mut App, ui: &mut egui::Ui) {
    match app.tool {
        Tool::Joint => {
            egui::CollapsingHeader::new("Joint Settings").default_open(true).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Joint radius");
                    ui.add(egui::DragValue::new(&mut app.joint_radius).speed(0.02).range(0.05..=10.0));
                });
                ui.label(RichText::new("Orientation: xyz, secondary axis +Y (applied when you press Enter).").weak());
                ui.label(RichText::new("Ortho views place joints on the view plane; persp places the first joint on the ground and the rest facing the camera.").weak());
                if ui.button("Finish chain (Enter)").clicked() {
                    app.finish_joint_chain();
                }
            });
        }
        Tool::PaintWeights => {
            let Some(id) = app.lead() else { return };
            let joints: Vec<(usize, String)> = app
                .scene
                .node(id)
                .and_then(|n| n.skin.as_ref())
                .map(|sk| sk.joints.iter().enumerate().map(|(i, j)| (i, app.scene.node(*j).map(|n| n.name.clone()).unwrap_or("<deleted>".into()))).collect())
                .unwrap_or_default();
            egui::CollapsingHeader::new("Influences").default_open(true).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(220.0).id_salt("influences").show(ui, |ui| {
                    for (i, name) in &joints {
                        if ui.selectable_label(app.paint.influence == *i, name).clicked() {
                            app.paint.influence = *i;
                        }
                    }
                });
            });
            egui::CollapsingHeader::new("Paint Weights").default_open(true).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut app.paint.op, PaintOp::Replace, "Replace");
                    ui.radio_value(&mut app.paint.op, PaintOp::Add, "Add");
                    ui.radio_value(&mut app.paint.op, PaintOp::Smooth, "Smooth");
                });
                ui.horizontal(|ui| {
                    ui.label("Value");
                    ui.add(egui::Slider::new(&mut app.paint.value, 0.0..=1.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Radius (px)");
                    ui.add(egui::Slider::new(&mut app.paint.radius, 5.0..=200.0));
                });
                if ui.button("Flood").on_hover_text("Apply the current value to every vertex").clicked() {
                    app.checkpoint();
                    let inf = app.paint.influence;
                    let v = app.paint.value;
                    if let Some(sk) = app.scene.node_mut(id).and_then(|n| n.skin.as_mut()) {
                        for vi in 0..sk.weights.len() {
                            sk.set_weight(vi, inf, v);
                        }
                    }
                }
            });
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ attribute editor

pub fn attribute_sections(app: &mut App, ui: &mut egui::Ui, id: NodeId) {
    let Some(node) = app.scene.node(id).cloned() else { return };
    if matches!(node.shape, Shape::Joint { .. }) {
        egui::CollapsingHeader::new("Joint").default_open(true).show(ui, |ui| {
            let mut o = node.joint_orient;
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.add_sized([70.0, 18.0], egui::Label::new("Joint Orient"));
                for i in 0..3 {
                    changed |= ui.add(egui::DragValue::new(&mut o[i]).speed(0.5).max_decimals(2)).changed();
                }
            });
            if let Shape::Joint { radius } = node.shape {
                let mut r = radius;
                ui.horizontal(|ui| {
                    ui.add_sized([70.0, 18.0], egui::Label::new("Radius"));
                    if ui.add(egui::DragValue::new(&mut r).speed(0.02).range(0.05..=20.0)).changed() {
                        app.edit_checkpoint(egui::Id::unique(("jrad", id)));
                        if let Some(Shape::Joint { radius }) = app.scene.node_mut(id).map(|n| &mut n.shape) {
                            *radius = r;
                        }
                    }
                });
            }
            if changed {
                app.edit_checkpoint(egui::Id::unique(("jo", id)));
                if let Some(n) = app.scene.node_mut(id) {
                    n.joint_orient = o;
                }
            }
            if ui.button("Orient Joint (xyz)").clicked() {
                app.orient_joints(&[id]);
            }
        });
    }
    if let Shape::IkHandle { start, end, pole, .. } = &node.shape {
        egui::CollapsingHeader::new("IK Handle (ikRPsolver)").default_open(true).show(ui, |ui| {
            let name = |n: NodeId| app.scene.node(n).map(|x| x.name.clone()).unwrap_or("<deleted>".into());
            ui.label(format!("Start joint: {}", name(*start)));
            ui.label(format!("End effector: {}", name(*end)));
            ui.label(format!("Pole vector: {}", pole.map(name).unwrap_or("none".into())));
            if pole.is_some() && ui.button("Remove pole vector").clicked() {
                app.checkpoint();
                if let Some(Shape::IkHandle { pole, .. }) = app.scene.node_mut(id).map(|n| &mut n.shape) {
                    *pole = None;
                }
            }
            if let Shape::IkHandle { stretch, twist, .. } = node.shape.clone() {
                let mut st = stretch;
                if ui.checkbox(&mut st, "Stretchy (bones lengthen to reach)").changed() {
                    let _ = app.set_ik_options(id, Some(st), None);
                }
                let mut tw = twist;
                ui.horizontal(|ui| {
                    ui.add_sized([70.0, 18.0], egui::Label::new("Twist"));
                    if ui.add(egui::DragValue::new(&mut tw).speed(0.5).suffix("°")).changed() {
                        app.edit_checkpoint(egui::Id::unique(("iktw", id)));
                        if let Some(Shape::IkHandle { twist, .. }) = app.scene.node_mut(id).map(|n| &mut n.shape) {
                            *twist = tw;
                        }
                    }
                });
            }
        });
    }
    if let Some(b) = &node.blend {
        egui::CollapsingHeader::new(format!("blendShape ({} targets)", b.targets.len())).default_open(true).show(ui, |ui| {
            let mut remove = None;
            for (i, t) in b.targets.iter().enumerate() {
                let mut w = b.weights.get(i).copied().unwrap_or(0.0);
                let keyed = b.curves.get(i).is_some_and(|c| c.is_some());
                let driven = node.driven.iter().any(|d| d.blend_target == Some(i));
                ui.horizontal(|ui| {
                    ui.add_sized([80.0, 18.0], egui::Label::new(&t.name).truncate());
                    let r = ui.add_enabled(!driven, egui::Slider::new(&mut w, 0.0..=1.0).max_decimals(3));
                    if r.changed() {
                        app.edit_checkpoint(egui::Id::unique(("bsw", id, i)));
                        app.set_blend_weight(id, i, w, true);
                    }
                    if r.drag_stopped() && app.auto_key {
                        app.key_blend_weight(id, i, false);
                    }
                    let kc = if keyed { Color32::from_rgb(220, 70, 60) } else { ui.visuals().text_color() };
                    if ui.small_button(RichText::new("◆").color(kc)).on_hover_text("Key this weight at the current frame").clicked() {
                        app.key_blend_weight(id, i, true);
                    }
                    if ui.small_button("✖").on_hover_text("Delete target").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                app.checkpoint();
                if let Some(n) = app.scene.node_mut(id) {
                    if let Some(b) = n.blend.as_mut() {
                        b.targets.remove(i);
                        b.weights.remove(i);
                        if i < b.curves.len() {
                            b.curves.remove(i);
                        }
                    }
                    n.driven.retain(|d| d.blend_target != Some(i));
                    for d in &mut n.driven {
                        if let Some(t) = d.blend_target.as_mut() {
                            if *t > i {
                                *t -= 1;
                            }
                        }
                    }
                }
            }
            ui.label(RichText::new("Add targets: select target meshes, then this base → Deform › Blend Shape").small().weak());
        });
    }
    if !node.clusters.is_empty() {
        egui::CollapsingHeader::new("Clusters").default_open(true).show(ui, |ui| {
            for c in &node.clusters {
                let hn = app.scene.node(c.handle).map(|n| n.name.clone()).unwrap_or("<deleted>".into());
                ui.horizontal(|ui| {
                    ui.label(format!("{hn}: {} vertices", c.weights.len()));
                    if ui.small_button("Select").clicked() && app.scene.node(c.handle).is_some() {
                        app.sel = vec![c.handle];
                    }
                });
            }
        });
    }
    if matches!(node.shape, Shape::ClusterHandle) {
        let users: Vec<String> = app.scene.nodes.iter().filter(|n| n.clusters.iter().any(|c| c.handle == id)).map(|n| n.name.clone()).collect();
        egui::CollapsingHeader::new("Cluster").default_open(true).show(ui, |ui| {
            ui.label(format!("Deforms: {}", if users.is_empty() { "nothing".into() } else { users.join(", ") }));
        });
    }
    if let Some(skin) = &node.skin {
        egui::CollapsingHeader::new("skinCluster").default_open(true).show(ui, |ui| {
            ui.label(format!("{} influences · max {} per vertex", skin.joints.len(), skin.max_influences));
            let names: Vec<String> = skin.joints.iter().map(|j| app.scene.node(*j).map(|n| n.name.clone()).unwrap_or("<deleted>".into())).collect();
            ui.label(RichText::new(names.join(", ")).small().weak());
            ui.horizontal(|ui| {
                if ui.button("Paint Weights").clicked() {
                    app.sel = vec![id];
                    paint_weights_tool(app);
                }
                if ui.button("Go to Bind Pose").clicked() {
                    app.goto_bind_pose(&[id]);
                }
                if ui.button("Unbind").clicked() {
                    app.unbind_skin(&[id]);
                }
            });
        });
    }
    if !node.constraints.is_empty() {
        egui::CollapsingHeader::new("Constraints").default_open(true).show(ui, |ui| {
            let mut remove = None;
            for (i, c) in node.constraints.iter().enumerate() {
                ui.horizontal(|ui| {
                    let d: Vec<String> = c.drivers.iter().filter_map(|x| app.scene.node(*x)).map(|n| n.name.clone()).collect();
                    ui.label(format!("{} ← {}", c.kind.name(), d.join(", ")));
                    if ui.small_button("✖").on_hover_text("Delete constraint").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                app.checkpoint();
                if let Some(n) = app.scene.node_mut(id) {
                    n.constraints.remove(i);
                }
            }
        });
    }
    if !node.driven.is_empty() {
        egui::CollapsingHeader::new("Driven Keys").default_open(true).show(ui, |ui| {
            let mut remove = None;
            for (i, d) in node.driven.iter().enumerate() {
                ui.horizontal(|ui| {
                    let dn = app.scene.node(d.driver).map(|n| n.name.clone()).unwrap_or("?".into());
                    let what = match d.blend_target {
                        Some(t) => node.blend.as_ref().and_then(|b| b.targets.get(t)).map(|x| format!("blend {}", x.name)).unwrap_or("blend ?".into()),
                        None => d.channel.long().to_string(),
                    };
                    ui.label(format!("{what} ← {dn}.{} ({} keys)", d.driver_channel.long(), d.curve.keys.len()));
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                app.checkpoint();
                if let Some(n) = app.scene.node_mut(id) {
                    n.driven.remove(i);
                }
            }
        });
    }
}

// ------------------------------------------------------------------ set driven key

pub fn sdk_window(app: &mut App, ctx: &egui::Context) {
    if !app.panels_sdk {
        return;
    }
    let mut open = true;
    egui::Window::new("Set Driven Key").open(&mut open).default_width(420.0).show(ctx, |ui| {
        let attr_list = |ui: &mut egui::Ui, salt: &str, slot: &mut Option<(NodeId, Channel)>, app_name: Option<String>| {
            ui.vertical(|ui| {
                ui.label(RichText::new(app_name.unwrap_or("—".into())).strong());
                egui::ScrollArea::vertical().id_salt(salt).max_height(190.0).show(ui, |ui| {
                    if let Some((id, cur)) = *slot {
                        for ch in Channel::ALL {
                            if ui.selectable_label(cur == ch, ch.long()).clicked() {
                                *slot = Some((id, ch));
                            }
                        }
                    }
                });
            });
        };
        let driver_name = app.sdk_driver.and_then(|(id, _)| app.scene.node(id)).map(|n| n.name.clone());
        let driven_name = app.sdk_driven.and_then(|(id, _)| app.scene.node(id)).map(|n| n.name.clone());
        ui.columns(2, |cols| {
            cols[0].label(RichText::new("Driver").color(Color32::from_rgb(150, 200, 255)));
            if cols[0].button("Load Driver (selected)").clicked() {
                if let Some(id) = app.lead() {
                    app.sdk_driver = Some((id, Channel::TranslateY));
                }
            }
            attr_list(&mut cols[0], "sdk_driver", &mut app.sdk_driver, driver_name);
            cols[1].label(RichText::new("Driven").color(Color32::from_rgb(255, 200, 150)));
            if cols[1].button("Load Driven (selected)").clicked() {
                if let Some(id) = app.lead() {
                    app.sdk_driven = Some((id, Channel::RotateY));
                }
            }
            attr_list(&mut cols[1], "sdk_driven", &mut app.sdk_driven, driven_name);
        });
        ui.separator();
        let ready = app.sdk_driver.is_some() && app.sdk_driven.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(ready, egui::Button::new("Key")).on_hover_text("Key the driven attribute's current value at the driver's current value").clicked() {
                let (d, dc) = app.sdk_driver.unwrap();
                let (t, tc) = app.sdk_driven.unwrap();
                let r = app.set_driven_key(d, dc, &[t], &[tc], None, None);
                report(app, r);
            }
            ui.label(RichText::new("Pose the driver and driven, press Key; repeat for each pose.").small().weak());
        });
    });
    app.panels_sdk = open;
}
