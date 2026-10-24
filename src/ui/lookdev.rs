//! Look-dev UI: Hypershade-lite window, material / light / camera Attribute Editor sections,
//! Create › Lights / Cameras menus and the Rendering shelf.

use crate::app::App;
use crate::look::{LightKind, MatKind, Material};
use crate::scene::{NodeId, Shape};
use eframe::egui::{self, Color32, RichText};

fn c32(c: [f32; 3]) -> Color32 {
    Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8)
}

/// Shaded-ball swatch (simple lambert + highlight preview of the material).
pub fn swatch(ui: &mut egui::Ui, m: &Material, size: f32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, if selected { Color32::from_rgb(82, 133, 166) } else { Color32::from_rgb(45, 45, 45) });
    let c = rect.center();
    let r = size * 0.38;
    // Concentric rings fake a lit sphere.
    let steps = 14;
    for i in 0..steps {
        let t = i as f32 / (steps - 1) as f32;
        let rr = r * (1.0 - t * 0.85);
        let off = egui::vec2(-r * 0.3 * t, -r * 0.3 * t);
        let k = 0.35 + 0.65 * t.powf(0.7);
        let base = [m.color[0] * k * (1.0 - m.metalness * 0.3), m.color[1] * k * (1.0 - m.metalness * 0.3), m.color[2] * k * (1.0 - m.metalness * 0.3)];
        let col = [base[0] + m.emission[0], base[1] + m.emission[1], base[2] + m.emission[2]];
        let a = (255.0 * (1.0 - m.transparency * 0.7)) as u8;
        let cc = c32(col);
        p.circle_filled(c + off, rr, Color32::from_rgba_unmultiplied(cc.r(), cc.g(), cc.b(), a));
    }
    if m.specular > 0.05 {
        let hr = r * (0.06 + m.roughness * 0.3);
        let a = ((m.specular * (1.0 - m.roughness * 0.7)) * 255.0) as u8;
        p.circle_filled(c + egui::vec2(-r * 0.35, -r * 0.35), hr, Color32::from_rgba_unmultiplied(255, 255, 255, a));
    }
    resp
}

