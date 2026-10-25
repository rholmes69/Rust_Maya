//! Main menu bar (menu sets), Status Line and Shelf.

use super::Action;
use crate::app::{App, CompMode, MenuSet, Workspace};
use crate::mesh::Primitive;
use crate::scene::Channel;
use glam::Vec3;
use eframe::egui::{self, Color32, RichText, Sense};

fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    ui.add(egui::Button::new(label).shortcut_text(shortcut)).clicked()
}

fn emit(app: &mut App, kind: crate::fx::EmitterKind, from_selection: bool) {
    let mut st = crate::fx::ParticleSettings { kind, ..Default::default() };
    if from_selection {
        st.source = app.lead();
    }
    let r = app.create_emitter(st, None, None).map(|id| format!("Created {} (play the timeline to simulate)", app.scene.node(id).map(|n| n.name.as_str()).unwrap_or("")));
    super::report(app, r);
}

/// Select colliders, then the particle system / cloth last.
fn make_collide(app: &mut App, remove: bool) {
    let sel = app.sel.clone();
    let r = match sel.split_last() {
        Some((target, cols)) if !cols.is_empty() => cols.iter().try_for_each(|c| app.connect_collider(*c, *target, remove)).map(|_| if remove { "Colliders removed".to_string() } else { format!("{} collider(s) connected", cols.len()) }),
        _ => Err("Select the collider meshes, then the particle system or nCloth mesh last.".into()),
    };
    super::report(app, r);
}

fn add_field(app: &mut App, kind: crate::fx::FieldKind) {
    let targets = app.selected_dynamics();
    let n = targets.len();
    let r = app.create_field(crate::fx::Field::new(kind), None, None, targets).map(|id| {
        let name = app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default();
        if n == 0 { format!("Created {name} (acts on every simulation)") } else { format!("Created {name} (acts on the {n} selected simulation(s))") }
    });
    super::report(app, r);
}

fn fields_menu(app: &mut App, ui: &mut egui::Ui) {
    use crate::fx::FieldKind as F;
    for (k, label, tip) in [
        (F::Air, "Air", "wind: carries things along"),
        (F::Drag, "Drag", "slows things down"),
        (F::Gravity, "Gravity", "extra weight"),
        (F::Newton, "Newton", "attracts to a point"),
        (F::Radial, "Radial", "pushes away from a point"),
        (F::Turbulence, "Turbulence", "noise"),
        (F::Uniform, "Uniform", "constant push"),
        (F::Vortex, "Vortex", "swirls around an axis"),
    ] {
        if item(ui, label, tip) {
            add_field(app, k);
        }
    }
    ui.separator();
    if item(ui, "Affect Selected Object(s)", "field, then targets") {
        let sel = app.sel.clone();
        let field = sel.iter().copied().find(|i| matches!(app.scene.node(*i).map(|n| &n.shape), Some(crate::scene::Shape::Field(_))));
        let r = match field {
            Some(f) => sel.iter().filter(|i| **i != f).try_for_each(|t| app.connect_field(f, *t, false)).map(|_| "Field connected".to_string()),
            None => Err("Select a field and the simulations it should affect.".into()),
        };
        super::report(app, r);
    }
    ui.label(egui::RichText::new("A field with no targets acts on every particle system, cloth and hair. Move / rotate it to place and aim it.").weak().small());
}

fn hair_menu(app: &mut App, ui: &mut egui::Ui) {
    let sel = app.sel.clone();
    if item(ui, "Create Hair", "on the selected mesh") {
        let mesh = sel.iter().copied().find(|i| app.scene.node(*i).is_some_and(|n| n.mesh().is_some()));
        let r = match mesh {
            Some(m) => app.create_hair(m, crate::hair::HairSettings { length: 0.0, width: 0.0, thickness: 0.0, ..Default::default() }, None).map(|_| "Hair created (play the timeline; style it in the Attribute Editor)".to_string()),
            None => Err("Select a polygon mesh to grow hair from.".into()),
        };
        super::report(app, r);
    }
    if item(ui, "Make Selected Curves Dynamic", "") {
        let curves: Vec<_> = sel.iter().copied().filter(|i| matches!(app.scene.node(*i).map(|n| &n.shape), Some(crate::scene::Shape::Curve { .. }))).collect();
        let r = if curves.is_empty() {
            Err("Select curves first.".to_string())
        } else {
            curves.iter().try_for_each(|c| app.make_curve_dynamic(*c, false, None).map(|_| ())).map(|_| format!("{} dynamic curve(s): the *_dyn curves follow the simulation", curves.len()))
        };
        super::report(app, r);
    }
    if item(ui, "Convert Hair to Curves", "") {
        let r = match sel.first() {
            Some(id) => app.hair_to_curves(*id).map(|_| "Curves created".to_string()),
            None => Err("Select a hair system.".into()),
        };
        super::report(app, r);
    }
    ui.separator();
    if item(ui, "Make Collide", "colliders, then hair") {
        make_collide(app, false);
    }
}

fn particles_menu(app: &mut App, ui: &mut egui::Ui) {
    use crate::fx::EmitterKind as K;
    if item(ui, "Create Emitter", "omni") {
        emit(app, K::Omni, false);
    }
    if item(ui, "Directional Emitter", "") {
        emit(app, K::Directional, false);
    }
    if item(ui, "Volume Emitter", "scale to size") {
        emit(app, K::Volume, false);
    }
    if item(ui, "Emit from Object", "mesh or curve") {
        emit(app, K::Omni, true);
    }
    ui.separator();
    if item(ui, "Make Collide", "colliders, then target") {
        make_collide(app, false);
    }
    if item(ui, "Remove Collision", "") {
        make_collide(app, true);
    }
    ui.label(egui::RichText::new("Particles simulate as the time slider plays; settings are in the Attribute Editor.").weak().small());
}

fn cloth_menu(app: &mut App, ui: &mut egui::Ui) {
    let sel = app.sel.clone();
    if item(ui, "Create nCloth", "meshes") {
        let r = app.create_cloth(&sel, |_| {}).map(|n| format!("{n} mesh(es) are now nCloth (play the timeline)"));
        super::report(app, r);
    }
    if item(ui, "Remove nCloth", "") {
        let n = app.remove_cloth(&sel);
        app.info(format!("Removed nCloth from {n} mesh(es)"));
    }
    ui.separator();
    if item(ui, "Pin Selected Vertices", "vertex mode") {
        let mut msg = Err("Select cloth vertices first (F9).".to_string());
        for (id, vs) in app.comp_vertices() {
            msg = app.pin_cloth(id, &vs, None, false).map(|n| format!("{n} pinned vertices"));
        }
        super::report(app, msg);
    }
    if item(ui, "Clear Pins", "") {
        let r = sel.iter().try_for_each(|id| app.pin_cloth(*id, &[], None, true).map(|_| ())).map(|_| "Pins cleared".to_string());
        super::report(app, r);
    }
    if item(ui, "Make Collide", "colliders, then cloth") {
        make_collide(app, false);
    }
    ui.label(egui::RichText::new("Pin to a joint or control in the Attribute Editor › nCloth.").weak().small());
}

