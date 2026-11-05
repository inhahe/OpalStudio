#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release builds

//! Opal Studio, a 2D graphics editor. See `design.md` for the architecture.

mod app;
mod canvas;
mod commands;
mod config;
mod curve_editor;
mod dialogs;
mod doc;
mod gallery;
mod gpu;
mod gpu_memory;
mod help;
mod icons;
mod panels;
mod prefs;
mod ribbon;
mod settings;
mod state;
mod theme;
mod tools;
#[cfg(test)]
mod ui_tests;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Opal Studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Opal Studio", options, Box::new(move |cc| Ok(Box::new(app::OpalApp::new(cc, files)))))
}