/// Editable material attributes. Returns true if anything changed.
pub fn material_editor(app: &mut App, ui: &mut egui::Ui, id: u32) {
    let Some(mut m) = app.material(id).cloned() else { return };
    let orig = m.clone();
    let mut name = m.name.clone();
    ui.horizontal(|ui| {
        ui.label(format!("{}:", m.kind.name()));
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
        if r.lost_focus() && name != m.name {
            app.rename_material(id, &name);
        }
    });
    let row = |ui: &mut egui::Ui, label: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.horizontal(|ui| {
            ui.add_sized([86.0, 18.0], egui::Label::new(label));
            add(ui);
        });
    };
    row(ui, "Type", &mut |ui| {
        egui::ComboBox::from_id_salt(("mat_kind", id)).selected_text(m.kind.name()).show_ui(ui, |ui| {
            for k in MatKind::ALL {
                ui.selectable_value(&mut m.kind, k, k.name());
            }
        });
    });
    row(ui, "Color", &mut |ui| {
        ui.color_edit_button_rgb(&mut m.color);
        if m.color_map.is_some() {
            ui.label(RichText::new("(texture replaces colour)").small().weak());
        }
    });
    let tex_info: Option<String> = m.color_map.as_ref().map(|p| match app.textures.get(p) {
        Ok(t) => format!("{}×{} source, shown at {}²", t.src_size.0, t.src_size.1, crate::texture::TEX_SIZE),
        Err(e) => format!("not loaded: {e}"),
    });
    let mut map_action: Option<Option<String>> = None;
    let mut reload = false;
    row(ui, "Color Map", &mut |ui| {
        let label = match &m.color_map {
            Some(p) if p == crate::texture::CHECKER => "UV checker".to_string(),
            Some(p) => std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(p.clone()),
            None => "none".to_string(),
        };
        ui.label(RichText::new(label).monospace()).on_hover_text(format!("{}\n{}", m.color_map.clone().unwrap_or_default(), tex_info.clone().unwrap_or_default()));
        if ui.small_button("File…").on_hover_text("Choose an image (PNG, JPEG, BMP, TGA)").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tga"]).pick_file()
        {
            map_action = Some(Some(p.display().to_string()));
        }
        if ui.small_button("Checker").clicked() {
            map_action = Some(Some(crate::texture::CHECKER.to_string()));
        }
        if m.color_map.is_some() {
            if ui.small_button("Reload").on_hover_text("Re-read the file from disk").clicked() {
                reload = true;
            }
            if ui.small_button("✖").on_hover_text("Disconnect the texture").clicked() {
                map_action = Some(None);
            }
        }
    });
    if m.color_map.is_some() {
        row(ui, "Repeat UV", &mut |ui| {
            ui.add(egui::DragValue::new(&mut m.repeat_uv[0]).speed(0.05).range(0.01..=1000.0));
            ui.add(egui::DragValue::new(&mut m.repeat_uv[1]).speed(0.05).range(0.01..=1000.0));
        });
    }
    let mut slot_actions: Vec<(crate::look::MapSlot, Option<String>)> = Vec::new();
    for slot in [crate::look::MapSlot::Normal, crate::look::MapSlot::Roughness, crate::look::MapSlot::Metalness, crate::look::MapSlot::Occlusion, crate::look::MapSlot::Emission] {
        let cur = m.map(slot).clone();
        row(ui, slot.label(), &mut |ui| {
            let label = cur.as_ref().map(|p| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(p.clone())).unwrap_or_else(|| "none".into());
            ui.label(RichText::new(label).monospace()).on_hover_text(cur.clone().unwrap_or_default());
            let tip = match slot {
                crate::look::MapSlot::Normal => "Tangent-space normal map (OpenGL / glTF convention). Shown with Textured (6) and in renders.",
                crate::look::MapSlot::Roughness => "Greyscale image that multiplies Roughness (set Roughness to 1 for the map's values).",
                crate::look::MapSlot::Metalness => "Greyscale image that multiplies Metalness (set Metalness to 1 for the map's values).",
                crate::look::MapSlot::Occlusion => "Greyscale ambient-occlusion map: darkens ambient and bounced light in crevices.",
                _ => "Colour image that multiplies Incandescence (set Incandescence to white for the map's colours).",
            };
            if ui.small_button("File…").on_hover_text(tip).clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tga"]).pick_file()
            {
                slot_actions.push((slot, Some(p.display().to_string())));
            }
            if cur.is_some() && ui.small_button("✖").on_hover_text("Disconnect").clicked() {
                slot_actions.push((slot, None));
            }
        });
    }
    row(ui, "Specular", &mut |ui| {
        ui.add(egui::Slider::new(&mut m.specular, 0.0..=1.0));
    });
    row(ui, "Roughness", &mut |ui| {
        ui.add(egui::Slider::new(&mut m.roughness, 0.0..=1.0));
    });
    if m.kind == MatKind::StandardSurface {
        row(ui, "Metalness", &mut |ui| {
            ui.add(egui::Slider::new(&mut m.metalness, 0.0..=1.0));
        });
    }
    row(ui, "Incandescence", &mut |ui| {
        ui.color_edit_button_rgb(&mut m.emission);
    });
    row(ui, "Transparency", &mut |ui| {
        ui.add(egui::Slider::new(&mut m.transparency, 0.0..=1.0));
    });
    if m != orig {
        app.edit_checkpoint(egui::Id::unique(("mat_edit", id)));
        if let Some(x) = app.material_mut(id) {
            let nm = x.name.clone();
            let map = x.color_map.clone();
            let (nmap, rmap, mmap, omap, emap) = (x.normal_map.clone(), x.roughness_map.clone(), x.metalness_map.clone(), x.occlusion_map.clone(), x.emission_map.clone());
            *x = Material { name: nm, color_map: map, normal_map: nmap, roughness_map: rmap, metalness_map: mmap, occlusion_map: omap, emission_map: emap, ..m };
        }
    }
    if reload && let Some(p) = app.material(id).and_then(|x| x.color_map.clone()) {
        app.reload_texture(&p);
        ui.ctx().data_mut(|d| d.remove::<egui::TextureHandle>(egui::Id::unique(("uv_bg", p.as_str()))));
    }
    if let Some(a) = map_action
        && let Err(e) = app.set_color_map(id, a)
    {
        app.warn(e);
    }
    for (slot, a) in slot_actions {
        if let Err(e) = app.set_material_map(id, slot, a) {
            app.warn(e);
        }
    }
}