fn curves_menu(app: &mut App, ui: &mut egui::Ui) {
    let sel = app.sel.clone();
    if item(ui, "Rebuild", "") {
        let (sp, d) = (app.curve_opts.rebuild_spans as usize, app.curve_opts.rebuild_degree as usize);
        let r = app.rebuild_curves(&sel, sp, d).map(|n| format!("Rebuilt {n} curve(s)"));
        super::report(app, r);
    }
    ui.horizontal(|ui| {
        ui.label("Spans");
        ui.add(egui::DragValue::new(&mut app.curve_opts.rebuild_spans).range(1..=256));
        ui.label("Degree");
        ui.radio_value(&mut app.curve_opts.rebuild_degree, 1, "1");
        ui.radio_value(&mut app.curve_opts.rebuild_degree, 3, "3");
    });
    if item(ui, "Reverse Direction", "") {
        let r = app.reverse_curves(&sel).map(|n| format!("Reversed {n} curve(s)"));
        super::report(app, r);
    }
    if item(ui, "Open/Close", "") {
        let r = app.open_close_curves(&sel).map(|n| format!("Toggled {n} curve(s)"));
        super::report(app, r);
    }
    ui.separator();
    if item(ui, "Attach", "two curves") {
        let r = if sel.len() >= 2 { app.attach_curves(sel[0], sel[1]).map(|_| "Attached".to_string()) } else { Err("Attach: select two curves.".into()) };
        super::report(app, r);
    }
    if item(ui, "Detach", "at the middle") {
        let r = match sel.first().and_then(|i| crate::nurbs::world_curve(&app.scene, *i).map(|c| (*i, c))) {
            Some((id, c)) => {
                let (a, b) = c.domain();
                app.detach_curve(id, 0.5 * (a + b)).map(|_| "Detached".to_string())
            }
            None => Err("Detach: select a curve.".into()),
        };
        super::report(app, r);
    }
    ui.separator();
    if item(ui, "Edit CVs", "F9") {
        app.set_comp_mode(CompMode::Vertex);
    }
}

fn surfaces_menu(app: &mut App, ui: &mut egui::Ui) {
    let sel = app.sel.clone();
    let o = app.curve_opts.clone();
    if item(ui, "Revolve", "profile curves") {
        let axis = [Vec3::X, Vec3::Y, Vec3::Z][o.revolve_axis.min(2)];
        let r = app.revolve_curves(&sel, axis, Vec3::ZERO, o.revolve_sections, 0.0, 360.0, o.samples, true, None).map(|v| format!("Revolved {} surface(s)", v.len()));
        super::report(app, r);
    }
    ui.horizontal(|ui| {
        ui.label("Axis");
        for (i, n) in ["X", "Y", "Z"].iter().enumerate() {
            ui.radio_value(&mut app.curve_opts.revolve_axis, i, *n);
        }
        ui.label("Sections");
        ui.add(egui::DragValue::new(&mut app.curve_opts.revolve_sections).range(3..=256));
    });
    if item(ui, "Loft", "curves in order") {
        let r = app.loft_curves(&sel, o.loft_sections, o.loft_close, o.samples, true, None).map(|_| "Lofted".to_string());
        super::report(app, r);
    }
    ui.horizontal(|ui| {
        ui.label("Section spans");
        ui.add(egui::DragValue::new(&mut app.curve_opts.loft_sections).range(1..=64));
        ui.checkbox(&mut app.curve_opts.loft_close, "Close");
    });
    if item(ui, "Extrude", "profile [, path]") {
        let r = match sel.first() {
            Some(p) => app.extrude_curve(*p, sel.get(1).copied(), o.extrude_length, Vec3::ZERO, o.samples, true, None).map(|_| "Extruded".to_string()),
            None => Err("Extrude: select a profile curve (then a path curve to sweep along).".into()),
        };
        super::report(app, r);
    }
    ui.horizontal(|ui| {
        ui.label("Length (no path)");
        ui.add(egui::DragValue::new(&mut app.curve_opts.extrude_length).speed(0.05));
    });
    if item(ui, "Planar", "closed curve") {
        let r = match sel.first() {
            Some(c) => app.planar_curve(*c, o.samples, true, None).map(|_| "Planar surface".to_string()),
            None => Err("Planar: select a closed curve.".into()),
        };
        super::report(app, r);
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Samples per curve");
        ui.add(egui::DragValue::new(&mut app.curve_opts.samples).range(3..=512));
    });
    ui.label(egui::RichText::new("Surfaces are polygon meshes with history: editing the curves updates them.").weak().small());
}

fn planned(ui: &mut egui::Ui, label: &str) {
    ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text("Planned — not available in this build yet");
}

