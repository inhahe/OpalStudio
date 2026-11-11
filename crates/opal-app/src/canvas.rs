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
    self, Color32, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke,
    Vec2,
};
use eframe::egui_wgpu;
use opal_core::composite::MAX_LEVEL;
use opal_core::document::LayerId;
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
    // The rulers, if shown, take a strip along the top and the left; the view the rest.
    let avail = ui.available_rect_before_wrap();
    let rulers = st.prefs.bool("canvas.rulers");
    let inset = if rulers { crate::rulers::THICK } else { 0.0 };
    // Align the viewport with device pixels, so 100% zoom maps pixels one to one.
    let min_px = ((avail.min.to_vec2() + Vec2::splat(inset)) * ppp).round();
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
    let cache_budget = crate::prefs::bytes(&st.prefs, "canvas.cache");
    // Tile textures may take half the GPU memory the system allows the process (or 1 GB
    // where it does not say).
    let texture_budget = (st.gpu.memory.map_or(1 << 30, |(_, budget)| budget / 2) / crate::gpu::TEXTURE_BYTES) as usize;
    let zs = st.tool_settings(ToolId::Zoom).clone();
    let ms = st.tool_settings(ToolId::Move).clone();
    let select = crate::select_tool::Options::of(st.tool_settings(ToolId::Select));
    let threshold = opal_core::mask::level((st.prefs.f64("selection.threshold") / 100.0) as f32);
    let overlay = st.prefs.bool("selection.overlay").then(|| (st.prefs.f64("selection.overlay_opacity") / 100.0) as f32);
    let snapping = st.prefs.bool("transform.snapping");
    let snap_points = st.prefs.f64("transform.snap_distance") as f32;
    let palette = st.palette;
    let unit = st.unit();
    let space = ctx.input(|i| i.key_down(Key::Space)) && !ctx.egui_wants_keyboard_input();
    let chosen = st.tool();
    let tool = if space { ToolId::Hand } else { chosen };
    let mut notice = None;
    let Some(d) = st.doc_mut(id) else { return };
    // While the transform box is open, it takes the pointer (Space and the middle button
    // still scroll the view, and the box stays, waiting).
    let boxed = d.transform.is_some();
    let box_active = boxed && !space;
    let canvas = crate::doc::canvas_rect(&d.doc().canvas);
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

    // With the transform box open, the primary button belongs to it (Space still pans).
    let panning = response.dragged_by(PointerButton::Middle)
        || (tool == ToolId::Hand && !box_active && response.dragged_by(PointerButton::Primary));
    if panning {
        d.view.offset += response.drag_delta() * ppp;
    }
    let modifiers = ctx.input(|i| i.modifiers);
    let to_doc = |p: Pos2, v: &crate::doc::View| {
        let c = v.to_canvas(to_px(p));
        [f64::from(c.x), f64::from(c.y)]
    };
    let pick_opacity = (ms.f64("move.pick_opacity") / 100.0) as f32;
    // Ctrl+click with any other tool selects the layer under the pointer, as the Move tool's
    // click does.
    if !boxed
        && tool != ToolId::Move
        && modifiers.command
        && response.clicked()
        && let Some(p) = response.interact_pointer_pos()
        && let Some(l) = crate::move_tool::pick(d, to_doc(p, &d.view), pick_opacity, modifiers.alt)
    {
        d.select_only(l);
    }
    if tool == ToolId::Move && !boxed {
        let snap = if snapping { f64::from(snap_points * ppp / d.view.zoom) } else { -1.0 };
        let pick = ms.bool("move.pick");
        if response.clicked()
            && pick
            && let Some(p) = response.interact_pointer_pos()
            && let Some(l) = crate::move_tool::pick(d, to_doc(p, &d.view), pick_opacity, modifiers.alt)
        {
            if modifiers.shift { d.toggle_selected(l) } else { d.select_only(l) }
        }
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        {
            let from = to_doc(origin, &d.view);
            if pick
                && !modifiers.shift
                && let Some(l) = crate::move_tool::pick(d, from, pick_opacity, modifiers.alt)
                && !d.is_selected(l)
            {
                d.select_only(l);
            }
            match crate::move_tool::locked_in_selected_layers(d) {
                Some(name) => notice = Some(format!("\"{name}\" is locked, so it cannot be moved. Unlock it first.")),
                None => {
                    let bounds = d.selected_layers_bounds();
                    d.moving =
                        Some(crate::move_tool::MoveDrag { from, bounds, guides: Default::default(), shown: None });
                }
            }
        }
        if let Some(drag) = d.moving.clone()
            && response.dragged_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            let (delta, guides) =
                d.move_delta(&drag, to_doc(p, &d.view), modifiers.shift, if modifiers.alt { -1.0 } else { snap });
            // Only a change of place redraws (holding still costs nothing).
            if drag.shown != Some(delta) {
                d.move_selected_layers_to(delta);
            }
            if let Some(m) = d.moving.as_mut() {
                m.guides = guides;
                m.shown = Some(delta);
            }
        }
    }
    // A Move drag ends with the button, whatever the pointer is doing by then: Space may
    // have made it the hand, or a key another tool.
    if d.moving.is_some() && !ctx.input(|i| i.pointer.primary_down()) {
        d.moving = None;
        if d.pending_label() == Some("Move") {
            d.commit_pending();
        }
    }
    if tool == ToolId::Select && !boxed {
        let v = d.view;
        let to_screen = |p: [f64; 2]| rect.min + v.to_screen(Vec2::new(p[0] as f32, p[1] as f32)) / ppp;
        crate::select_tool::canvas(&ctx, d, &response, &|p| to_doc(p, &v), &to_screen, select);
    } else {
        // The tool changed (or Space made it the hand) mid-drag: the drag is given up.
        if d.select_drag.take().is_some() {
            d.revision += 1;
        }
        if let Some(s) = d.shape_drag.take() {
            d.discard_pending(crate::select_tool::ShapeDrag::label(s.drag.part));
        }
    }
    if tool == ToolId::Zoom && !boxed {
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
        ToolId::Move => CursorIcon::Move,
        ToolId::Select | ToolId::Crop => CursorIcon::Crosshair,
    };
    // The boxes set their own cursors, by the handle under the pointer.
    // The crop box belongs to the chosen tool, so it stays while Space scrolls the view.
    let cropping = chosen == ToolId::Crop && !boxed;
    if (response.hovered() || response.dragged()) && !(box_active || (cropping && !space)) {
        ctx.set_cursor_icon(cursor);
    }
    if !cropping && d.crop.take().is_some() {
        // The crop box goes when the tool does; it was only a proposal.
        d.crop_drag = None;
        d.revision += 1;
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
    // While something is dragged, what it changes is drawn at the level shown, so the drag
    // keeps up however large the image. The frame the button is let go is drawn as a view
    // at rest is, whether or not the code that ends the drag has run yet.
    let held = ctx.input(|i| i.pointer.primary_down());
    let dragging = held && (d.moving.is_some() || d.transform.as_ref().is_some_and(|s| s.drag.is_some()));
    let quality = if dragging { opal_core::composite::Quality::Preview } else { opal_core::composite::Quality::Exact };
    d.projector.ask(&layers, level, &wider, quality, Some(cache_budget), &ctx);
    // What was asked for, or, if it is not done in time, the last view done (at the level it
    // was done at); nothing at all until the first is.
    let wait = d.projector.wait;
    let shown = d.projector.shown(wait);
    let shown_level = shown.as_ref().map_or(level, |s| s.level);
    let mut draws = Vec::new();
    for (&coord, _) in shown.iter().flat_map(|s| &s.tiles) {
        let r = coord.rect(shown_level);
        if r.intersect(&visible).is_empty() {
            continue;
        }
        let tiles = &shown.as_ref().expect("a tile came from it").tiles;
        let mut sources: [Option<Arc<Tile>>; 9] = Default::default();
        for dy in -1..=1 {
            for dx in -1..=1 {
                let n = TileCoord::new(coord.x + dx, coord.y + dy);
                sources[((dy + 1) * 3 + dx + 1) as usize] = tiles.get(&n).cloned();
            }
        }
        let p0 = view.to_screen(Vec2::new(r.x0 as f32, r.y0 as f32));
        let p1 = view.to_screen(Vec2::new(r.x1 as f32, r.y1 as f32));
        draws.push(TileDraw { coord, rect: [p0.x, p0.y, p1.x, p1.y], sources });
    }
    let doc_key = d.id.0;

    let nearest = view.zoom >= prefs.sharp_from;
    let uniforms = ViewUniforms {
        vp_size: [vp.x, vp.y],
        canvas_origin: view.to_screen(canvas.min.to_vec2()).into(),
        canvas_size: (canvas.size() * view.zoom).into(),
        checker_px: (prefs.checker_size * ppp).max(1.0),
        off_canvas: prefs.off_canvas,
        checker_a: theme::linear(prefs.checker_a),
        checker_b: theme::linear(prefs.checker_b),
        backdrop: theme::linear(palette.backdrop),
        encode_srgb: 0,
        smooth: u32::from(!nearest),
        _pad: [0; 2],
    };
    let callback = CanvasCallback {
        view_key: doc_key,
        level: shown_level,
        uniforms,
        tiles: draws,
        nearest,
        texture_budget,
        frame: ctx.cumulative_frame_nr(),
    };
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
    // A view taking a while (a huge image) says so, rather than look finished.
    const BEHIND: std::time::Duration = std::time::Duration::from_millis(150);
    if let Some(t) = d.projector.behind_for() {
        if t >= BEHIND {
            let at = Rect::from_min_size(rect.right_top() + Vec2::new(-112.0, 8.0), Vec2::new(104.0, 22.0));
            // Painted, and one widget that only senses hovering (read out as a label), so it
            // never takes a press from the canvas under it.
            const SAID: &str = "Drawing…";
            let sign = ui.interact(at, ui.scope_id().with("drawing"), egui::Sense::hover());
            sign.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, SAID));
            let painter = ui.painter();
            painter.rect_filled(at.expand(2.0), 4.0, palette.backdrop.gamma_multiply(0.85));
            let spinner = Rect::from_center_size(at.left_center() + Vec2::new(11.0, 0.0), Vec2::splat(14.0));
            egui::Spinner::new().size(14.0).paint_at(ui, spinner);
            let font = egui::TextStyle::Body.resolve(ui.style());
            painter.text(
                at.left_center() + Vec2::new(24.0, 0.0),
                egui::Align2::LEFT_CENTER,
                SAID,
                font,
                ui.visuals().text_color(),
            );
        } else {
            ctx.request_repaint_after(BEHIND - t);
        }
    }
    if prefs.border {
        let min = rect.min + view.to_screen(canvas.min.to_vec2()) / ppp;
        let r = Rect::from_min_size(min, canvas.size() * view.zoom / ppp);
        ui.painter().with_clip_rect(rect).rect_stroke(
            r,
            0.0,
            Stroke::new(1.0, palette.separator),
            egui::StrokeKind::Outside,
        );
    }
    if rulers {
        let corner = Rect::from_min_max(avail.min, rect.min);
        let top = Rect::from_min_max(Pos2::new(rect.left(), avail.top()), rect.right_top());
        let left = Rect::from_min_max(Pos2::new(avail.left(), rect.top()), rect.left_bottom());
        let ruling = crate::rulers::Ruling {
            origin: rect.min + view.to_screen(canvas.min.to_vec2()) / ppp,
            per_point: f64::from(ppp / view.zoom),
            unit,
            ppi: d.doc().resolution,
            pointer: response.hover_pos(),
        };
        crate::rulers::draw(ui, corner, top, left, &ruling, &palette);
    }
    let to_screen = |p: [f64; 2]| rect.min + view.to_screen(Vec2::new(p[0] as f32, p[1] as f32)) / ppp;
    if !boxed && !cropping {
        draw_outlines(ui, d, rect, &to_screen, palette.accent);
    }
    // What is not selected, under red, if asked for.
    if let Some(opacity) = overlay
        && let Some(s) = d.pixel_selection().cloned()
    {
        let (view, canvas) = (d.view, d.doc().canvas.rect());
        crate::mask_view::draw(ui, s.mask(), &canvas, rect, view, ppp, opacity, &mut d.overlay);
    }
    // What is selected: its outline, the marching ants. They march while they are seen and
    // the window has the focus: each step is a frame of the whole window.
    if let Some(lines) = d.selection_outline(threshold) {
        march(&ctx, draw_ants(ui, &lines, rect, &to_screen, ctx.input(|i| i.time)));
    }
    if let Some(drag) = &d.select_drag {
        crate::select_tool::draw(ui, drag, select, rect, &to_screen);
    }
    if let Some(m) = &d.moving {
        draw_guides(ui, &m.guides, rect, &to_screen);
    }
    // Where the context menu was opened, for listing the layers there.
    let menu_at_id = response.id.with("menu-at");
    if response.secondary_clicked()
        && let Some(p) = response.interact_pointer_pos()
    {
        let at = to_doc(p, &view);
        ctx.data_mut(|m| m.insert_temp(menu_at_id, at));
    }
    let menu_at: Option<[f64; 2]> = ctx.data(|m| m.get_temp(menu_at_id));
    let here: Vec<(LayerId, String)> = match menu_at {
        Some([x, y]) => opal_core::composite::layers_at(&d.doc().layers, x.floor() as i64, y.floor() as i64)
            .into_iter()
            .filter_map(|(l, _)| Some((l, d.doc().layer(l)?.name.clone())))
            .collect(),
        None => Vec::new(),
    };
    // The app clears this at the start of each frame; the canvas under the pointer sets it.
    if cursor_canvas.is_some() {
        st.cursor_canvas = cursor_canvas;
    }
    if boxed {
        crate::transform_ui::canvas_box(ui, st, id, &response, rect, !space);
    } else if cropping {
        crate::crop::canvas_crop(ui, st, id, &response, rect, !space);
    } else if tool == ToolId::Select {
        crate::select_tool::draw_box(ui, st, id, &response, rect, &to_screen);
    }
    if let Some(n) = notice {
        st.notify(n);
    }
    response.context_menu(|ui| {
        if !here.is_empty() {
            ui.label(RichText::new("Layers here").small().color(palette.weak_text));
            for (l, name) in &here {
                if ui.button(format!("{}  {name}", crate::icons::LAYER)).on_hover_text("Select this layer").clicked() {
                    if let Some(d) = st.doc_mut(id) {
                        d.select_only(*l);
                    }
                    ui.close();
                }
            }
            ui.separator();
        }
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

/// How often the marching ants move on, in milliseconds.
pub(crate) const ANTS_STEP_MS: u64 = 100;

/// Asks for the frame in which marching ants drawn this frame take their next step, if
/// they were `seen` and the window has the focus: each step is a frame of the whole window.
pub(crate) fn march(ctx: &egui::Context, seen: bool) {
    if seen && ctx.input(|i| i.focused) {
        ctx.request_repaint_after(std::time::Duration::from_millis(ANTS_STEP_MS));
    }
}

/// The selection's outline (`lines`, closed, in document coordinates), as marching ants:
/// black dashes on white, moving along it as `time` (seconds) goes on. Each line is drawn
/// through the middles of the screen's pixels, so that it stays one sharp pixel wide; those
/// wholly outside `rect` are not drawn. Whether any of it is within `rect`.
pub(crate) fn draw_ants(
    ui: &egui::Ui,
    lines: &[Vec<[f64; 2]>],
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
    time: f64,
) -> bool {
    const DASH: f32 = 4.0;
    let painter = ui.painter().with_clip_rect(rect);
    let ppp = ui.ctx().pixels_per_point();
    let sharp = |p: Pos2| Pos2::new(((p.x * ppp).floor() + 0.5) / ppp, ((p.y * ppp).floor() + 0.5) / ppp);
    // A dash and a gap a step.
    let steps = (time * 1000.0 / ANTS_STEP_MS as f64) as u64;
    let offset = (steps % (2 * DASH as u64)) as f32;
    let mut seen = false;
    for line in lines {
        let mut pts: Vec<Pos2> = line.iter().map(|&p| sharp(to_screen(p))).collect();
        if pts.len() < 2 {
            continue;
        }
        pts.push(pts[0]);
        // Its pieces run along the pixels' edges, across or down: each one's rectangle is it.
        if !pts.windows(2).any(|w| Rect::from_two_pos(w[0], w[1]).intersects(rect)) {
            continue;
        }
        seen = true;
        painter.add(Shape::line(pts.clone(), Stroke::new(1.0, Color32::WHITE)));
        painter.extend(Shape::dashed_line_with_offset(
            &pts,
            Stroke::new(1.0, Color32::BLACK),
            &[DASH],
            &[DASH],
            offset,
        ));
    }
    seen
}

/// A faint outline around what each selected layer holds (stronger for the active layer).
fn draw_outlines(
    ui: &egui::Ui,
    d: &crate::doc::DocState,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
    accent: Color32,
) {
    let painter = ui.painter().with_clip_rect(rect);
    let doc = d.doc();
    for l in d.selected_layers() {
        let Some(layer) = doc.layer(l) else { continue };
        if !layer.visible {
            continue;
        }
        let strength = if Some(l) == d.active_layer() { 0.7 } else { 0.4 };
        let stroke = Stroke::new(1.0, accent.gamma_multiply(strength));
        for o in layer.all_objects() {
            if let Some(corners) = o.outline() {
                let pts: Vec<Pos2> = corners.iter().map(|&p| to_screen(p)).collect();
                painter.add(Shape::dashed_line(&[pts[0], pts[1], pts[2], pts[3], pts[0]], stroke, 4.0, 3.0));
            }
        }
    }
}

/// The snap lines a drag is using, across the whole view.
pub fn draw_guides(ui: &egui::Ui, guides: &crate::transform::Guides, rect: Rect, to_screen: &dyn Fn([f64; 2]) -> Pos2) {
    let painter = ui.painter().with_clip_rect(rect);
    let stroke = Stroke::new(1.0, Color32::from_rgb(230, 60, 200));
    for &x in &guides.xs {
        let sx = to_screen([x, 0.0]).x;
        painter.line_segment([Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())], stroke);
    }
    for &y in &guides.ys {
        let sy = to_screen([0.0, y]).y;
        painter.line_segment([Pos2::new(rect.left(), sy), Pos2::new(rect.right(), sy)], stroke);
    }
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