pub fn create_material_ui(app: &mut App, kind: MatKind, assign: bool) -> u32 {
    let id = app.create_material(kind, None);
    if assign && !app.sel.is_empty() {
        let sel = app.sel.clone();
        // Seed the new material with the object's current colour.
        if let Some(c) = sel.first().map(|s| app.surface_color(*s)) {
            app.material_mut(id).unwrap().color = c;
        }
        app.assign_material(Some(id), &sel);
    }
    app.hs_selected = Some(id);
    id
}

/// Hypershade-lite: material browser + editor.
pub fn hypershade_window(app: &mut App, ctx: &egui::Context) {
    if !app.panels_hypershade {
        return;
    }
    let mut open = true;
    egui::Window::new("Hypershade").open(&mut open).default_size([560.0, 420.0]).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Create:").strong());
            for k in MatKind::ALL {
                if ui.button(k.name()).on_hover_text("Create a material (assigned to the selection if any)").clicked() {
                    create_material_ui(app, k, true);
                }
            }
        });
        ui.separator();
        let mats: Vec<Material> = app.scene.materials.clone();
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.label(RichText::new("Materials").strong());
            egui::ScrollArea::vertical().id_salt("hs_list").max_height(330.0).show(ui, |ui| {
                // Default lambert1 (per-object colour).
                ui.horizontal(|ui| {
                    let dflt = Material { name: "lambert1".into(), color: [0.62, 0.62, 0.62], ..Default::default() };
                    if swatch(ui, &dflt, 44.0, app.hs_selected.is_none()).clicked() {
                        app.hs_selected = None;
                    }
                    ui.vertical(|ui| {
                        ui.label("lambert1");
                        ui.label(RichText::new("default · per-object colour").small().weak());
                    });
                });
                egui::Grid::new("hs_grid").num_columns(1).show(ui, |ui| {
                    for m in &mats {
                        ui.horizontal(|ui| {
                            let sel = app.hs_selected == Some(m.id);
                            let r = swatch(ui, m, 44.0, sel);
                            if r.clicked() {
                                app.hs_selected = Some(m.id);
                            }
                            r.context_menu(|ui| {
                                if ui.button("Assign to selection").clicked() {
                                    let s = app.sel.clone();
                                    app.assign_material(Some(m.id), &s);
                                    ui.close();
                                }
                                if ui.button("Select objects with material").clicked() {
                                    app.sel = app.material_users(m.id);
                                    ui.close();
                                }
                                if ui.button("Duplicate").clicked() {
                                    app.hs_selected = app.duplicate_material(m.id);
                                    ui.close();
                                }
                                if ui.button("Delete").clicked() {
                                    app.delete_material(m.id);
                                    ui.close();
                                }
                            });
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&m.name).strong());
                                ui.label(RichText::new(format!("{} · {} object(s)", m.kind.name(), app.material_users(m.id).len())).small().weak());
                            });
                        });
                        ui.end_row();
                    }
                });
            });
            let ui = &mut cols[1];
            match app.hs_selected.filter(|id| app.material(*id).is_some()) {
                Some(id) => {
                    material_editor(app, ui, id);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(!app.sel.is_empty(), egui::Button::new("Assign to selection")).clicked() {
                            let s = app.sel.clone();
                            let n = app.assign_material(Some(id), &s);
                            app.info(format!("Assigned to {n} object(s)"));
                        }
                        if ui.button("Select objects").clicked() {
                            app.sel = app.material_users(id);
                        }
                        if ui.button("Duplicate").clicked() {
                            app.hs_selected = app.duplicate_material(id);
                        }
                        if ui.button("Delete").clicked() {
                            app.delete_material(id);
                            app.hs_selected = None;
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("lambert1").strong());
                    ui.label(RichText::new("The default material uses each object's own colour (Attribute Editor › Color, or setColor).").weak());
                    if ui.add_enabled(!app.sel.is_empty(), egui::Button::new("Assign lambert1 to selection")).clicked() {
                        let s = app.sel.clone();
                        app.assign_material(None, &s);
                    }
                }
            }
        });
    });
    if !open {
        app.panels_hypershade = false;
    }
}