pub fn menu_bar(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if item(ui, "New Scene", "Ctrl+N") {
                actions.push(Action::New);
            }
            if item(ui, "Open Scene...", "Ctrl+O") {
                actions.push(Action::Open);
            }
            if item(ui, "Save Scene", "Ctrl+S") {
                actions.push(Action::Save);
            }
            if item(ui, "Save Scene As...", "Ctrl+Shift+S") {
                actions.push(Action::SaveAs);
            }
            ui.menu_button("Recent Files", |ui| {
                if app.recent_files.is_empty() {
                    ui.label(RichText::new("No recent files").weak());
                }
                for p in app.recent_files.clone() {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if ui.button(name).on_hover_text(p.display().to_string()).clicked() {
                        actions.push(Action::OpenPath(p));
                    }
                }
                if !app.recent_files.is_empty() {
                    ui.separator();
                    if ui.button("Clear List").clicked() {
                        app.recent_files.clear();
                    }
                }
            });
            ui.menu_button("Recover Autosave", |ui| {
                let list = crate::prefs::list_autosaves();
                if list.is_empty() {
                    ui.label(RichText::new("No autosaves").weak());
                }
                for (p, t) in list {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if ui.button(format!("{name}  ({})", crate::prefs::age_text(t))).clicked() {
                        actions.push(Action::Recover(p));
                    }
                }
            });
            ui.separator();
            if item(ui, "Import (FBX / glTF / USD / OBJ)...", "") {
                actions.push(Action::ImportObj);
            }
            if item(ui, "Export All (OBJ)...", "") {
                actions.push(Action::ExportObj(false));
            }
            if item(ui, "Export Selection (OBJ)...", "") {
                actions.push(Action::ExportObj(true));
            }
            if item(ui, "Export All (glTF / FBX / USD)...", "") {
                actions.push(Action::Export(false));
            }
            if item(ui, "Export Selection (glTF / FBX / USD)...", "") {
                actions.push(Action::Export(true));
            }
            ui.checkbox(&mut app.usd_bake_points, "USD: bake skinning to points (instead of UsdSkel)");
            ui.separator();
            planned(ui, "Reference Editor");
            ui.separator();
            if item(ui, "Exit", "Ctrl+Q") {
                actions.push(Action::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.add_enabled(app.can_undo(), egui::Button::new("Undo").shortcut_text("Z / Ctrl+Z")).clicked() {
                app.undo();
            }
            if ui.add_enabled(app.can_redo(), egui::Button::new("Redo").shortcut_text("Shift+Z / Ctrl+Y")).clicked() {
                app.redo();
            }
            ui.separator();
            if item(ui, "Duplicate", "Ctrl+D") {
                app.duplicate_selected();
            }
            if item(ui, "Delete", "Del") {
                app.delete_selected();
            }
            ui.separator();
            if item(ui, "Group", "Ctrl+G") {
                app.group_selected();
            }
            if item(ui, "Parent", "P") {
                app.parent_selected();
            }
            if item(ui, "Unparent", "Shift+P") {
                app.unparent_selected();
            }
            ui.separator();
            ui.menu_button("Delete by Type", |ui| {
                if ui.button("History").clicked() {
                    app.delete_history();
                }
                if ui.button("Channels (animation)").clicked() {
                    app.delete_keys_selected(false);
                }
            });
        });
        ui.menu_button("Create", |ui| {
            ui.menu_button("Polygon Primitives", |ui| create_prims(app, ui));
            ui.menu_button("NURBS Primitives", |ui| {
                for n in ["Sphere", "Cube", "Cylinder", "Cone", "Plane", "Torus", "Circle"] {
                    planned(ui, n);
                }
            });
            ui.separator();
            if ui.button("Locator").clicked() {
                app.create_locator();
            }
            if ui.button("Empty Group").clicked() {
                app.create_group_empty();
            }
            ui.separator();
            ui.menu_button("Lights", |ui| super::lookdev::lights_menu(app, ui));
            ui.menu_button("Cameras", |ui| super::lookdev::cameras_menu(app, ui));
            ui.menu_button("Curve Tools", |ui| {
                if item(ui, "CV Curve Tool", "click, Enter") {
                    app.curve_tool.points.clear();
                    app.tool = crate::app::Tool::CurveCv;
                }
                if item(ui, "EP Curve Tool", "click, Enter") {
                    app.curve_tool.points.clear();
                    app.tool = crate::app::Tool::CurveEp;
                }
                ui.horizontal(|ui| {
                    ui.label("Degree");
                    ui.radio_value(&mut app.curve_tool.degree, 1, "1 Linear");
                    ui.radio_value(&mut app.curve_tool.degree, 3, "3 Cubic");
                });
            });
            ui.menu_button("NURBS Primitives", |ui| {
                if item(ui, "Circle", "") {
                    let (r, s) = (app.curve_opts.circle_radius, app.curve_opts.circle_sections as usize);
                    app.create_circle(r, s, Vec3::Y, Vec3::ZERO, None);
                }
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(&mut app.curve_opts.circle_radius).speed(0.05).range(0.001..=1000.0));
                    ui.label("Sections");
                    ui.add(egui::DragValue::new(&mut app.curve_opts.circle_sections).range(3..=64));
                });
            });
            planned(ui, "Type");
        });
        ui.menu_button("Select", |ui| {
            if item(ui, "All", "Ctrl+Shift+A") {
                app.select_all();
            }
            if item(ui, "Deselect All", "") {
                app.clear_selection();
            }
            if item(ui, "Hierarchy", "") {
                app.select_hierarchy();
            }
            if item(ui, "Inverse", "Ctrl+Shift+I") {
                app.invert_selection();
            }
            ui.separator();
            if item(ui, "Object/Component", "F8") {
                let m = if app.comp_mode == CompMode::Object { CompMode::Vertex } else { CompMode::Object };
                app.set_comp_mode(m);
            }
            if item(ui, "Vertex", "F9") {
                app.set_comp_mode(CompMode::Vertex);
            }
            if item(ui, "Edge", "F10") {
                app.set_comp_mode(CompMode::Edge);
            }
            if item(ui, "Edge Loop", "double-click edge") {
                app.select_edge_loop(false);
            }
            if item(ui, "Edge Ring", "") {
                app.select_edge_loop(true);
            }
            if item(ui, "Face", "F11") {
                app.set_comp_mode(CompMode::Face);
            }
        });
        ui.menu_button("Modify", |ui| {
            if ui.button("Freeze Transformations").clicked() {
                app.freeze_transforms();
            }
            if ui.button("Reset Transformations").clicked() {
                app.reset_transforms();
            }
            planned(ui, "Center Pivot");
            planned(ui, "Snap Align Objects");
            ui.separator();
            if item(ui, "Make Live", "mesh") {
                let id = app.lead();
                let r = app.qd_make_live(id).map(|_| String::new());
                super::report(app, r);
            }
            if item(ui, "Make Not Live", "") {
                let _ = app.qd_make_live(None);
            }
        });
        ui.menu_button("Display", |ui| {
            if item(ui, "Hide Selection", "Ctrl+H") {
                app.toggle_visibility_selected(false);
            }
            if item(ui, "Show Selection", "Shift+H") {
                app.toggle_visibility_selected(true);
            }
            if item(ui, "Show All", "Ctrl+Shift+H") {
                app.show_all();
            }
            ui.separator();
            let v = app.active_view;
            ui.checkbox(&mut app.views[v].show_grid, "Grid");
            ui.checkbox(&mut app.views[v].show_hud, "Heads Up Display (Poly Count)");
        });
        ui.menu_button("Windows", |ui| {
            ui.checkbox(&mut app.panels.outliner, "Outliner");
            ui.checkbox(&mut app.panels.right, "Channel Box / Attribute Editor");
            ui.checkbox(&mut app.panels.graph_editor, "Animation Editors > Graph Editor");
            ui.checkbox(&mut app.panels_dope_sheet, "Animation Editors > Dope Sheet");
            ui.checkbox(&mut app.panels_pose_lib, "Animation Editors > Pose Library");
            ui.checkbox(&mut app.panels_retarget, "Animation Editors > Retarget");
            ui.checkbox(&mut app.panels.script_editor, "General Editors > Script Editor");
            ui.checkbox(&mut app.panels.prefs, "Settings/Preferences > Preferences");
            ui.separator();
            ui.menu_button("UI Elements", |ui| {
                ui.checkbox(&mut app.panels.status, "Status Line");
                ui.checkbox(&mut app.panels.shelf, "Shelf");
                ui.checkbox(&mut app.panels.toolbox, "Tool Box");
                ui.checkbox(&mut app.panels.timeline, "Time Slider");
                ui.checkbox(&mut app.panels.range, "Range Slider");
                ui.checkbox(&mut app.panels.cmdline, "Command Line");
                ui.checkbox(&mut app.panels.helpline, "Help Line");
            });
            ui.separator();
            ui.checkbox(&mut app.panels_hypershade, "Rendering Editors > Hypershade");
            ui.checkbox(&mut app.render.open, "Rendering Editors > Render View");
            ui.checkbox(&mut app.panels_uv_editor, "Modeling Editors > UV Editor");
            planned(ui, "Node Editor");
        });
        // Menu-set specific menus.
        match app.menu_set {
            MenuSet::Modeling => modeling_menus(app, ui),
            MenuSet::Animation => animation_menus(app, ui),
            MenuSet::Rigging => super::rigging::rigging_menus(app, ui),
            MenuSet::Fx => {
                ui.menu_button("nParticles", |ui| particles_menu(app, ui));
                ui.menu_button("nCloth", |ui| cloth_menu(app, ui));
                ui.menu_button("nHair", |ui| hair_menu(app, ui));
                ui.menu_button("Fields/Solvers", |ui| fields_menu(app, ui));
                for m in ["Fluids", "Effects", "Bifrost Fluids"] {
                    ui.menu_button(m, |ui| planned(ui, "Not available in this build"));
                }
            }
            MenuSet::Rendering => {
                ui.menu_button("Lighting/Shading", |ui| super::lookdev::lighting_shading_menu(app, ui));
                ui.menu_button("Render", |ui| render_menu(app, ui));
                for m in ["Texturing", "Toon", "Stereo"] {
                    ui.menu_button(m, |ui| planned(ui, "Not available in this build"));
                }
            }
        }
        ai_menu(app, ui);
        ui.menu_button("Cache", |ui| planned(ui, "Alembic Cache"));
        ui.menu_button("Arnold", |ui| {
            ui.label(RichText::new("Arnold isn't available; the built-in path tracer renders instead.").weak());
            render_menu(app, ui);
        });
        ui.menu_button("Help", |ui| {
            if ui.button("Hotkeys").clicked() {
                app.panels.hotkeys = true;
            }
            if ui.button("MEL Command Reference").clicked() {
                app.log.push(crate::commands::HELP.to_string());
                app.panels.script_editor = true;
            }
            if ui.button("About Rust Maya").clicked() {
                app.panels.about = true;
            }
        });
    });
}

