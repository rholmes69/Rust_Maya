//! Rust Maya — a Maya-style 3D DCC application built with egui + wgpu.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod anim_ops;
mod app;
mod batch;
mod camera;
mod commands;
mod curve_ops;
mod deform;
mod director;
mod export;
mod fbx;
mod fluid;
mod fx;
mod hair;
mod importer;
mod look;
mod mesh;
mod mesh_ops;
mod nurbs;
mod modeling_ops;
mod prefs;
mod quad_draw;
mod raytrace;
mod recipes;
mod reduce;
mod render;
mod render_view;
mod rig;
mod rig_report;
mod posing;
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--render" | "-render") => {
            batch::attach_console();
            std::process::exit(batch::run(&args[1..]));
        }
        Some("--help" | "-h" | "/?") => {
            batch::attach_console();
            print!("{}", batch::HELP);
            return Ok(());
        }
        Some("--version" | "-v") => {
            batch::attach_console();
            println!("Rust Maya {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }
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