/// Attribute Editor sections for materials (on meshes), lights and cameras.
pub fn attribute_sections(app: &mut App, ui: &mut egui::Ui, id: NodeId) {
    let Some(node) = app.scene.node(id).cloned() else { return };
    if node.mesh().is_some() {
        let title = match node.material.and_then(|m| app.material(m)) {
            Some(m) => format!("Material: {}", m.name),
            None => "Material: lambert1".into(),
        };
        egui::CollapsingHeader::new(title).id_salt("ae_material").default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Assigned");
                let cur = node.material;
                let cur_name = cur.and_then(|m| app.material(m)).map(|m| m.name.clone()).unwrap_or("lambert1".into());
                let mut pick = cur;
                egui::ComboBox::from_id_salt(("ae_mat_pick", id)).selected_text(cur_name).show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, None, "lambert1 (object colour)");
                    for m in &app.scene.materials {
                        ui.selectable_value(&mut pick, Some(m.id), &m.name);
                    }
                });
                if pick != cur {
                    let mut targets = app.sel.clone();
                    if !targets.contains(&id) {
                        targets = vec![id];
                    }
                    app.assign_material(pick, &targets);
                }
                if ui.small_button("New…").on_hover_text("Create a blinn for the selection").clicked() {
                    create_material_ui(app, MatKind::Blinn, true);
                }
                if ui.small_button("Hypershade").clicked() {
                    app.panels_hypershade = true;
                    app.hs_selected = node.material;
                }
            });
            match node.material.filter(|m| app.material(*m).is_some()) {
                Some(m) => {
                    let users = app.material_users(m).len();
                    if users > 1 {
                        ui.label(RichText::new(format!("Shared by {users} objects — edits affect all of them.")).small().weak());
                    }
                    material_editor(app, ui, m);
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.label("Color");
                        let mut c = node.color;
                        if ui.color_edit_button_rgb(&mut c).changed() {
                            app.edit_checkpoint(egui::Id::unique(("ae_color", id)));
                            app.set_color_selected(c);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (name, c) in [("Grey", [0.62, 0.62, 0.62]), ("Red", [0.8, 0.25, 0.22]), ("Green", [0.3, 0.7, 0.3]), ("Blue", [0.25, 0.45, 0.85]), ("Gold", [0.9, 0.7, 0.25]), ("Skin", [0.9, 0.7, 0.6])] {
                            if ui.add(egui::Button::new(RichText::new(name).color(Color32::BLACK).small()).fill(c32(c))).clicked() {
                                app.checkpoint();
                                app.set_color_selected(c);
                            }
                        }
                    });
                }
            }
        });
    }
    if let Shape::Light(l) = &node.shape {
        egui::CollapsingHeader::new(format!("{} Attributes", l.kind.name())).default_open(true).show(ui, |ui| {
            let mut x = l.clone();
            ui.horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Color"));
                ui.color_edit_button_rgb(&mut x.color);
            });
            ui.horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Intensity"));
                ui.add(egui::DragValue::new(&mut x.intensity).speed(0.02).range(0.0..=1000.0));
            });
            if x.kind == LightKind::Spot {
                ui.horizontal(|ui| {
                    ui.add_sized([86.0, 18.0], egui::Label::new("Cone Angle"));
                    ui.add(egui::Slider::new(&mut x.cone, 1.0..=179.0).suffix("°"));
                });
                ui.horizontal(|ui| {
                    ui.add_sized([86.0, 18.0], egui::Label::new("Penumbra"));
                    ui.add(egui::Slider::new(&mut x.penumbra, 0.0..=45.0).suffix("°"));
                });
            }
            if x.kind == LightKind::Area {
                ui.label(RichText::new("Scale the light to resize it. Soft shadows and falloff appear in the Render View (path tracer).").small().weak());
            }
            if matches!(x.kind, LightKind::Directional | LightKind::Spot) {
                ui.checkbox(&mut x.shadows, "Use Depth Map Shadows");
                if x.shadows {
                    ui.label(RichText::new("Shown in views with Lighting › Use All Lights + Shadows (one caster at a time).").small().weak());
                }
            }
            if &x != l {
                app.edit_checkpoint(egui::Id::unique(("ae_light", id)));
                if let Some(Shape::Light(y)) = app.scene.node_mut(id).map(|n| &mut n.shape) {
                    *y = x;
                }
            }
        });
    }
    if let Shape::Camera(c) = &node.shape {
        egui::CollapsingHeader::new("Camera Attributes").default_open(true).show(ui, |ui| {
            let mut x = c.clone();
            ui.horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Focal Length"));
                ui.add(egui::DragValue::new(&mut x.focal).speed(0.5).range(2.5..=3500.0).suffix(" mm"));
                ui.label(RichText::new(format!("{:.1}° vertical", x.fov_y())).weak());
            });
            ui.horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Clip planes"));
                ui.add(egui::DragValue::new(&mut x.near).speed(0.01).range(0.0001..=100.0));
                ui.add(egui::DragValue::new(&mut x.far).speed(10.0).range(1.0..=1.0e6));
            });
            ui.checkbox(&mut x.dof, "Depth of Field (renders)");
            if x.dof {
                ui.horizontal(|ui| {
                    ui.add_sized([86.0, 18.0], egui::Label::new("F Stop"));
                    ui.add(egui::DragValue::new(&mut x.fstop).speed(0.05).range(0.5..=128.0).prefix("f/"));
                    ui.label("Blur ×");
                    ui.add(egui::DragValue::new(&mut x.blur_scale).speed(0.1).range(0.0..=1000.0)).on_hover_text("Scales the physical blur. Real lenses are subtle at 1 unit = 1 m; raise this for a stronger effect.");
                });
                ui.horizontal(|ui| {
                    ui.add_sized([86.0, 18.0], egui::Label::new("Focus Distance"));
                    ui.add(egui::DragValue::new(&mut x.focus_distance).speed(0.02).range(0.001..=1.0e5));
                    let target = app.sel.iter().copied().find(|s| *s != id);
                    if ui.add_enabled(target.is_some(), egui::Button::new("Focus on selection").small()).on_hover_text("Select the camera, then the object to focus on (Shift-click)").clicked()
                        && let Some(t) = target
                    {
                        let m = app.scene.world_matrix(id);
                        let fwd = m.transform_vector3(glam::Vec3::NEG_Z).normalize_or(glam::Vec3::NEG_Z);
                        let p = app.world_bounds(&[t]).map(|(a, b)| (a + b) * 0.5).unwrap_or(glam::Vec3::ZERO);
                        x.focus_distance = (p - m.transform_point3(glam::Vec3::ZERO)).dot(fwd).max(0.01);
                    }
                });
            }
            if &x != c {
                app.edit_checkpoint(egui::Id::unique(("ae_cam", id)));
                if let Some(Shape::Camera(y)) = app.scene.node_mut(id).map(|n| &mut n.shape) {
                    *y = x;
                }
            }
            let v = app.active_view;
            let through = app.views[v].look_through == Some(id);
            if ui.button(if through { "Stop looking through" } else { "Look Through (active view)" }).clicked() {
                let _ = app.look_through(v, if through { None } else { Some(id) });
            }
        });
    }
}