fn create_prims(app: &mut App, ui: &mut egui::Ui) {
    for (n, p) in [
        ("Sphere", Primitive::default_sphere()),
        ("Cube", Primitive::default_cube()),
        ("Cylinder", Primitive::default_cylinder()),
        ("Cone", Primitive::default_cone()),
        ("Plane", Primitive::default_plane()),
        ("Torus", Primitive::default_torus()),
    ] {
        if ui.button(n).clicked() {
            app.create_primitive(p);
        }
    }
}

fn modeling_menus(app: &mut App, ui: &mut egui::Ui) {
    ui.menu_button("Mesh", |ui| {
        if ui.button("Combine").clicked() {
            app.combine();
        }
        if ui.button("Separate").clicked() {
            app.separate();
        }
        ui.separator();
        if ui.button("Smooth").clicked() {
            app.smooth();
        }
        if ui.button("Triangulate").clicked() {
            app.triangulate();
        }
        ui.separator();
        if item(ui, "Mirror", "") {
            let r = app.mirror_selected(app.mirror_axis, 0.001);
            super::report(app, r);
        }
        if item(ui, "Merge (weld by distance)", "") {
            let d = app.merge_dist;
            let r = app.merge_selected(Some(d));
            super::report(app, r);
        }
        ui.horizontal(|ui| {
            let id = egui::Id::unique("reduce_percent");
            let mut pct: f32 = ui.data_mut(|d| *d.get_temp_mut_or(id, 50.0));
            if ui.button("Reduce").on_hover_text("Remove this percentage of the triangles (quadric decimation; keeps borders, UV seams, skin weights)").clicked() {
                let sel = app.sel.clone();
                let r = app.reduce_meshes(&sel, 100.0 - pct, true);
                super::report(app, r);
            }
            ui.add(egui::DragValue::new(&mut pct).range(1.0..=99.0).suffix(" %"));
            ui.data_mut(|d| d.insert_temp(id, pct));
        });
    });
    ui.menu_button("Edit Mesh", |ui| {
        if item(ui, "Extrude", "Ctrl+E") {
            app.extrude();
        }
        if item(ui, "Delete Faces/Vertices", "Del") {
            app.delete_selected();
        }
        if item(ui, "Delete Edge", "Del (edge mode)") {
            app.delete_edges();
        }
        ui.separator();
        if item(ui, "Bevel", "Ctrl+B") {
            let w = app.bevel_width;
            let r = app.bevel_selected(w);
            super::report(app, r);
        }
        if item(ui, "Bridge", "2 faces") {
            let r = app.bridge_selected();
            super::report(app, r);
        }
        if item(ui, "Merge (to center)", "") {
            let r = app.merge_selected(None);
            super::report(app, r);
        }
        if item(ui, "Insert Edge Loop on Selected Edges", "") {
            let r = app.split_ring_selected(0.5);
            super::report(app, r);
        }
    });
    ui.menu_button("Mesh Tools", |ui| {
        if item(ui, "Insert Edge Loop", "Ctrl+R") {
            app.tool = crate::app::Tool::EdgeLoop;
        }
        if item(ui, "Multi-Cut", "click edges, Enter") {
            app.tool = crate::app::Tool::MultiCut;
        }
        if item(ui, "Sculpt", "") {
            app.tool = crate::app::Tool::Sculpt;
        }
        ui.checkbox(&mut app.soft_select, "Soft Select (B)");
        if item(ui, "Quad Draw", "on a live mesh") {
            // The selected mesh becomes live (unless one already is).
            if app.qd.live.is_none()
                && let Some(id) = app.lead().filter(|i| app.scene.node(*i).is_some_and(|n| n.mesh().is_some()))
            {
                let _ = app.qd_make_live(Some(id));
            }
            app.qd.target = None;
            app.tool = crate::app::Tool::QuadDraw;
        }
    });
    ui.menu_button("Mesh Display", |ui| {
        if ui.button("Reverse (normals)").clicked() {
            app.reverse_normals();
        }
        let sel = app.sel.clone();
        if item(ui, "Soften Edge", "selected edges, or meshes") {
            let r = app.soft_edge(&sel, 180.0).map(|n| format!("Softened {n} edges"));
            super::report(app, r);
        }
        if item(ui, "Harden Edge", "") {
            let r = app.soft_edge(&sel, 0.0).map(|n| format!("Hardened {n} edges"));
            super::report(app, r);
        }
        ui.horizontal(|ui| {
            let id = egui::Id::unique("normal_angle");
            let mut a: f32 = ui.data_mut(|d| *d.get_temp_mut_or(id, 30.0));
            if ui.button("Set Normal Angle").on_hover_text("Edges whose faces meet at more than this angle become hard").clicked() {
                let r = app.soft_edge(&sel, a).map(|n| format!("Normal angle {a}° on {n} edges"));
                super::report(app, r);
            }
            ui.add(egui::DragValue::new(&mut a).range(0.0..=180.0).suffix("°"));
            ui.data_mut(|d| d.insert_temp(id, a));
        });
    });
    ui.menu_button("Curves", |ui| curves_menu(app, ui));
    ui.menu_button("Surfaces", |ui| surfaces_menu(app, ui));
    super::rigging::deform_menu(app, ui);
    ui.menu_button("UV", |ui| uv_menu(app, ui));
    ui.menu_button("Generate", |ui| planned(ui, "Not available"));
}

fn uv_menu(app: &mut App, ui: &mut egui::Ui) {
    use crate::uv_ops::Projection as P;
    if item(ui, "UV Editor", "") {
        app.panels_uv_editor = true;
    }
    ui.separator();
    let run = |app: &mut App, r: Result<usize, String>| {
        if let Err(e) = r {
            app.warn(e);
        }
    };
    if item(ui, "Automatic", "") {
        let r = app.uv_project(P::Automatic);
        run(app, r);
    }
    ui.menu_button("Planar", |ui| {
        for (l, a) in [("Best Plane", 3), ("X Axis", 0), ("Y Axis", 1), ("Z Axis", 2)] {
            if ui.button(l).clicked() {
                let r = app.uv_project(P::Planar(a));
                run(app, r);
            }
        }
    });
    if item(ui, "Cylindrical", "") {
        let r = app.uv_project(P::Cylindrical);
        run(app, r);
    }
    if item(ui, "Spherical", "") {
        let r = app.uv_project(P::Spherical);
        run(app, r);
    }
    ui.separator();
    if item(ui, "Layout", "") {
        let r = app.uv_layout();
        run(app, r);
    }
    if item(ui, "Normalize", "") {
        let r = app.uv_normalize();
        run(app, r);
    }
    if item(ui, "Flip U", "") {
        let r = app.uv_transform(glam::Vec2::ZERO, glam::Vec2::new(-1.0, 1.0), 0.0, None, false);
        run(app, r);
    }
    if item(ui, "Flip V", "") {
        let r = app.uv_transform(glam::Vec2::ZERO, glam::Vec2::new(1.0, -1.0), 0.0, None, false);
        run(app, r);
    }
    ui.separator();
    let v = app.active_view;
    ui.checkbox(&mut app.views[v].uv_checker, "UV Checker in viewport");
}

