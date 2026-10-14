//! Forge Design — open-source raster image editor.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod canvas;
mod dialogs;
mod icons;
mod panels;
mod text;
mod tools;

fn main() -> eframe::Result {
    let files: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Forge Design")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("icon-64.png")).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native("Forge Design", options, Box::new(|cc| Ok(Box::new(app::ForgeApp::new(cc, files)))))
}