pub fn lights_menu(app: &mut App, ui: &mut egui::Ui) {
    for (label, k) in [("Ambient Light", LightKind::Ambient), ("Directional Light", LightKind::Directional), ("Point Light", LightKind::Point), ("Spot Light", LightKind::Spot), ("Area Light", LightKind::Area)] {
        if ui.button(label).clicked() {
            app.create_light(k, None);
            if !app.views[app.active_view].all_lights {
                app.info("Light created — press 7 (Lighting › Use All Lights) to see it in the viewport.");
            }
        }
    }
    ui.add_enabled(false, egui::Button::new("Area Light")).on_disabled_hover_text("Planned");
}

pub fn cameras_menu(app: &mut App, ui: &mut egui::Ui) {
    if ui.button("Camera").clicked() {
        app.create_camera(None, 35.0);
    }
    if ui.button("Camera from View").on_hover_text("Create a camera matching the active view").clicked() {
        let v = app.active_view;
        let (eye, fwd) = (app.views[v].cam.eye(), app.views[v].cam.forward());
        let id = app.create_camera(None, 12.0 / (app.views[v].cam.fov_y.to_radians() / 2.0).tan());
        crate::rig::set_world_position(&mut app.scene, id, eye);
        let q = crate::rig::euler_quat(crate::look::aim_euler(fwd));
        crate::rig::set_world_rotation(&mut app.scene, id, q);
    }
}