fn animation_menus(app: &mut App, ui: &mut egui::Ui) {
    ui.menu_button("Key", |ui| {
        if item(ui, "Set Key", "S") {
            app.key_selected(&Channel::ALL);
        }
        if item(ui, "Key Translate", "Shift+W") {
            app.key_selected(&Channel::translate());
        }
        if item(ui, "Key Rotate", "Shift+E") {
            app.key_selected(&Channel::rotate());
        }
        if item(ui, "Key Scale", "Shift+R") {
            app.key_selected(&Channel::scale());
        }
        ui.separator();
        if item(ui, "Delete Key at Current Time", "") {
            app.delete_keys_selected(true);
        }
        if item(ui, "Delete All Keys", "") {
            app.delete_keys_selected(false);
        }
        if item(ui, "Set Driven Key…", "") {
            app.panels_sdk = true;
        }
        ui.separator();
        ui.menu_button("Tangents", |ui| {
            for k in crate::scene::TanKind::MENU {
                if ui.button(k.name()).clicked() {
                    app.set_key_tangents(k);
                }
            }
        });
        if item(ui, "Copy Keys", "") {
            app.copy_keys();
        }
        if item(ui, "Paste Keys", "at current time") {
            let t = app.scene.time;
            app.paste_keys(t);
        }
        ui.separator();
        ui.checkbox(&mut app.auto_key, "Auto Keyframe");
    });
    ui.menu_button("Playback", |ui| {
        if item(ui, if app.playing { "Stop Playback" } else { "Play Forwards" }, "Alt+V") {
            app.playing = !app.playing;
        }
        if item(ui, "Next Key", ".") {
            app.step_key(true);
        }
        if item(ui, "Previous Key", ",") {
            app.step_key(false);
        }
        if item(ui, "Go to Start", "Shift+Alt+V") {
            let s = app.scene.play_start;
            app.set_time(s);
        }
        ui.checkbox(&mut app.loop_playback, "Loop");
    });
    ui.menu_button("Visualize", |ui| {
        if ui.button("Graph Editor").clicked() {
            app.panels.graph_editor = true;
        }
        if ui.button("Dope Sheet").clicked() {
            app.panels_dope_sheet = true;
        }
        ui.separator();
        if ui.button("Ghost Selected (toggle)").clicked() {
            let on = app.toggle_ghost();
            app.info(if on { "Ghosting on" } else { "Ghosting off" });
        }
        if ui.button("Unghost All").clicked() {
            app.ghosts.clear();
        }
        ui.horizontal(|ui| {
            ui.label("Ghosts before/after/step");
            ui.add(egui::DragValue::new(&mut app.ghost_settings.before).range(0..=10));
            ui.add(egui::DragValue::new(&mut app.ghost_settings.after).range(0..=10));
            ui.add(egui::DragValue::new(&mut app.ghost_settings.step).range(1..=50));
        });
        ui.separator();
        if ui.button("Create Editable Motion Trail (toggle)").on_hover_text("Drag the red key markers in the viewport to edit translate keys").clicked() {
            app.toggle_trail();
        }
        if ui.button("Delete All Motion Trails").clicked() {
            app.trails.clear();
        }
    });
    ui.menu_button("Skeleton", |ui| {
        if ui.button("Pose Library…").clicked() {
            app.panels_pose_lib = true;
        }
        if ui.button("Retarget Animation…").clicked() {
            app.panels_retarget = true;
        }
    });
    super::rigging::deform_menu(app, ui);
    ui.menu_button("Constrain", |ui| planned(ui, "Not available"));
}

/// Render menu (Rendering menu set; also under Arnold).
fn render_menu(app: &mut App, ui: &mut egui::Ui) {
    if item(ui, "Render Current Frame", "") {
        app.start_render();
    }
    if item(ui, "IPR Render Current Frame", "") {
        app.render.ipr = true;
        app.start_render();
    }
    if item(ui, "Render Sequence…", "") {
        app.render.open = true;
        app.render.last_message = "Open Sequence… in the Render View to set frames and output.".into();
    }
    if item(ui, "Stop Render", "") {
        app.render.ipr = false;
        app.stop_render();
    }
    ui.separator();
    if item(ui, "Render View", "") {
        app.render.open = true;
    }
    ui.menu_button("Render Settings", |ui| {
        let s = &mut app.render.settings;
        ui.horizontal(|ui| {
            ui.label("Width");
            ui.add(egui::DragValue::new(&mut s.width).range(8..=8192));
            ui.label("Height");
            ui.add(egui::DragValue::new(&mut s.height).range(8..=8192));
        });
        ui.horizontal(|ui| {
            ui.label("Samples");
            ui.add(egui::DragValue::new(&mut s.spp).range(1..=16384));
            ui.label("Bounces");
            ui.add(egui::DragValue::new(&mut s.bounces).range(0..=32));
        });
        ui.horizontal(|ui| {
            ui.label("Sky");
            ui.add(egui::DragValue::new(&mut s.env).range(0.0..=10.0).speed(0.02));
            ui.checkbox(&mut s.env_background, "Sky background");
        });
    });
}

// --------------------------------------------------------------- status line

fn icon_btn(ui: &mut egui::Ui, glyph: &str, selected: bool, tip: &str) -> egui::Response {
    ui.add(egui::Button::selectable(selected, RichText::new(glyph).size(14.0))).on_hover_text(tip)
}

pub fn status_line(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("menuset").selected_text(app.menu_set.name()).width(110.0).show_ui(ui, |ui| {
            for m in MenuSet::ALL {
                ui.selectable_value(&mut app.menu_set, m, m.name());
            }
        });
        ui.separator();
        if icon_btn(ui, "🗋", false, "New scene (Ctrl+N)").clicked() {
            actions.push(Action::New);
        }
        if icon_btn(ui, "🗁", false, "Open scene (Ctrl+O)").clicked() {
            actions.push(Action::Open);
        }
        if icon_btn(ui, "💾", false, "Save scene (Ctrl+S)").clicked() {
            actions.push(Action::Save);
        }
        ui.separator();
        if ui.add_enabled(app.can_undo(), egui::Button::new("⟲")).on_hover_text("Undo (Z)").clicked() {
            app.undo();
        }
        if ui.add_enabled(app.can_redo(), egui::Button::new("⟳")).on_hover_text("Redo (Shift+Z)").clicked() {
            app.redo();
        }
        ui.separator();
        ui.label(RichText::new("Select by").small().weak());
        for (m, g, tip) in [(CompMode::Object, "◼ Object", "Object mode (F8)"), (CompMode::Vertex, "• Vertex", "Vertex mode (F9)"), (CompMode::Edge, "╱ Edge", "Edge mode (F10)"), (CompMode::Face, "▰ Face", "Face mode (F11)")] {
            if icon_btn(ui, g, app.comp_mode == m, tip).clicked() {
                app.set_comp_mode(m);
            }
        }
        ui.separator();
        if icon_btn(ui, "⊞", app.snap_grid, "Snap to grids (hold X)").clicked() {
            app.snap_grid ^= true;
        }
        ui.separator();
        if ui.button("🎬").on_hover_text("Render the current frame (Render View)").clicked() {
            app.start_render();
        }
        if icon_btn(ui, "IPR", app.render.ipr, "Interactive render: re-renders as you edit").clicked() {
            app.render.ipr ^= true;
            if app.render.ipr {
                app.start_render();
            }
        }
        ui.separator();
        if ui.button("✨ AI").on_hover_text("AI Copilot (Ctrl+L)").clicked() {
            app.ai_requests.push(crate::app::AiUiRequest::ToggleCopilot);
        }
        if ui.button("🎤").on_hover_text("Voice command (`)").clicked() {
            app.ai_requests.push(crate::app::AiUiRequest::ToggleVoice);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_btn(ui, "⚙", app.panels.right && app.right_tab == crate::app::RightTab::ToolSettings, "Show/hide Tool Settings").clicked() {
                toggle_tab(app, crate::app::RightTab::ToolSettings);
            }
            if icon_btn(ui, "▤", app.panels.right && app.right_tab == crate::app::RightTab::ChannelBox, "Show/hide Channel Box").clicked() {
                toggle_tab(app, crate::app::RightTab::ChannelBox);
            }
            if icon_btn(ui, "🗐", app.panels.right && app.right_tab == crate::app::RightTab::AttributeEditor, "Show/hide Attribute Editor (Ctrl+A)").clicked() {
                toggle_tab(app, crate::app::RightTab::AttributeEditor);
            }
            if icon_btn(ui, "☰", app.panels.outliner, "Show/hide Outliner").clicked() {
                app.panels.outliner ^= true;
            }
            ui.separator();
            let before = app.workspace;
            egui::ComboBox::from_id_salt("workspace").selected_text(app.workspace.name()).width(140.0).show_ui(ui, |ui| {
                for w in Workspace::ALL {
                    ui.selectable_value(&mut app.workspace, w, w.name());
                }
            });
            if before != app.workspace {
                apply_workspace(app);
            }
            ui.label("Workspace:");
        });
    });
}

