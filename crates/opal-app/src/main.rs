#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release builds

//! Opal Studio, a 2D graphics editor. See `design.md` for the architecture.

mod api;
mod app;
mod assistant;
mod background;
mod box_fields;
mod canvas;
mod clipboard;
mod commands;
mod config;
mod console;
mod crop;
mod curve_editor;
mod dialogs;
mod doc;
mod edit_box;
mod edits;
mod gallery;
mod gpu;
mod gpu_memory;
mod headless;
mod help;
mod icons;
mod image_ops;
mod layer_ops;
mod lengths;
mod mask_view;
mod mcp;
mod mcp_http;
mod memory;
mod move_tool;
mod number_field;
mod panels;
mod paste;
mod paste_keys;
mod prefs;
mod projector;
mod recent;
mod ribbon;
mod rulers;
mod select;
mod select_tool;
mod selection_panel;
mod server;
mod settings;
mod shared_memory;
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
    // An MCP server for an assistant's client, which launches it: no window.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--mcp") {
        std::process::exit(mcp::run_stdio());
    }
    // A script run against an editor with no window, for batch work.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--headless") {
        std::process::exit(headless::run(std::env::args_os().skip(2).collect()));
    }
    // An editor with no window for a script that started it (through the `opal` package).
    if std::env::args_os().nth(1).is_some_and(|a| a == "--serve") {
        std::process::exit(headless::serve(std::env::args_os().skip(2).collect()));
    }
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    // For scripts to start this program with no window when it is not running.
    config::note_program();
    // Opened from Explorer with an editor already running: they open in that one.
    if !files.is_empty()
        && let Some(file) = server::connection_file()
        && server::hand_to_running(&file, &files)
    {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Opal Studio")
            // Moved, or maximized, where it does not fit (app.rs, fit_to_monitor).
            .with_inner_size(app::START_SIZE)
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        // In the middle of its monitor, rather than cascaded down from its corner.
        centered: true,
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
