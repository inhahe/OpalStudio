//! The canvas: one document's view, its navigation, and handing tiles to the GPU.

use crate::commands::Cmd;
use crate::doc::DocId;
use crate::gpu::{CanvasCallback, TileDraw, ViewUniforms};
use crate::settings::Settings;
use crate::state::AppState;
use crate::theme;
use crate::tools::{self, ToolId};
use crate::widgets::menu_command;
use eframe::egui::{
    self, Color32, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Stroke, Vec2,
};
use eframe::egui_wgpu;
use opal_core::composite::MAX_LEVEL;
use opal_core::geom::IRect;
use opal_core::tile::{TILE_SIZE, Tile, TileCoord};
use std::sync::Arc;

/// Device pixels a wheel "line" scrolls when the wheel scrolls the view.
const LINE_PX: f32 = 48.0;

/// Where a zoom drag started: the anchor (device pixels) and the zoom then.
#[derive(Clone, Copy)]
struct ZoomDrag {
    anchor: Vec2,
    zoom: f32,
}

/// The preferences the canvas uses, read once per frame.
struct CanvasPrefs {
    fit_max: f32,
    wheel_zooms: bool,
    wheel_step: f32,
    sharper: bool,
    sharp_from: f32,
    off_canvas: f32,
    checker_size: f32,
    checker_a: Color32,
    checker_b: Color32,
    border: bool,
}

impl CanvasPrefs {
    fn read(p: &Settings) -> CanvasPrefs {
        CanvasPrefs {
            fit_max: (p.f64("nav.fit_max") / 100.0) as f32,
            wheel_zooms: p.choice("nav.wheel") == "zoom",
            wheel_step: (p.f64("nav.wheel_step") / 100.0) as f32,
            sharper: p.choice("canvas.zoomed_out") == "sharper",
            sharp_from: (p.f64("canvas.sharp_from") / 100.0) as f32,
            off_canvas: if p.bool("canvas.off_canvas_shown") {
                (p.f64("canvas.off_canvas") / 100.0) as f32
            } else {
                0.0
            },
            checker_size: p.f64("canvas.checker_size") as f32,
            checker_a: p.color("canvas.checker_a"),
            checker_b: p.color("canvas.checker_b"),
            border: p.bool("canvas.border"),
        }
    }
}

/// The pyramid level to show at `zoom`: 0 at 100% and above; below, the level whose pixels
/// are just smaller than a screen pixel (`sharper`) or just larger.
pub fn level_for(zoom: f32, sharper: bool) -> u32 {
    if zoom >= 1.0 {
        return 0;
    }
    let l = (1.0 / zoom).log2();
    let l = if sharper { (l + 1e-4).floor() } else { (l - 1e-4).ceil() };
    (l.max(0.0) as u32).min(MAX_LEVEL)
}

