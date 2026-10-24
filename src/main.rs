//! Rust Maya — a Maya-style 3D DCC application built with egui + wgpu.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod anim_ops;
mod app;
mod camera;
mod commands;
mod deform;
mod export;
mod importer;
mod look;
mod mesh;
mod mesh_ops;
mod modeling_ops;
mod prefs;
mod raytrace;
mod recipes;
mod render;
mod render_view;
mod rig;
mod rig_ops;
mod scene;
mod sculpt;
mod texture;
mod tts;
mod uv;
mod uv_ops;
mod ui;
mod voice;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Rust Maya 2026")
            .with_inner_size([1600.0, 940.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Rust Maya", options, Box::new(|cc| Ok(Box::new(ui::Shell::new(cc)))))
}
