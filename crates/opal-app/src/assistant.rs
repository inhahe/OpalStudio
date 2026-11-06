//! Connect an Assistant: how to set up an AI assistant's client to drive the editor
//! through MCP, by the stdio shim (`opal-studio --mcp`) or over HTTP (see design.md,
//! *Scripting*), each with what to copy into the client's settings.

use crate::state::AppState;
use eframe::egui::{self, RichText};

/// The window, while it is open.
pub fn window(ctx: &egui::Context, st: &mut AppState) {
    if !st.assistant_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Connect an Assistant")
        .open(&mut open)
        .default_pos(ctx.content_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .show(ctx, |ui| contents(ui, st));
    st.assistant_open = open;
}

/// `text` to copy, in a box of its own with a button that copies it.
fn copyable(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        if ui.small_button(format!("{}  Copy", crate::icons::COPY)).clicked() {
            ui.ctx().copy_text(text.to_owned());
        }
        ui.add(egui::Label::new(RichText::new(text).monospace()).wrap_mode(egui::TextWrapMode::Wrap).selectable(true));
    });
}

fn contents(ui: &mut egui::Ui, st: &mut AppState) {
    let weak = st.palette.weak_text;
    ui.label(
        "An AI assistant (Claude Code, Claude Desktop, Cursor and others) drives Opal Studio through MCP: its \
         tools are the commands and queries scripts call, and image lets it see what it is doing. Opal Studio \
         must be running for a tool to act.",
    );
    ui.add_space(8.0);
    ui.label(RichText::new("Clients that start their servers").strong());
    let exe = std::env::current_exe().map_or_else(|_| "opal-studio.exe".to_owned(), |p| p.display().to_string());
    ui.label(RichText::new("Claude Code:").color(weak));
    copyable(ui, &format!("claude mcp add opal-studio -- \"{exe}\" --mcp"));
    ui.label(RichText::new("Others, in their MCP settings:").color(weak));
    let config = serde_json::json!({ "mcpServers": { "opal-studio": { "command": exe, "args": ["--mcp"] } } });
    copyable(ui, &config.to_string());
    ui.add_space(8.0);
    ui.label(RichText::new("Clients that connect by URL").strong());
    let mut serve = st.prefs.bool("scripting.mcp_http");
    let port = st.prefs.f64("scripting.mcp_port") as u16;
    if ui
        .checkbox(&mut serve, format!("Serve MCP over HTTP, on port {port}"))
        .on_hover_text("Only programs on this computer that have the token can connect. The port is set in Preferences › Scripting.")
        .changed()
    {
        st.prefs.set("scripting.mcp_http", crate::settings::Value::Bool(serve));
        st.settings_touched();
    }
    match (&st.mcp_http, serve) {
        (Some((port, token)), true) => {
            let url = format!("http://127.0.0.1:{port}/mcp");
            ui.label(RichText::new("The address, and the header to send with every request:").color(weak));
            copyable(ui, &url);
            copyable(ui, &format!("Authorization: Bearer {token}"));
            ui.label(RichText::new("Claude Code:").color(weak));
            copyable(
                ui,
                &format!(
                    "claude mcp add --transport http opal-studio {url} --header \"Authorization: Bearer {token}\""
                ),
            );
            if ui
                .button(format!("{}  New token", crate::icons::KEY))
                .on_hover_text("Clients given this one are refused from now on: give them the new one.")
                .clicked()
            {
                st.renew_mcp_token = true;
            }
        }
        (None, true) if st.scripts_file.is_none() => {
            let why = st.scripts_unserved.as_deref().unwrap_or(crate::console::NOT_SERVING);
            ui.label(RichText::new(why).color(weak));
        }
        (None, true) => match st.mcp_http_trouble.clone() {
            Some(why) => {
                ui.label(RichText::new(why).color(weak));
                if ui.button("Try again").on_hover_text("Once whatever held the port has let it go.").clicked() {
                    st.retry_mcp_http = true;
                }
            }
            None => {
                ui.label(RichText::new("Starting…").color(weak));
            }
        },
        (_, false) => {}
    }
}
