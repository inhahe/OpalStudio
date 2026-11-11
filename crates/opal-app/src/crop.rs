//! The Crop tool: a box over the canvas that becomes the new canvas, turned to straighten
//! the image if the box was rotated.
//!
//! Cropping only moves the canvas (see `design.md` › Document model), so nothing is lost:
//! enlarging the canvas again brings back what was cropped away, unless the tool's *Delete
//! cropped pixels* is set.

use crate::doc::{DocId, DocState};
use crate::edit_box::{BoxDrag, EditBox, OnScreen, Part};
use crate::icons;
use crate::image_ops::transform_all;
use crate::settings::Settings;
use crate::state::AppState;
use crate::theme::icon;
use eframe::egui::{self, Color32, Pos2, Rect, Response, Shape, Stroke, Vec2};
use opal_core::document::{Canvas, LayerContent, ObjectKind, PixelObject};
use opal_core::geom::{Affine, IRect};
use std::sync::Arc;

/// The Crop tool's own of the box's (see `edit_box`).
impl EditBox {
    /// The box covering `c`.
    pub fn of(c: &Canvas) -> EditBox {
        let r = c.rect().to_f();
        EditBox { center: r.center(), size: [r.width(), r.height()], angle: 0.0 }
    }

    /// The canvas the box makes: its size, rounded, around its center.
    /// (Sides past the largest canvas allowed are cut to it, still around the center.)
    pub fn canvas(&self) -> Canvas {
        let side = f64::from(crate::dialogs::MAX_SIDE);
        let w = self.size[0].round().clamp(1.0, side);
        let h = self.size[1].round().clamp(1.0, side);
        Canvas {
            x: (self.center[0] - w * 0.5).round() as i64,
            y: (self.center[1] - h * 0.5).round() as i64,
            width: w as u32,
            height: h as u32,
        }
    }
}

/// The width ÷ height the Crop tool's settings ask for, if any (`original` is the canvas's).
pub fn aspect(s: &Settings, original: f64) -> Option<f64> {
    let r = match s.choice("crop.aspect").as_str() {
        "free" => return None,
        "original" => original,
        "1:1" => 1.0,
        "4:3" => 4.0 / 3.0,
        "3:2" => 1.5,
        "16:9" => 16.0 / 9.0,
        "5:4" => 1.25,
        _ => s.f64("crop.custom_w") / s.f64("crop.custom_h").max(1e-9),
    };
    // Portrait means taller than wide, whichever way the ratio was given.
    Some(if s.bool("crop.portrait") { r.min(1.0 / r) } else { r.max(1.0 / r) })
}

/// Pixel object `p` with everything outside `canvas` (document pixels) removed.
fn cropped(p: &PixelObject, canvas: &IRect) -> PixelObject {
    let local = canvas.translate(-p.offset[0], -p.offset[1]);
    let mut out = opal_core::raster::RasterImage::new();
    for (c, t) in p.image.tiles() {
        let r = c.rect(0);
        let keep = r.intersect(&local);
        if keep.is_empty() {
            continue;
        }
        if keep == r {
            out.set_tile(c, Some(t.clone()));
            continue;
        }
        let within = [keep.x0 - r.x0, keep.y0 - r.y0, keep.x1 - r.x0, keep.y1 - r.y0].map(|v| v as u32);
        let tile = t.kept_within(within);
        // Only tiles with content are kept.
        if !tile.is_transparent() {
            out.set_tile(c, Some(Arc::new(tile)));
        }
    }
    PixelObject { image: Arc::new(out), offset: p.offset }
}

impl DocState {
    /// Crops to `b`: the canvas becomes the box, and if it was turned, everything turns
    /// the other way about its center, so the box's contents come out upright.
    ///
    /// Moving the canvas changes no layer, but straightening and deleting cropped pixels
    /// change every one, so they are refused (with the reason) while a layer is locked.
    pub fn crop_to(
        &mut self,
        b: &EditBox,
        delete: bool,
        sampling: opal_core::resample::Sampling,
    ) -> Result<(), String> {
        let canvas = b.canvas();
        let turned = b.angle.abs() > 1e-9;
        if turned || delete {
            let mut locked = None;
            self.doc().for_each_layer(&mut |l, _| {
                if l.locked && locked.is_none() {
                    locked = Some(l.name.clone());
                }
            });
            if let Some(name) = locked {
                let what = if turned { "Straightening" } else { "Deleting cropped pixels" };
                return Err(format!("{what} changes every layer, and \"{name}\" is locked. Unlock it first."));
            }
        }
        let label = if turned { "Crop and straighten" } else { "Crop" };
        let mut clipped = false;
        self.apply(label, |doc, _| {
            if turned {
                // Deleting what is cropped away is destructive anyway, so the turned pixels are
                // resampled into pixels (which the deleting below then crops) rather than kept as
                // images.
                let m = Affine::rotate(-b.angle).about(b.center[0], b.center[1]);
                clipped = transform_all(doc, &m, delete.then_some(sampling), sampling, &canvas.rect())?;
            }
            doc.canvas = canvas;
            if delete {
                let rect = canvas.rect();
                fn walk(l: &mut opal_core::document::Layer, rect: &IRect) {
                    match &mut l.content {
                        LayerContent::Objects(objects) => {
                            for o in objects {
                                if let ObjectKind::Pixels(p) = &o.kind {
                                    o.kind = ObjectKind::Pixels(cropped(p, rect));
                                }
                            }
                        }
                        LayerContent::Group(c) => c.iter_mut().for_each(|c| walk(Arc::make_mut(c), rect)),
                    }
                }
                for l in &mut doc.layers {
                    walk(Arc::make_mut(l), &rect);
                }
            }
            Ok::<(), String>(())
        })?;
        if clipped {
            self.notice = Some(crate::layer_ops::CLIPPED.into());
        }
        Ok(())
    }
}