fn toggle_tab(app: &mut App, tab: crate::app::RightTab) {
    if app.panels.right && app.right_tab == tab {
        app.panels.right = false;
    } else {
        app.panels.right = true;
        app.right_tab = tab;
    }
}

pub fn apply_workspace(app: &mut App) {
    use crate::app::RightTab;
    match app.workspace {
        Workspace::MayaClassic => {
            app.panels.outliner = true;
            app.panels.right = true;
            app.right_tab = RightTab::ChannelBox;
            app.panels.graph_editor = false;
            app.menu_set = MenuSet::Modeling;
            app.shelf_tab = 1;
        }
        Workspace::Modeling => {
            app.panels.outliner = true;
            app.panels.right = true;
            app.right_tab = RightTab::AttributeEditor;
            app.panels.graph_editor = false;
            app.menu_set = MenuSet::Modeling;
            app.shelf_tab = 1;
        }
        Workspace::Animation => {
            app.panels.outliner = true;
            app.panels.right = true;
            app.right_tab = RightTab::ChannelBox;
            app.panels.graph_editor = true;
            app.menu_set = MenuSet::Animation;
            app.shelf_tab = 4;
        }
        Workspace::Rigging => {
            app.panels.outliner = true;
            app.panels.right = true;
            app.right_tab = RightTab::ChannelBox;
            app.panels.graph_editor = false;
            app.menu_set = MenuSet::Rigging;
            app.shelf_tab = 3;
        }
    }
}

// --------------------------------------------------------------------- shelf

pub const SHELF_TABS: [&str; 10] = ["Curves / Surfaces", "Poly Modeling", "Sculpting", "Rigging", "Animation", "Rendering", "FX", "MASH", "Custom", "✨ AI"];
pub const AI_SHELF: usize = 9;

fn ai_menu(app: &mut App, ui: &mut egui::Ui) {
    use crate::ai::prompts::Task;
    use crate::app::AiUiRequest as R;
    ui.menu_button("AI", |ui| {
        if item(ui, "Copilot Panel", "Ctrl+L") {
            app.ai_requests.push(R::FocusPrompt);
        }
        if item(ui, "Voice Command (Whisper)", "`") {
            app.ai_requests.push(R::ToggleVoice);
        }
        if item(ui, "Find Menu…", "F3") {
            app.ai_requests.push(R::FindMenu);
        }
        ui.separator();
        if item(ui, "Generate MEL from Description…", "") {
            app.panels.script_editor = true;
        }
        if ui.add_enabled(app.last_mel_error.is_some(), egui::Button::new("Fix Last MEL Error")).clicked() {
            if let Some((script, err)) = app.last_mel_error.clone() {
                app.ai_requests.push(R::Task(Task::FixMel, format!("Script:\n{script}\n\nError: {err}")));
            }
        }
        ui.separator();
        if item(ui, "Scene Health Check…", "") {
            app.ai_requests.push(R::HealthCheck);
        }
        if ui.add_enabled(!app.sel.is_empty(), egui::Button::new("Smart Rename Selected")).clicked() {
            let d = crate::ai::context::selection_details(app);
            app.ai_requests.push(R::Task(Task::SmartRename, d));
        }
        if ui.add_enabled(!app.sel.is_empty(), egui::Button::new("Color Selected (Palette)…")).clicked() {
            app.ai_requests.push(R::PaletteDialog);
        }
        ui.separator();
        ui.checkbox(&mut app.cmd_ai_mode, "Command Line Uses AI");
        if item(ui, "AI Settings…", "") {
            app.ai_requests.push(R::Settings);
        }
    });
}

fn shelf_button(ui: &mut egui::Ui, label: &str, accent: Color32, tip: &str, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 34.0), if enabled { Sense::click() } else { Sense::hover() });
    let bg = if !enabled {
        Color32::from_rgb(60, 60, 60)
    } else if resp.is_pointer_button_down_on() {
        Color32::from_rgb(82, 133, 166)
    } else if resp.hovered() {
        Color32::from_rgb(95, 95, 95)
    } else {
        Color32::from_rgb(76, 76, 76)
    };
    let p = ui.painter();
    p.rect_filled(rect, 3.0, bg);
    p.rect_filled(egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), egui::vec2(rect.width() - 6.0, 3.0)), 1.0, if enabled { accent } else { Color32::from_rgb(80, 80, 80) });
    let tc = if enabled { Color32::from_rgb(225, 225, 225) } else { Color32::from_rgb(120, 120, 120) };
    p.text(rect.center() + egui::vec2(0.0, 3.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(10.5), tc);
    let resp = if enabled { resp.on_hover_text(tip) } else { resp.on_hover_text(format!("{tip} — planned")) };
    enabled && resp.clicked()
}

