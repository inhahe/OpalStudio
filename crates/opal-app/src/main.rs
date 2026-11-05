#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release builds

//! Opal Studio, a 2D graphics editor. See `design.md` for the architecture.

mod app;
mod background;
mod canvas;
mod clipboard;
mod commands;
mod config;
mod crop;
mod curve_editor;
mod dialogs;
mod doc;
mod gallery;
mod gpu;
mod gpu_memory;
mod help;
mod icons;
mod image_ops;
mod layer_ops;
mod move_tool;
mod panels;
mod paste;
mod paste_keys;
mod prefs;
mod projector;
mod ribbon;
mod settings;
mod state;
mod theme;
mod thumbs;
mod tools;
mod transform;
mod transform_ui;
#[cfg(test)]
mod ui_tests;
mod widgets;
mod writer;

use eframe::egui;

// Tiles are 512 KiB, just past the size from which Windows' heap hands each allocation
// back to the system when freed, so with it every new tile costs fresh pages (and the page
// faults serialize the threads compositing in parallel). mimalloc keeps memory for reuse.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> eframe::Result {
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Opal Studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        // See paste_keys.rs: Ctrl+V with only files on the clipboard.
        #[cfg(windows)]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            builder.with_msg_hook(paste_keys::hook);
        })),
        ..Default::default()
    };
    eframe::run_native("Opal Studio", options, Box::new(move |cc| Ok(Box::new(app::OpalApp::new(cc, files)))))
}