pub fn rendering_shelf(app: &mut App, ui: &mut egui::Ui, button: &mut dyn FnMut(&mut egui::Ui, &str, &str, bool) -> bool) {
    if button(ui, "Render", "Render the current frame (Render View)", true) {
        app.start_render();
    }
    if button(ui, if app.render.ipr { "IPR✔" } else { "IPR" }, "Interactive render", true) {
        app.render.ipr ^= true;
        if app.render.ipr {
            app.start_render();
        }
    }
    if button(ui, "RView", "Open the Render View", true) {
        app.render.open = true;
    }
    ui.separator();
    if button(ui, "Hyper", "Hypershade", true) {
        app.panels_hypershade = true;
    }
    if button(ui, "Lambert", "New lambert assigned to the selection", true) {
        create_material_ui(app, MatKind::Lambert, true);
    }
    if button(ui, "Blinn", "New blinn assigned to the selection", true) {
        create_material_ui(app, MatKind::Blinn, true);
    }
    if button(ui, "Std", "New standardSurface assigned to the selection", true) {
        create_material_ui(app, MatKind::StandardSurface, true);
    }
    ui.separator();
    for (l, k) in [("DirLt", LightKind::Directional), ("PointLt", LightKind::Point), ("SpotLt", LightKind::Spot), ("AreaLt", LightKind::Area), ("AmbLt", LightKind::Ambient)] {
        if button(ui, l, k.name(), true) {
            app.create_light(k, None);
        }
    }
    if button(ui, "Camera", "Create camera", true) {
        app.create_camera(None, 35.0);
    }
    ui.separator();
    let v = app.active_view;
    if button(ui, "Lights", "Use All Lights (7)", true) {
        app.views[v].all_lights ^= true;
    }
    if button(ui, "Shadows", "Toggle shadows", true) {
        let vw = &mut app.views[v];
        vw.shadows ^= true;
        if vw.shadows {
            vw.all_lights = true;
        }
    }
    if button(ui, "X-Ray", "X-Ray (Alt+X)", true) {
        app.views[v].xray ^= true;
    }
}

/// Rendering menu set › Lighting/Shading.
pub fn lighting_shading_menu(app: &mut App, ui: &mut egui::Ui) {
    if ui.button("Hypershade…").clicked() {
        app.panels_hypershade = true;
    }
    ui.menu_button("Assign New Material", |ui| {
        for k in MatKind::ALL {
            if ui.button(k.name()).clicked() {
                create_material_ui(app, k, true);
            }
        }
    });
    ui.menu_button("Assign Existing Material", |ui| {
        if ui.button("lambert1").clicked() {
            let s = app.sel.clone();
            app.assign_material(None, &s);
        }
        let mats: Vec<(u32, String)> = app.scene.materials.iter().map(|m| (m.id, m.name.clone())).collect();
        for (id, name) in mats {
            if ui.button(name).clicked() {
                let s = app.sel.clone();
                app.assign_material(Some(id), &s);
            }
        }
    });
    ui.separator();
    ui.menu_button("Lights", |ui| lights_menu(app, ui));
    ui.menu_button("Cameras", |ui| cameras_menu(app, ui));
    ui.separator();
    let v = app.active_view;
    ui.checkbox(&mut app.views[v].all_lights, "Use All Lights (7)");
    if ui.checkbox(&mut app.views[v].shadows, "Shadows").changed() && app.views[v].shadows {
        app.views[v].all_lights = true;
    }
    ui.checkbox(&mut app.views[v].xray, "X-Ray (Alt+X)");
}