/// Shows document `id`'s canvas in all the space `ui` has left.
pub fn canvas_ui(ui: &mut egui::Ui, st: &mut AppState, id: DocId) {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    // Align the viewport with device pixels, so 100% zoom maps pixels one to one.
    let avail = ui.available_rect_before_wrap();
    let min_px = (avail.min.to_vec2() * ppp).round();
    let max_px = (avail.max.to_vec2() * ppp).round();
    let rect = Rect::from_min_max((min_px / ppp).to_pos2(), (max_px / ppp).to_pos2());
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let vp = max_px - min_px;
    if vp.x < 1.0 || vp.y < 1.0 {
        return;
    }
    if response.clicked() || response.drag_started() || response.secondary_clicked() {
        st.active_doc = Some(id);
    }

    let prefs = CanvasPrefs::read(&st.prefs);
    let zs = st.tool_settings(ToolId::Zoom).clone();
    let palette = st.palette;
    let space = ctx.input(|i| i.key_down(Key::Space)) && !ctx.egui_wants_keyboard_input();
    let tool = if space { ToolId::Hand } else { st.tool };
    let Some(d) = st.doc_mut(id) else { return };
    let canvas = Vec2::new(d.doc().canvas.width as f32, d.doc().canvas.height as f32);
    d.view.viewport = vp;
    d.view.screen_rect = rect;
    if !d.view.placed {
        d.view.fit(canvas, vp, prefs.fit_max);
    }

    // ---- Navigation -----------------------------------------------------------------
    let to_px = |p: Pos2| (p - rect.min) * ppp;
    let pointer_px = response.hover_pos().map(to_px);
    if response.hovered() {
        let anchor = pointer_px.unwrap_or(vp * 0.5);
        for e in ctx.input(|i| i.events.clone()) {
            match e {
                Event::MouseWheel { unit, delta, source, modifiers, .. } => {
                    let lines = match unit {
                        MouseWheelUnit::Line => delta,
                        MouseWheelUnit::Point => delta / 40.0,
                        MouseWheelUnit::Page => delta * 10.0,
                    };
                    let from_wheel = matches!(source, egui::MouseWheelSource::Wheel)
                        || (matches!(source, egui::MouseWheelSource::Unknown) && unit == MouseWheelUnit::Line);
                    let zoom = modifiers.command || (from_wheel && prefs.wheel_zooms && !modifiers.shift);
                    if zoom {
                        let z = d.view.zoom * (1.0 + prefs.wheel_step).powf(lines.y);
                        d.view.zoom_about(z, anchor);
                    } else {
                        let px = match unit {
                            MouseWheelUnit::Point => delta * ppp,
                            _ => lines * LINE_PX * ppp,
                        };
                        // Shift turns vertical wheel motion into horizontal scrolling.
                        let px = if modifiers.shift && px.x == 0.0 { Vec2::new(px.y, 0.0) } else { px };
                        d.view.offset += px;
                    }
                }
                Event::Zoom(factor) => {
                    let z = d.view.zoom * factor;
                    d.view.zoom_about(z, anchor);
                }
                _ => {}
            }
        }
    }

    let panning = response.dragged_by(PointerButton::Middle)
        || (tool == ToolId::Hand && response.dragged_by(PointerButton::Primary));
    if panning {
        d.view.offset += response.drag_delta() * ppp;
    }
    if tool == ToolId::Zoom {
        let alt = ctx.input(|i| i.modifiers.alt);
        if response.clicked()
            && let Some(p) = pointer_px
        {
            let z = if zs.choice("zoom.steps") == "presets" {
                tools::next_preset(d.view.zoom, !alt)
            } else {
                let f = zs.f64("zoom.factor") as f32;
                if alt { d.view.zoom / f } else { d.view.zoom * f }
            };
            d.view.zoom_about(z, p);
        }
        let drag_id = response.id.with("zoom-drag");
        if zs.bool("zoom.drag") && response.dragged_by(PointerButton::Primary) {
            if response.drag_started()
                && let Some(p) = response.interact_pointer_pos()
            {
                let start = ZoomDrag { anchor: to_px(p - response.drag_delta()), zoom: d.view.zoom };
                ctx.data_mut(|m| m.insert_temp(drag_id, start));
            }
            if let Some(start) = ctx.data(|m| m.get_temp::<ZoomDrag>(drag_id)) {
                let total = response.total_drag_delta().unwrap_or_default().x;
                let speed = (zs.f64("zoom.drag_speed") / 100.0) as f32;
                let z = start.zoom * 2f32.powf(total / 100.0 * speed);
                d.view.zoom_about(z, start.anchor);
            }
        }
        if response.drag_stopped() {
            ctx.data_mut(|m| m.remove::<ZoomDrag>(drag_id));
        }
    }
    let cursor = match tool {
        _ if response.dragged_by(PointerButton::Middle) => CursorIcon::Grabbing,
        ToolId::Hand if response.dragged() => CursorIcon::Grabbing,
        ToolId::Hand => CursorIcon::Grab,
        ToolId::Zoom if ctx.input(|i| i.modifiers.alt) => CursorIcon::ZoomOut,
        ToolId::Zoom => CursorIcon::ZoomIn,
    };
    if response.hovered() || response.dragged() {
        ctx.set_cursor_icon(cursor);
    }
    let cursor_canvas = pointer_px.map(|p| {
        let c = d.view.to_canvas(p);
        (c.x.floor() as i64, c.y.floor() as i64)
    });

    // ---- Drawing --------------------------------------------------------------------
    let view = d.view;
    let level = level_for(view.zoom, prefs.sharper);
    let c0 = view.to_canvas(Vec2::ZERO);
    let c1 = view.to_canvas(vp);
    let visible = IRect::new(c0.x.floor() as i64, c0.y.floor() as i64, c1.x.ceil() as i64 + 1, c1.y.ceil() as i64 + 1);
    // One tile more on every side, so the borders of the visible tiles are up to date.
    let span = i64::from(TILE_SIZE) << level;
    let wider = IRect::new(visible.x0 - span, visible.y0 - span, visible.x1 + span, visible.y1 + span);
    let layers = d.doc().layers.clone();
    let tiles = d.projection.update(&layers, level, &wider);
    let mut draws = Vec::new();
    for (coord, _) in tiles {
        let r = coord.rect(level);
        if r.intersect(&visible).is_empty() {
            continue;
        }
        let mut sources: [Option<Arc<Tile>>; 9] = Default::default();
        for dy in -1..=1 {
            for dx in -1..=1 {
                let n = TileCoord::new(coord.x + dx, coord.y + dy);
                sources[((dy + 1) * 3 + dx + 1) as usize] = d.projection.tile(level, n).cloned();
            }
        }
        let p0 = view.to_screen(Vec2::new(r.x0 as f32, r.y0 as f32));
        let p1 = view.to_screen(Vec2::new(r.x1 as f32, r.y1 as f32));
        draws.push(TileDraw { coord, rect: [p0.x, p0.y, p1.x, p1.y], sources });
    }
    let doc_key = d.id.0;
    // The app clears this at the start of each frame; the canvas under the pointer sets it.
    if cursor_canvas.is_some() {
        st.cursor_canvas = cursor_canvas;
    }

    let nearest = view.zoom >= prefs.sharp_from;
    let uniforms = ViewUniforms {
        vp_size: [vp.x, vp.y],
        canvas_origin: [view.offset.x, view.offset.y],
        canvas_size: [canvas.x * view.zoom, canvas.y * view.zoom],
        checker_px: (prefs.checker_size * ppp).max(1.0),
        off_canvas: prefs.off_canvas,
        checker_a: theme::linear(prefs.checker_a),
        checker_b: theme::linear(prefs.checker_b),
        backdrop: theme::linear(palette.backdrop),
        encode_srgb: 0,
        smooth: u32::from(!nearest),
        _pad: [0; 2],
    };
    let callback = CanvasCallback { view_key: doc_key, level, uniforms, tiles: draws, nearest };
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
    if prefs.border {
        let min = rect.min + view.offset / ppp;
        let r = Rect::from_min_size(min, canvas * view.zoom / ppp);
        ui.painter().with_clip_rect(rect).rect_stroke(
            r,
            0.0,
            Stroke::new(1.0, palette.separator),
            egui::StrokeKind::Outside,
        );
    }
    response.context_menu(|ui| {
        for &cmd in CANVAS_MENU {
            match cmd {
                None => {
                    ui.separator();
                }
                Some(cmd) => menu_command(ui, st, cmd),
            }
        }
    });
}

/// The canvas's context menu; `None` is a separator.
const CANVAS_MENU: &[Option<Cmd>] = &[
    Some(Cmd::Undo),
    Some(Cmd::Redo),
    None,
    Some(Cmd::ZoomIn),
    Some(Cmd::ZoomOut),
    Some(Cmd::ZoomFit),
    Some(Cmd::Zoom100),
    None,
    Some(Cmd::NewLayer),
    Some(Cmd::ToggleOffCanvas),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_zoom() {
        assert_eq!(level_for(2.0, true), 0);
        assert_eq!(level_for(1.0, true), 0);
        assert_eq!(level_for(0.75, true), 0);
        assert_eq!(level_for(0.75, false), 1);
        assert_eq!(level_for(0.5, true), 1);
        assert_eq!(level_for(0.5, false), 1, "exactly half needs no further reduction");
        assert_eq!(level_for(0.3, true), 1);
        assert_eq!(level_for(0.3, false), 2);
        assert_eq!(level_for(1e-9, true), MAX_LEVEL);
    }
}