/// The part of convex polygon `poly` on the same side of the line through `a` and `b` as
/// `toward` (`same`), or on the other side.
fn clip(poly: Vec<Pos2>, a: Pos2, b: Pos2, toward: Pos2, same: bool) -> Vec<Pos2> {
    let side = |p: Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
    let s0 = side(toward).signum();
    let keep = |p: Pos2| if same { side(p) * s0 >= 0.0 } else { side(p) * s0 <= 0.0 };
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let (kp, kq) = (keep(p), keep(q));
        if kp {
            out.push(p);
        }
        if kp != kq {
            let (sp, sq) = (side(p), side(q));
            let t = sp / (sp - sq);
            out.push(p + (q - p) * t);
        }
    }
    out
}

/// The Crop tool on document `id`'s canvas: the box, its handles, the shade outside it, the
/// guides, and the Crop and Reset buttons. The box takes drags when `interactive` (not
/// while Space scrolls the view).
pub fn canvas_crop(
    ui: &mut egui::Ui,
    st: &mut AppState,
    id: DocId,
    response: &Response,
    rect: Rect,
    interactive: bool,
) {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let settings = st.tool_settings(crate::tools::ToolId::Crop).clone();
    let palette = st.palette;
    let sampling = crate::prefs::sampling(&st.prefs, "transform");
    let mut numbers = st.box_numbers;
    let Some(d) = st.doc_mut(id) else { return };
    let c = d.doc().canvas;
    let b = *d.crop.get_or_insert_with(|| EditBox::of(&c));
    let view = d.view;
    let to_screen = |p: [f64; 2]| rect.min + view.to_screen(Vec2::new(p[0] as f32, p[1] as f32)) / ppp;
    let to_doc = |p: Pos2| {
        let q = view.to_canvas((p - rect.min) * ppp);
        [f64::from(q.x), f64::from(q.y)]
    };
    let aspect = aspect(&settings, f64::from(c.width) / f64::from(c.height));
    let screen = OnScreen { corners: b.corners().map(to_screen) };

    if interactive
        && response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        d.crop_drag = Some(BoxDrag { part: screen.part_at(origin), from: to_doc(origin), start: b });
    }
    if let Some(drag) = d.crop_drag
        && interactive
        && response.dragged_by(egui::PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
    {
        let keep = if ctx.input(|i| i.modifiers.shift) && aspect.is_none() {
            Some(drag.start.size[0] / drag.start.size[1])
        } else {
            aspect
        };
        let moved = EditBox::dragged(&drag, to_doc(p), keep);
        if d.crop != Some(moved) {
            d.crop = Some(moved);
            d.revision += 1;
        }
    }
    if response.drag_stopped() {
        d.crop_drag = None;
    }
    // A double-click inside the box crops (outside it, it does nothing).
    let apply = interactive
        && response.double_clicked()
        && response.interact_pointer_pos().is_some_and(|p| screen.part_at(p) == Part::Move);

    // ---- Drawing ----------------------------------------------------------------------
    let b = d.crop.expect("set above");
    let corners = b.corners().map(to_screen);
    let screen = OnScreen { corners };
    let painter = ui.painter().with_clip_rect(rect);
    // The shade: what lies outside the box, as convex pieces that do not overlap (outside
    // the first edge; inside it but outside the second; and so on).
    let shade = Color32::from_black_alpha((settings.f64("crop.shade") / 100.0 * 255.0) as u8);
    let view = vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    let mid = corners[0].lerp(corners[2], 0.5);
    for k in 0..4 {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let mut piece = clip(view.clone(), a, b, mid, false);
        for j in 0..k {
            piece = clip(piece, corners[j], corners[(j + 1) % 4], mid, true);
        }
        if piece.len() >= 3 {
            painter.add(Shape::convex_polygon(piece, shade, Stroke::NONE));
        }
    }
    let line = Stroke::new(1.0, Color32::WHITE);
    painter.add(Shape::closed_line(corners.to_vec(), Stroke::new(3.0, Color32::from_black_alpha(100))));
    painter.add(Shape::closed_line(corners.to_vec(), line));
    // Guides, light over dark so they show on any image.
    let under = Stroke::new(2.0, Color32::from_black_alpha(70));
    let thin = Stroke::new(1.0, Color32::from_white_alpha(150));
    let n = match settings.choice("crop.guides").as_str() {
        "thirds" => 3,
        "grid" => 8,
        _ => 0,
    };
    for i in 1..n {
        let t = i as f32 / n as f32;
        for stroke in [under, thin] {
            painter.line_segment([corners[0].lerp(corners[1], t), corners[3].lerp(corners[2], t)], stroke);
            painter.line_segment([corners[0].lerp(corners[3], t), corners[1].lerp(corners[2], t)], stroke);
        }
    }
    crate::edit_box::draw_handles(&painter, &screen, palette.accent);
    if let Some(p) = response.hover_pos()
        && interactive
    {
        let part = d.crop_drag.map_or_else(|| screen.part_at(p), |g| g.part);
        ctx.set_cursor_icon(crate::edit_box::cursor(part));
    }
    if d.crop_drag.is_some()
        && let Some(p) = response.interact_pointer_pos()
    {
        let canvas = b.canvas();
        let text = format!("{} × {} px   {:.1}°", canvas.width, canvas.height, b.angle.to_degrees());
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
        let at = p + Vec2::new(16.0, 18.0);
        painter.rect_filled(Rect::from_min_size(at, galley.size()).expand(4.0), 3.0, Color32::from_black_alpha(190));
        painter.galley(at, galley, Color32::WHITE);
    }

    // ---- Buttons ----------------------------------------------------------------------
    let lowest = corners.iter().fold(f32::NEG_INFINITY, |m, p| m.max(p.y));
    let cx = corners.iter().map(|p| p.x).sum::<f32>() / 4.0;
    // As wide as it was drawn last (with its numbers button or not).
    let bar_id = egui::Id::unique(("crop-toolbar", id.0));
    let width = ctx.memory(|m| m.area_rect(bar_id)).map_or(160.0, |r| r.width());
    let x = (cx - width * 0.5).clamp(rect.left() + 4.0, (rect.right() - width - 4.0).max(rect.left() + 4.0));
    let y = (lowest + 12.0).clamp(rect.top() + 4.0, rect.bottom() - 40.0);
    let mut choice = None;
    let bar = egui::Area::new(bar_id).order(egui::Order::Foreground).fixed_pos(Pos2::new(x, y)).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{}  Crop", icons::CHECK))
                    .on_hover_text("Crop to the box (Enter, or double-click inside it)")
                    .clicked()
                {
                    choice = Some(true);
                }
                if ui
                    .button(icon(icons::RESET, 15.0))
                    .on_hover_text("Put the box back around the whole canvas (Esc)")
                    .clicked()
                {
                    choice = Some(false);
                }
                crate::box_fields::toggle(ui, &mut numbers);
            });
        });
    });
    let mut refused = None;
    if apply || choice == Some(true) {
        let delete = settings.bool("crop.delete");
        let b = d.crop.expect("set above");
        if b.canvas() != c || b.angle.abs() > 1e-9 || delete {
            match d.crop_to(&b, delete, sampling) {
                Ok(()) => d.crop = None,
                Err(why) => refused = Some(why),
            }
        }
    } else if choice == Some(false) {
        d.crop = None;
        d.revision += 1;
    }
    if let Some(why) = refused {
        st.message(crate::state::MessageKind::Info, "Crop", why);
    }
    st.box_numbers = numbers;
    if numbers && st.doc(id).is_some_and(|d| d.crop.is_some()) {
        let panel = egui::Id::unique(("crop-numbers", id.0));
        // Beside the box and its toolbar, covering neither.
        let around = [corners.as_slice(), &[bar.response.rect.min, bar.response.rect.max]].concat();
        crate::box_fields::panel_by(&ctx, panel, rect, &around, |ui| {
            crate::box_fields::crop(ui, st, id, "crop-by-box");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_box_is_drawn_from_the_press_and_rotation_turns_about_the_center() {
        let b = EditBox { center: [50.0, 50.0], size: [100.0, 100.0], angle: 0.0 };
        let drag = BoxDrag { part: Part::New, from: [200.0, 10.0], start: b };
        let n = EditBox::dragged(&drag, [150.0, 40.0], None);
        assert_eq!(n.canvas(), Canvas { x: 150, y: 10, width: 50, height: 30 });
        let drag = BoxDrag { part: Part::Rotate, from: [150.0, 50.0], start: b };
        let n = EditBox::dragged(&drag, [50.0, 150.0], None);
        assert!((n.angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert_eq!(n.center, b.center);
    }

    fn state_with_image() -> DocState {
        let mut ids = opal_core::document::IdGen::default();
        let mut img = opal_core::raster::RasterImage::new();
        for y in 0..100 {
            for x in 0..100 {
                img.set_pixel(x, y, opal_core::tile::pixel_from_f32([1.0; 4]));
            }
        }
        let mut d = opal_core::document::Document::new(100, 100);
        let l = opal_core::document::Layer::new_raster(ids.layer(), ids.object(), "l", img);
        d.insert(opal_core::document::Position { parent: None, index: 0 }, l).unwrap();
        DocState::new(crate::doc::DocId(1), "t".into(), d, ids, "New")
    }

    #[test]
    fn straightening_or_deleting_is_refused_while_a_layer_is_locked() {
        let mut s = state_with_image();
        let id = s.doc().layers[0].id;
        let _ = s.apply::<String>("Lock", |d, _| {
            d.layer_mut(id).unwrap().locked = true;
            Ok(())
        });
        let turned = EditBox { center: [50.0, 50.0], size: [60.0, 60.0], angle: 0.1 };
        let sampling = opal_core::resample::Sampling::default();
        assert!(s.crop_to(&turned, false, sampling).unwrap_err().contains("locked"));
        let plain = EditBox { angle: 0.0, ..turned };
        assert!(s.crop_to(&plain, true, sampling).is_err(), "deleting changes the locked layer");
        assert!(s.crop_to(&plain, false, sampling).is_ok(), "moving the canvas changes no layer");
    }

    #[test]
    fn deleting_cropped_pixels_works_when_straightening_too() {
        let mut s = state_with_image();
        let turned = EditBox { center: [50.0, 50.0], size: [40.0, 40.0], angle: 0.2 };
        s.crop_to(&turned, true, opal_core::resample::Sampling::default()).unwrap();
        let o = &s.doc().layers[0].objects()[0];
        let ObjectKind::Pixels(p) = &o.kind else { panic!("pixels, cropped") };
        let b = p.image.content_bounds().unwrap().translate(p.offset[0], p.offset[1]);
        assert_eq!(b.intersect(&s.doc().canvas.rect()), b, "nothing is left outside the canvas");
    }

    #[test]
    fn a_box_larger_than_the_largest_canvas_stays_centered() {
        let b = EditBox { center: [500.0, 500.0], size: [1e9, 100.0], angle: 0.0 };
        let c = b.canvas();
        assert_eq!(c.width, crate::dialogs::MAX_SIDE);
        assert_eq!(c.x + i64::from(c.width) / 2, 500, "{c:?}");
    }

    #[test]
    fn cropping_keeps_eight_bit_pixels_eight_bit() {
        let rgba: Vec<[u8; 4]> = (0..400 * 300).map(|i| [(i % 400) as u8, 50, 90, 255]).collect();
        let img = opal_core::raster::RasterImage::from_srgb8(400, 300, &rgba);
        let p = PixelObject { image: Arc::new(img.clone()), offset: [0, 0] };
        let keep = IRect::new(10, 20, 330, 290);
        let out = cropped(&p, &keep);
        assert!(out.image.tiles().all(|(_, t)| t.is_srgb8()), "made working pixels");
        assert!(out.image.read_rect(&keep) == img.read_rect(&keep));
        assert!(out.image.pixel(5, 25)[3].to_f32() == 0.0, "kept what was cropped away");
    }

    #[test]
    fn cropping_keeps_no_empty_tiles() {
        let mut img = opal_core::raster::RasterImage::new();
        img.set_pixel(0, 0, opal_core::tile::pixel_from_f32([1.0; 4]));
        let p = PixelObject { image: Arc::new(img), offset: [0, 0] };
        // The canvas takes part of the tile, but none of its content.
        let out = cropped(&p, &IRect::new(10, 10, 300, 300));
        assert!(out.image.is_empty());
    }

    #[test]
    fn deleting_cropped_pixels_keeps_only_the_canvas() {
        let mut img = opal_core::raster::RasterImage::new();
        for x in 0..600 {
            img.set_pixel(x, 0, opal_core::tile::pixel_from_f32([1.0; 4]));
        }
        let p = PixelObject { image: Arc::new(img), offset: [10, 0] };
        let out = cropped(&p, &IRect::new(100, 0, 400, 1));
        assert_eq!(out.image.content_bounds(), Some(IRect::new(90, 0, 390, 1)));
    }
}