pub fn shelf(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, t) in SHELF_TABS.iter().enumerate() {
            let sel = app.shelf_tab == i;
            let txt = RichText::new(*t).size(11.5);
            if ui.add(egui::Button::selectable(sel, txt)).clicked() {
                app.shelf_tab = i;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let poly = Color32::from_rgb(90, 170, 230);
        let edit = Color32::from_rgb(230, 160, 60);
        let anim = Color32::from_rgb(220, 80, 80);
        let rig = Color32::from_rgb(120, 210, 110);
        match app.shelf_tab {
            1 => {
                for (l, p, t) in [
                    ("Sphere", Primitive::default_sphere(), "Polygon Sphere"),
                    ("Cube", Primitive::default_cube(), "Polygon Cube"),
                    ("Cyl", Primitive::default_cylinder(), "Polygon Cylinder"),
                    ("Cone", Primitive::default_cone(), "Polygon Cone"),
                    ("Plane", Primitive::default_plane(), "Polygon Plane"),
                    ("Torus", Primitive::default_torus(), "Polygon Torus"),
                ] {
                    if shelf_button(ui, l, poly, t, true) {
                        app.create_primitive(p);
                    }
                }
                ui.separator();
                if shelf_button(ui, "Combine", edit, "Combine selected meshes", true) {
                    app.combine();
                }
                if shelf_button(ui, "Separ.", edit, "Separate mesh into shells", true) {
                    app.separate();
                }
                if shelf_button(ui, "Smooth", edit, "Smooth (Catmull-Clark)", true) {
                    app.smooth();
                }
                if shelf_button(ui, "Extrude", edit, "Extrude faces (Ctrl+E)", true) {
                    app.extrude();
                }
                if shelf_button(ui, "Tri", edit, "Triangulate", true) {
                    app.triangulate();
                }
                if shelf_button(ui, "Bevel", edit, "Bevel selected edges (or whole object)", true) {
                    let w = app.bevel_width;
                    let r = app.bevel_selected(w);
                    super::report(app, r);
                }
                if shelf_button(ui, "Bridge", edit, "Bridge two selected faces", true) {
                    let r = app.bridge_selected();
                    super::report(app, r);
                }
                if shelf_button(ui, "Loop", edit, "Insert Edge Loop tool (Ctrl+R)", true) {
                    app.tool = crate::app::Tool::EdgeLoop;
                }
                if shelf_button(ui, "MCut", edit, "Multi-Cut tool: click points on edges, Enter to cut", true) {
                    app.tool = crate::app::Tool::MultiCut;
                }
                if shelf_button(ui, "Merge", edit, "Merge selected components to center", true) {
                    let r = app.merge_selected(None);
                    super::report(app, r);
                }
                if shelf_button(ui, "Mirror", edit, "Mirror geometry (axis in Tool Settings)", true) {
                    let r = app.mirror_selected(app.mirror_axis, 0.001);
                    super::report(app, r);
                }
                ui.separator();
                if shelf_button(ui, "Freeze", rig, "Freeze Transformations", true) {
                    app.freeze_transforms();
                }
                if shelf_button(ui, "DelHist", rig, "Delete History", true) {
                    app.delete_history();
                }
            }
            3 => {
                use super::rigging as rg;
                use crate::rig::ConstraintKind as K;
                if shelf_button(ui, "Joint", rig, "Create Joints tool (click to place, Enter to finish)", true) {
                    rg::start_joint_tool(app);
                }
                if shelf_button(ui, "Orient", rig, "Orient Joint (xyz)", !app.sel.is_empty()) {
                    let sel = app.sel.clone();
                    app.orient_joints(&sel);
                }
                if shelf_button(ui, "IK", rig, "IK Handle: select start joint, then end joint", app.sel.len() >= 2) {
                    rg::ik_from_selection(app);
                }
                if shelf_button(ui, "Pole", rig, "Pole Vector: select pole object, then IK handle", app.sel.len() == 2) {
                    rg::pole_from_selection(app);
                }
                ui.separator();
                if shelf_button(ui, "Bind", rig, "Bind Skin: select joints + mesh", app.sel.len() >= 2) {
                    rg::bind_selected(app);
                }
                if shelf_button(ui, "Paint", rig, "Paint Skin Weights", true) {
                    rg::paint_weights_tool(app);
                }
                if shelf_button(ui, "BindPose", rig, "Go to Bind Pose", true) {
                    let sel = app.sel.clone();
                    app.goto_bind_pose(&sel);
                }
                ui.separator();
                if shelf_button(ui, "ParCon", rig, "Parent Constraint (maintain offset): drivers, then target", app.sel.len() >= 2) {
                    rg::constrain_selected(app, K::Parent, true);
                }
                if shelf_button(ui, "PtCon", rig, "Point Constraint: drivers, then target", app.sel.len() >= 2) {
                    rg::constrain_selected(app, K::Point, false);
                }
                if shelf_button(ui, "OriCon", rig, "Orient Constraint (maintain offset)", app.sel.len() >= 2) {
                    rg::constrain_selected(app, K::Orient, true);
                }
                if shelf_button(ui, "AimCon", rig, "Aim Constraint: X axis aims at the driver", app.sel.len() >= 2) {
                    rg::constrain_selected(app, K::Aim, false);
                }
                ui.separator();
                if shelf_button(ui, "Circle", rig, "Circle control at the selection", true) {
                    rg::control_at_selection(app, "circle");
                }
                if shelf_button(ui, "Cube", rig, "Cube control at the selection", true) {
                    rg::control_at_selection(app, "cube");
                }
                ui.separator();
                if shelf_button(ui, "Mirror", rig, "Mirror Joints (L_ → R_)", !app.sel.is_empty()) {
                    rg::mirror_joints_selected(app);
                }
                if shelf_button(ui, "MirW", rig, "Mirror Skin Weights (+X → −X)", !app.sel.is_empty()) {
                    rg::mirror_weights_selected(app, true);
                }
                if shelf_button(ui, "Blend", rig, "Blend Shape: targets, then base", app.sel.len() >= 2) {
                    rg::blend_shape_selected(app);
                }
                if shelf_button(ui, "Cluster", rig, "Cluster from selected vertices", true) {
                    rg::cluster_selected(app);
                }
                if shelf_button(ui, "QuickRig", rig, "One-click biped rig for the selected mesh", true) {
                    rg::quick_rig_selected(app);
                }
                if shelf_button(ui, "SDK", rig, "Set Driven Key window", true) {
                    app.panels_sdk = true;
                }
                if shelf_button(ui, "Locator", rig, "Create Locator", true) {
                    app.create_locator();
                }
                if shelf_button(ui, "Group", rig, "Group (Ctrl+G)", true) {
                    app.group_selected();
                }
                if shelf_button(ui, "Parent", rig, "Parent (P)", true) {
                    app.parent_selected();
                }
            }
            4 => {
                if shelf_button(ui, "Key", anim, "Set Key (S)", true) {
                    app.key_selected(&Channel::ALL);
                }
                if shelf_button(ui, "Key T", anim, "Key Translate (Shift+W)", true) {
                    app.key_selected(&Channel::translate());
                }
                if shelf_button(ui, "Key R", anim, "Key Rotate (Shift+E)", true) {
                    app.key_selected(&Channel::rotate());
                }
                if shelf_button(ui, "Key S", anim, "Key Scale (Shift+R)", true) {
                    app.key_selected(&Channel::scale());
                }
                if shelf_button(ui, "DelKey", anim, "Delete key at current time", true) {
                    app.delete_keys_selected(true);
                }
                ui.separator();
                if shelf_button(ui, "Graph", anim, "Graph Editor", true) {
                    app.panels.graph_editor ^= true;
                }
                if shelf_button(ui, if app.playing { "Stop" } else { "Play" }, anim, "Toggle playback (Alt+V)", true) {
                    app.playing ^= true;
                }
                if shelf_button(ui, "AutoKey", anim, "Toggle Auto Keyframe", true) {
                    app.auto_key ^= true;
                }
            }
            8 => {
                if app.custom_shelf.is_empty() {
                    ui.label(RichText::new("Empty shelf — use \"Save to Shelf\" in the Script Editor to add buttons.").weak());
                }
                let items = app.custom_shelf.clone();
                for (label, cmd) in items {
                    if shelf_button(ui, &label, Color32::from_rgb(200, 200, 90), &cmd, true) {
                        let r = crate::commands::execute(app, &cmd);
                        super::report(app, r);
                    }
                }
            }
            AI_SHELF => {
                use crate::app::AiUiRequest as R;
                let ai = Color32::from_rgb(190, 120, 240);
                if shelf_button(ui, "Copilot", ai, "Toggle the AI Copilot panel", true) {
                    app.ai_requests.push(R::ToggleCopilot);
                }
                if shelf_button(ui, "🎤 Voice", ai, "Voice command (`)", true) {
                    app.ai_requests.push(R::ToggleVoice);
                }
                if shelf_button(ui, "Find", ai, "Find Menu (F3)", true) {
                    app.ai_requests.push(R::FindMenu);
                }
                if shelf_button(ui, "Health", ai, "Scene Health Check", true) {
                    app.ai_requests.push(R::HealthCheck);
                }
                if shelf_button(ui, "Rename", ai, "Smart Rename selected", !app.sel.is_empty()) {
                    let d = crate::ai::context::selection_details(app);
                    app.ai_requests.push(R::Task(crate::ai::prompts::Task::SmartRename, d));
                }
                if shelf_button(ui, "Palette", ai, "Color selected from a mood", !app.sel.is_empty()) {
                    app.ai_requests.push(R::PaletteDialog);
                }
                ui.separator();
                if app.ai_shelf.is_empty() {
                    ui.label(RichText::new("Save prompts here with ⭐ in the Copilot.").weak());
                }
                let mut remove = None;
                for (i, (label, prompt)) in app.ai_shelf.clone().into_iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 34.0), Sense::click());
                    let bg = if resp.hovered() { Color32::from_rgb(95, 95, 95) } else { Color32::from_rgb(76, 76, 76) };
                    ui.painter().rect_filled(rect, 3.0, bg);
                    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), egui::vec2(34.0, 3.0)), 1.0, Color32::from_rgb(240, 200, 80));
                    ui.painter().text(rect.center() + egui::vec2(0.0, 3.0), egui::Align2::CENTER_CENTER, &label, egui::FontId::proportional(10.0), Color32::from_rgb(225, 225, 225));
                    let resp = resp.on_hover_text(format!("✨ {prompt}"));
                    if resp.clicked() {
                        app.ai_requests.push(R::Prompt(prompt.clone()));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Remove from shelf").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    app.ai_shelf.remove(i);
                }
            }
            2 => {
                use crate::sculpt::Brush;
                let acc = Color32::from_rgb(200, 120, 220);
                for b in Brush::ALL {
                    let on = app.tool == crate::app::Tool::Sculpt && app.sculpt.brush == b;
                    let label = if on { format!("[{}]", b.name()) } else { b.name().to_string() };
                    if shelf_button(ui, &label, acc, &format!("{} brush (LMB-drag on a mesh; Ctrl inverts)", b.name()), true) {
                        app.sculpt.brush = b;
                        app.tool = crate::app::Tool::Sculpt;
                    }
                }
                ui.separator();
                if shelf_button(ui, if app.sculpt.symmetry_x { "Sym X✔" } else { "Sym X" }, acc, "Toggle X symmetry", true) {
                    app.sculpt.symmetry_x ^= true;
                }
                if shelf_button(ui, if app.soft_select { "Soft✔" } else { "Soft" }, edit, "Soft Select (B)", true) {
                    app.soft_select ^= true;
                }
                ui.separator();
                if shelf_button(ui, "Lattice", rig, "Lattice deformer on selected meshes", !app.sel.is_empty()) {
                    let r = app.create_lattice(app.lattice_div, None).map(|_| String::new());
                    super::report(app, r);
                }
                if shelf_button(ui, "Wrap", rig, "Wrap: targets, then the influence mesh", app.sel.len() >= 2) {
                    let mut sel = app.sel.clone();
                    let d = sel.pop().unwrap();
                    let r = app.create_wrap(d, &sel).map(|_| String::new());
                    super::report(app, r);
                }
            }
            5 => {
                let acc = Color32::from_rgb(230, 170, 70);
                super::lookdev::rendering_shelf(app, ui, &mut |ui, l, t, e| shelf_button(ui, l, acc, t, e));
            }
            0 => {
                let acc = Color32::from_rgb(110, 200, 120);
                if shelf_button(ui, "CV", acc, "CV Curve Tool", true) {
                    app.curve_tool.points.clear();
                    app.tool = crate::app::Tool::CurveCv;
                }
                if shelf_button(ui, "EP", acc, "EP Curve Tool", true) {
                    app.curve_tool.points.clear();
                    app.tool = crate::app::Tool::CurveEp;
                }
                if shelf_button(ui, "Circle", acc, "NURBS circle", true) {
                    let (r, s) = (app.curve_opts.circle_radius, app.curve_opts.circle_sections as usize);
                    app.create_circle(r, s, Vec3::Y, Vec3::ZERO, None);
                }
                let sel = app.sel.clone();
                let o = app.curve_opts.clone();
                if shelf_button(ui, "Revolve", acc, "Revolve the selected curves around Y", !sel.is_empty()) {
                    let r = app.revolve_curves(&sel, Vec3::Y, Vec3::ZERO, o.revolve_sections, 0.0, 360.0, o.samples, true, None).map(|_| String::new());
                    super::report(app, r);
                }
                if shelf_button(ui, "Loft", acc, "Loft through the selected curves", sel.len() >= 2) {
                    let r = app.loft_curves(&sel, o.loft_sections, o.loft_close, o.samples, true, None).map(|_| String::new());
                    super::report(app, r);
                }
                if shelf_button(ui, "Extrude", acc, "Extrude profile (along a path if two curves are selected)", !sel.is_empty()) {
                    let r = app.extrude_curve(sel[0], sel.get(1).copied(), o.extrude_length, Vec3::ZERO, o.samples, true, None).map(|_| String::new());
                    super::report(app, r);
                }
                if shelf_button(ui, "Planar", acc, "Fill a closed curve", !sel.is_empty()) {
                    let r = app.planar_curve(sel[0], o.samples, true, None).map(|_| String::new());
                    super::report(app, r);
                }
            }
            6 => {
                use crate::fx::EmitterKind as K;
                let acc = Color32::from_rgb(240, 150, 70);
                if shelf_button(ui, "Emit", acc, "Omni particle emitter", true) {
                    emit(app, K::Omni, false);
                }
                if shelf_button(ui, "EmitObj", acc, "Emit from the selected mesh or curve", app.lead().is_some()) {
                    emit(app, K::Omni, true);
                }
                if shelf_button(ui, "nCloth", acc, "Make the selected meshes cloth", !app.sel.is_empty()) {
                    let sel = app.sel.clone();
                    let r = app.create_cloth(&sel, |_| {}).map(|_| String::new());
                    super::report(app, r);
                }
                if shelf_button(ui, "Pin", acc, "Pin the selected cloth vertices", app.comp_mode != CompMode::Object) {
                    let mut r = Ok(String::new());
                    for (id, vs) in app.comp_vertices() {
                        r = app.pin_cloth(id, &vs, None, false).map(|n| format!("{n} pinned"));
                    }
                    super::report(app, r);
                }
                if shelf_button(ui, "Collide", acc, "Colliders, then the particles / cloth", app.sel.len() >= 2) {
                    make_collide(app, false);
                }
                if shelf_button(ui, "Air", acc, "Air (wind) field — acts on the selected simulations, or all", true) {
                    add_field(app, crate::fx::FieldKind::Air);
                }
                if shelf_button(ui, "Turb", acc, "Turbulence field", true) {
                    add_field(app, crate::fx::FieldKind::Turbulence);
                }
                if shelf_button(ui, "Vortex", acc, "Vortex field", true) {
                    add_field(app, crate::fx::FieldKind::Vortex);
                }
                if shelf_button(ui, "Hair", acc, "Grow hair on the selected mesh", app.lead().is_some()) {
                    let mesh = app.sel.iter().copied().find(|i| app.scene.node(*i).is_some_and(|n| n.mesh().is_some()));
                    let r = match mesh {
                        Some(m) => app.create_hair(m, crate::hair::HairSettings { length: 0.0, width: 0.0, thickness: 0.0, ..Default::default() }, None).map(|_| String::new()),
                        None => Err("Select a polygon mesh.".into()),
                    };
                    super::report(app, r);
                }
                if shelf_button(ui, "DynCrv", acc, "Make the selected curves dynamic", app.lead().is_some()) {
                    let sel = app.sel.clone();
                    let r = sel.iter().try_for_each(|c| app.make_curve_dynamic(*c, false, None).map(|_| ())).map(|_| String::new());
                    super::report(app, r);
                }
                shelf_button(ui, "Fluid", Color32::from_rgb(150, 150, 150), "Fluid", false);
            }
            _ => {
                let names: &[&str] = &["Network", "Distrib", "Repro"];
                for n in names {
                    shelf_button(ui, n, Color32::from_rgb(150, 150, 150), n, false);
                }
            }
        }
    });
}
