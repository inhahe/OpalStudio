//! The Image tab's commands: rotating, flipping and resizing the whole image, and changing
//! the canvas.
//!
//! Whole-image changes carry every object through one transform. Pixel objects undergo
//! quarter turns and flips exactly; any other change makes them image objects (so they keep
//! their pixels), unless resampling is asked for (see `design.md` › Document model).

use crate::memory::Allowance;
use crate::state::{AppState, MessageKind};
use opal_core::document::{Canvas, Document, ImageObject, Layer, LayerContent, ObjectKind, PixelObject};
use opal_core::geom::{Affine, FRect, GridMap, IRect};
use opal_core::raster::RasterImage;
use opal_core::resample::{self, Sampling};
use std::sync::Arc;

/// A quarter turn or flip of the whole image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Clockwise,
    Counterclockwise,
    Half,
    FlipHorizontal,
    FlipVertical,
}

/// The Resize Image dialog's fields.
#[derive(Clone, Debug)]
pub struct ResizeImage {
    pub width: u32,
    pub height: u32,
    pub keep_aspect: bool,
    /// Pixels per inch.
    pub resolution: f64,
    /// Whether changing the resolution in the dialog changes the pixel size with it, so that
    /// the print size stays.
    pub keep_print_size: bool,
}

/// The Resize Canvas dialog's fields: either a new size, with where the old canvas sits in
/// it (each of `anchor` 0, 1 or 2: left, center, right; top, middle, bottom), or `margins`
/// to add on each side (left, top, right, bottom; negative ones cut).
#[derive(Clone, Debug)]
pub struct ResizeCanvas {
    pub width: u32,
    pub height: u32,
    pub anchor: [u8; 2],
    pub by_margins: bool,
    pub margins: [i64; 4],
}

impl ResizeCanvas {
    /// The canvas the fields describe, for the current canvas `c`.
    pub fn canvas(&self, c: &Canvas) -> Canvas {
        if self.by_margins {
            let [l, t, r, b] = self.margins;
            let w = (i64::from(c.width) + l + r).clamp(1, i64::from(MAX_SIDE));
            let h = (i64::from(c.height) + t + b).clamp(1, i64::from(MAX_SIDE));
            Canvas { x: c.x - l, y: c.y - t, width: w as u32, height: h as u32 }
        } else {
            let place = |old: u32, new: u32, a: u8| (i64::from(old) - i64::from(new)) * i64::from(a) / 2;
            Canvas {
                x: c.x + place(c.width, self.width, self.anchor[0]),
                y: c.y + place(c.height, self.height, self.anchor[1]),
                width: self.width,
                height: self.height,
            }
        }
    }
}

/// The Rotate by Angle dialog's fields.
#[derive(Clone, Debug)]
pub struct RotateBy {
    pub degrees: f64,
    pub grow: bool,
}

impl Turn {
    /// The map carrying a `w × h` canvas at `(x, y)` onto its turned self (whose corner
    /// stays at `(x, y)`).
    fn map(self, c: &Canvas) -> Affine {
        let (x, y, w, h) = (c.x as f64, c.y as f64, f64::from(c.width), f64::from(c.height));
        let m = |a: f64, b: f64, cc: f64, d: f64, e: f64, f: f64| Affine { a, b, c: cc, d, e, f };
        match self {
            // (u, v) -> (h - v, u) within the canvas.
            Turn::Clockwise => m(0.0, 1.0, -1.0, 0.0, x + y + h, y - x),
            // (u, v) -> (v, w - u).
            Turn::Counterclockwise => m(0.0, -1.0, 1.0, 0.0, x - y, w + x + y),
            Turn::Half => m(-1.0, 0.0, 0.0, -1.0, 2.0 * x + w, 2.0 * y + h),
            Turn::FlipHorizontal => m(-1.0, 0.0, 0.0, 1.0, 2.0 * x + w, 0.0),
            Turn::FlipVertical => m(1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * y + h),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Turn::Clockwise => "Rotate right",
            Turn::Counterclockwise => "Rotate left",
            Turn::Half => "Rotate 180°",
            Turn::FlipHorizontal => "Flip horizontally",
            Turn::FlipVertical => "Flip vertically",
        }
    }
}

/// Carries every object of `doc`, and its selection, through `m`. Pixel objects stay pixels
/// where `m` is exact on the grid; otherwise they are resampled with `resample` if given, else
/// become image objects drawn with `sampling`. Resampling keeps only the part within `keep` of
/// what would reach too far (see `layer_ops::drawable`); returns whether it had to, or why
/// nothing could be done: the pixels drawn would need more memory than is free.
pub fn transform_all(
    doc: &mut Document,
    m: &Affine,
    resample: Option<Sampling>,
    sampling: Sampling,
    keep: &IRect,
) -> Result<bool, String> {
    let mut to = Drawing::new(*keep, Allowance::now());
    fn walk(
        layer: &mut Layer,
        m: &Affine,
        resample_with: Option<Sampling>,
        sampling: Sampling,
        to: &mut Drawing,
    ) -> Result<(), String> {
        match &mut layer.content {
            LayerContent::Objects(objects) => {
                for o in objects {
                    o.kind = match &o.kind {
                        ObjectKind::Image(i) => {
                            let t = i.transform.then(*m);
                            if ImageObject::transform_ok(&t) {
                                ObjectKind::Image(ImageObject { transform: t, ..i.clone() })
                            } else {
                                // Stretched past what an image object may be (see
                                // transform_ok), as repeated resizing could do.
                                drawn(&i.image, &t, &i.sampling, to)?
                            }
                        }
                        ObjectKind::Pixels(p) => {
                            let placed = Affine::translate(p.offset[0] as f64, p.offset[1] as f64).then(*m);
                            match (placed.grid_map(), resample_with) {
                                (Some(g), _) => on_grid(&p.image, &placed, &g, &sampling, to)?,
                                (None, Some(s)) => drawn(&p.image, &placed, &s, to)?,
                                (None, None) if !ImageObject::transform_ok(&placed) => {
                                    drawn(&p.image, &placed, &sampling, to)?
                                }
                                (None, None) => match p.image.content_bounds() {
                                    Some(frame) => ObjectKind::Image(ImageObject {
                                        image: p.image.clone(),
                                        frame,
                                        transform: placed,
                                        sampling,
                                    }),
                                    None => ObjectKind::Pixels(p.clone()),
                                },
                            }
                        }
                    };
                }
            }
            LayerContent::Group(children) => {
                for c in children {
                    walk(Arc::make_mut(c), m, resample_with, sampling, to)?;
                }
            }
        }
        Ok(())
    }
    for l in &mut doc.layers {
        walk(Arc::make_mut(l), m, resample, sampling, &mut to)?;
    }
    // What is selected goes with what it was selected on.
    if let Some(s) = doc.selection.take() {
        doc.selection = Some(Arc::new(s.transformed(m))).filter(|s| !s.is_empty());
    }
    Ok(to.clipped)
}

/// What drawing into pixels may take, and what it has done.
pub(crate) struct Drawing {
    /// The part to keep of what reaches too far (see `layer_ops::drawable`).
    keep: IRect,
    /// Whether part of something drawn was left out.
    pub clipped: bool,
    /// Kept while drawing: it caps the tiles' memory.
    memory: Allowance,
}

impl Drawing {
    /// Drawing that keeps the part within `keep` of what reaches too far, within `memory`.
    pub(crate) fn new(keep: IRect, memory: Allowance) -> Drawing {
        Drawing { keep, clipped: false, memory }
    }

    /// What the drawing may still take, for more drawn as part of the same work.
    pub(crate) fn memory(&mut self) -> &mut Allowance {
        &mut self.memory
    }
}

/// `image`, placed by `placed`, which maps the grid onto itself (`g`), as a pixel object:
/// the same pixels moved, or turned or flipped, copied exactly (see [`drawn`]).
pub(crate) fn on_grid(
    image: &Arc<RasterImage>,
    placed: &Affine,
    g: &GridMap,
    sampling: &Sampling,
    to: &mut Drawing,
) -> Result<ObjectKind, String> {
    if (g.a, g.b, g.c, g.d) == (1, 0, 0, 1) {
        Ok(ObjectKind::Pixels(PixelObject { image: image.clone(), offset: [g.e, g.f] }))
    } else {
        drawn(image, placed, sampling, to)
    }
}

/// `image` drawn through `t` into a pixel object, keeping only the part within `to.keep` of
/// what reaches too far, or why it cannot be: more memory than is left.
fn drawn(image: &RasterImage, t: &Affine, sampling: &Sampling, to: &mut Drawing) -> Result<ObjectKind, String> {
    let drawn = match crate::layer_ops::drawable(image, t, sampling, &to.keep, &mut to.memory)? {
        Some((area, cut)) => {
            to.clipped |= cut;
            resample::render_image(image, t, sampling, Some(&area))
        }
        None => RasterImage::new(),
    };
    Ok(ObjectKind::Pixels(PixelObject { image: Arc::new(drawn), offset: [0, 0] }))
}

/// The bounds of what the visible layers hold, in whole pixels.
pub fn visible_bounds(doc: &Document) -> Option<IRect> {
    let mut out: Option<FRect> = None;
    for l in &doc.layers {
        if !l.visible {
            continue;
        }
        // Visible objects inside visible groups only.
        fn walk(l: &Layer, out: &mut Option<FRect>) {
            if !l.visible || l.opacity <= 0.0 {
                return;
            }
            match &l.content {
                LayerContent::Objects(objects) => {
                    for o in objects {
                        if let Some(b) = o.bounds() {
                            *out = Some(match *out {
                                None => b,
                                Some(a) => FRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)),
                            });
                        }
                    }
                }
                LayerContent::Group(c) => c.iter().for_each(|c| walk(c, out)),
            }
        }
        walk(l, &mut out);
    }
    out.map(|b| IRect::covering(&b)).filter(|r| !r.is_empty())
}

/// The canvas size limit for the commands here, as for New Image.
const MAX_SIDE: u32 = crate::dialogs::MAX_SIDE;

impl AppState {
    pub(crate) fn turn(&mut self, turn: Turn) {
        let sampling = crate::prefs::sampling(&self.prefs, "transform");
        let d = self.active_mut().expect("available");
        let result = d.apply(turn.label(), |doc, _| {
            let c = doc.canvas;
            let m = turn.map(&c);
            transform_all(doc, &m, None, sampling, &c.rect())?;
            if matches!(turn, Turn::Clockwise | Turn::Counterclockwise) {
                doc.canvas = Canvas { x: c.x, y: c.y, width: c.height, height: c.width };
            }
            Ok::<(), String>(())
        });
        self.report(result);
    }

    /// Scales the image to `width × height`, about the canvas's corner, and gives it the
    /// resolution `spec` has. A change of resolution alone changes no pixels.
    pub(crate) fn resize_image(&mut self, spec: &ResizeImage, resample: bool) {
        assert!(opal_core::units::valid_ppi(spec.resolution), "resolution {} out of range", spec.resolution);
        let sampling = crate::prefs::sampling(&self.prefs, "resize");
        let d = self.active_mut().expect("available");
        let (c, ppi) = (d.doc().canvas, d.doc().resolution);
        if (spec.width, spec.height) == (c.width, c.height) {
            if spec.resolution != ppi {
                let result = d.apply("Change resolution", |doc, _| {
                    doc.resolution = spec.resolution;
                    Ok::<(), String>(())
                });
                self.report(result);
            }
            return;
        }
        if let Some(name) = crate::commands::locked_layer(d.doc()) {
            return self.message(
                MessageKind::Info,
                "Resize image",
                format!(
                    "\"{name}\" is locked, and a new size changes every layer. Unlock it first. (A new \
                     resolution alone can be given while it is locked.)"
                ),
            );
        }
        let mut clipped = false;
        let result = d.apply("Resize image", |doc, _| {
            doc.resolution = spec.resolution;
            let c = doc.canvas;
            let (x, y) = (c.x as f64, c.y as f64);
            let sx = f64::from(spec.width) / f64::from(c.width);
            let sy = f64::from(spec.height) / f64::from(c.height);
            let m = Affine::scale(sx, sy).about(x, y);
            let canvas = Canvas { width: spec.width, height: spec.height, ..c };
            clipped = transform_all(doc, &m, resample.then_some(sampling), sampling, &canvas.rect())?;
            doc.canvas = canvas;
            Ok::<(), String>(())
        });
        self.report(result);
        if clipped {
            self.notify(crate::layer_ops::CLIPPED);
        }
    }

    /// Gives the canvas the size and place `spec` describes.
    pub(crate) fn resize_canvas(&mut self, spec: &ResizeCanvas) {
        let d = self.active_mut().expect("available");
        let c = d.doc().canvas;
        let canvas = spec.canvas(&c);
        if canvas == c {
            return;
        }
        let result = d.apply("Resize canvas", |doc, _| {
            doc.canvas = canvas;
            Ok::<(), String>(())
        });
        self.report(result);
    }

    /// Shrinks (or grows) the canvas to what the visible layers hold.
    pub(crate) fn crop_to_content(&mut self) {
        let d = self.active().expect("available");
        let Some(b) = visible_bounds(d.doc()) else {
            return self.message(MessageKind::Info, "Crop to Content", "The visible layers are empty.");
        };
        self.crop_canvas_to(&b, "Crop to content", "The canvas already fits the content.");
    }

    /// Makes the active image's canvas the rectangle `b` (each side at most [`MAX_SIDE`]), as a
    /// history state labeled `label`; if it is that already, says `same` instead.
    pub(crate) fn crop_canvas_to(&mut self, b: &IRect, label: &str, same: &str) {
        let d = self.active_mut().expect("available");
        let (w, h) = (b.width().min(i64::from(MAX_SIDE)) as u32, b.height().min(i64::from(MAX_SIDE)) as u32);
        let canvas = Canvas { x: b.x0, y: b.y0, width: w, height: h };
        if canvas == d.doc().canvas {
            return self.notify(same);
        }
        let result = d.apply(label, |doc, _| {
            doc.canvas = canvas;
            Ok::<(), String>(())
        });
        self.report(result);
    }

    /// Turns the image by an angle about the canvas's center, growing the canvas to hold
    /// the turned canvas if `grow`.
    pub(crate) fn rotate_by(&mut self, spec: &RotateBy) {
        let sampling = crate::prefs::sampling(&self.prefs, "transform");
        let d = self.active_mut().expect("available");
        let result = d.apply("Rotate", |doc, _| {
            let c = doc.canvas;
            let r = c.rect().to_f();
            let [cx, cy] = r.center();
            let m = Affine::rotate(spec.degrees.to_radians()).about(cx, cy);
            transform_all(doc, &m, None, sampling, &c.rect())?;
            if spec.grow {
                let b = m.bounds(&r);
                let (w, h) =
                    ((b.x1 - b.x0).ceil().min(f64::from(MAX_SIDE)), (b.y1 - b.y0).ceil().min(f64::from(MAX_SIDE)));
                doc.canvas = Canvas {
                    x: (cx - w * 0.5).round() as i64,
                    y: (cy - h * 0.5).round() as i64,
                    width: w as u32,
                    height: h as u32,
                };
            }
            Ok::<(), String>(())
        });
        self.report(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::document::{IdGen, Position};
    use opal_core::raster::RasterImage;
    use opal_core::tile::pixel_from_f32;

    /// A 3×2 image at (10, 20) with a distinct pixel at each corner.
    fn corners() -> Document {
        let mut ids = IdGen::default();
        let mut img = RasterImage::new();
        for (x, y, v) in [(10, 20, 0.1), (12, 20, 0.2), (12, 21, 0.3), (10, 21, 0.4)] {
            img.set_pixel(x, y, pixel_from_f32([v, 0.0, 0.0, 1.0]));
        }
        let mut d = Document::new(3, 2);
        d.canvas.x = 10;
        d.canvas.y = 20;
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img)).unwrap();
        d
    }

    fn pixel_at(d: &Document, x: i64, y: i64) -> f32 {
        let ObjectKind::Pixels(p) = &d.layers[0].objects()[0].kind else { panic!("still pixels") };
        p.image.pixel(x - p.offset[0], y - p.offset[1])[0].to_f32()
    }

    #[test]
    fn quarter_turns_and_flips_are_exact_and_keep_the_corner() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // Clockwise, the bottom-left corner comes to the top-left.
        let mut d = corners();
        let c = d.canvas;
        transform_all(&mut d, &Turn::Clockwise.map(&c), None, Sampling::default(), &IRect::from_size(100, 100))
            .unwrap();
        assert!(
            close(pixel_at(&d, 10, 20), 0.4) && close(pixel_at(&d, 11, 20), 0.1) && close(pixel_at(&d, 11, 22), 0.2)
        );
        let mut d = corners();
        transform_all(&mut d, &Turn::Counterclockwise.map(&c), None, Sampling::default(), &IRect::from_size(100, 100))
            .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.2) && close(pixel_at(&d, 10, 22), 0.1));
        let mut d = corners();
        transform_all(&mut d, &Turn::Half.map(&c), None, Sampling::default(), &IRect::from_size(100, 100)).unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.3) && close(pixel_at(&d, 12, 21), 0.1));
        let mut d = corners();
        transform_all(&mut d, &Turn::FlipHorizontal.map(&c), None, Sampling::default(), &IRect::from_size(100, 100))
            .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.2) && close(pixel_at(&d, 12, 21), 0.4));
        let mut d = corners();
        transform_all(&mut d, &Turn::FlipVertical.map(&c), None, Sampling::default(), &IRect::from_size(100, 100))
            .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.4) && close(pixel_at(&d, 12, 21), 0.2));
    }

    #[test]
    fn resizing_keeps_pixels_as_an_image_unless_resampling() {
        let mut d = corners();
        let m = Affine::scale(2.0, 2.0).about(10.0, 20.0);
        transform_all(&mut d, &m, None, Sampling::default(), &IRect::from_size(100, 100)).unwrap();
        assert!(matches!(d.layers[0].objects()[0].kind, ObjectKind::Image(_)), "kept as an image object");
        let mut d = corners();
        transform_all(&mut d, &m, Some(Sampling::default()), Sampling::default(), &IRect::from_size(100, 100)).unwrap();
        assert!(matches!(d.layers[0].objects()[0].kind, ObjectKind::Pixels(_)), "resampled into pixels");
    }

    #[test]
    fn resampling_into_more_memory_than_is_free_is_refused() {
        use crate::state::{Background, NewImage};
        let mut st = AppState::new();
        st.create_image(&NewImage {
            width: 300,
            height: 200,
            background: Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        let before = st.active().unwrap().history.current();
        // 300,000 × 200,000 pixels resampled would take nearly 500 GB: more than the 64 GB
        // pretended free (whatever this computer has, and whether its disk counts, which it
        // does once any test in this process has given the tiles a budget).
        let _free = crate::memory::testing::pretend_free(Some(64 << 30));
        let spec = ResizeImage {
            width: 300_000,
            height: 200_000,
            keep_aspect: true,
            resolution: opal_core::units::DEFAULT_PPI,
            keep_print_size: false,
        };
        st.resize_image(&spec, true);
        assert_eq!(st.active().unwrap().history.current(), before, "resized all the same");
        let said = st.messages.back().map(|m| m.body.as_str()).unwrap_or_default();
        assert!(said.contains("memory"), "{said:?}");
        // Drawn at the new size, the pixels kept: nothing to make.
        st.resize_image(&spec, false);
        assert_eq!(st.active().unwrap().doc().canvas.width, 300_000);
    }

    #[test]
    fn a_new_resolution_alone_changes_no_pixels() {
        use crate::state::{Background, NewImage};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 30, height: 20, background: Background::White, resolution: 72.0 });
        let before = st.active().unwrap().doc().layers[0].clone();
        let spec = ResizeImage { width: 30, height: 20, keep_aspect: true, resolution: 300.0, keep_print_size: false };
        st.resize_image(&spec, false);
        let d = st.active().unwrap();
        assert_eq!(d.doc().resolution, 300.0);
        assert!(Arc::ptr_eq(&d.doc().layers[0], &before), "the layer was redrawn");
        assert_eq!(d.history.undo_label(), Some("Change resolution"));
        // With a layer locked, the resolution can still change; the size cannot.
        let bg = st.active().unwrap().doc().layers[0].id;
        st.active_mut()
            .unwrap()
            .apply("Lock", |doc, _| {
                doc.layer_mut(bg).unwrap().locked = true;
                Ok::<(), String>(())
            })
            .unwrap();
        assert!(crate::commands::availability(&st, crate::commands::Cmd::ResizeImage).is_ok());
        st.resize_image(&ResizeImage { resolution: 200.0, ..spec.clone() }, false);
        assert_eq!(st.active().unwrap().doc().resolution, 200.0);
        st.resize_image(&ResizeImage { width: 60, height: 40, ..spec.clone() }, false);
        assert!(st.messages.back().is_some_and(|m| m.body.contains("is locked")), "resized a locked layer");
        assert_eq!(st.active().unwrap().doc().canvas.width, 30);
        st.messages.clear();
        st.active_mut()
            .unwrap()
            .apply("Unlock", |doc, _| {
                doc.layer_mut(bg).unwrap().locked = false;
                Ok::<(), String>(())
            })
            .unwrap();
        // Resized, it takes the resolution too.
        let spec = ResizeImage { width: 60, height: 40, resolution: 150.0, ..spec };
        st.resize_image(&spec, false);
        let d = st.active().unwrap().doc();
        assert_eq!((d.canvas.width, d.resolution), (60, 150.0));
    }

    #[test]
    fn what_is_selected_is_turned_flipped_and_resized_with_the_image() {
        use crate::state::{Background, NewImage};
        use opal_core::mask::{Combine, Extent, Shape, ShapeKind};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let shape =
            Shape { kind: ShapeKind::Rectangle, center: [60.0, 45.0], size: [100.0, 50.0], angle: 0.0, smooth: true };
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
        let selected = |st: &AppState| st.active().unwrap().pixel_selection().unwrap().mask().extent();
        // (10, 20)-(110, 70), turned right on a canvas 200 high: (u, v) goes to (200 - v, u).
        st.turn(Turn::Clockwise);
        assert_eq!(selected(&st), Extent::Within(IRect::new(130, 10, 180, 110)));
        // Flipped across the canvas, now 200 wide.
        st.turn(Turn::FlipHorizontal);
        assert_eq!(selected(&st), Extent::Within(IRect::new(20, 10, 70, 110)));
        // Halved, about the canvas's corner.
        let spec = ResizeImage { width: 100, height: 150, keep_aspect: true, resolution: 72.0, keep_print_size: false };
        st.resize_image(&spec, false);
        assert_eq!(selected(&st), Extent::Within(IRect::new(10, 5, 35, 55)));
        // Still the rectangle it was, on the grid.
        let steps = st.active().unwrap().pixel_selection().unwrap().steps().to_vec();
        assert!(
            matches!(steps[..], [Step::Shape { shape: Shape { angle: 0.0, size: [25.0, 50.0], .. }, .. }]),
            "{steps:?}"
        );
        // Undone, back where it was.
        for _ in 0..3 {
            st.active_mut().unwrap().undo();
        }
        assert_eq!(selected(&st), Extent::Within(IRect::new(10, 20, 110, 70)));
    }

    #[test]
    fn visible_bounds_are_the_content() {
        let d = corners();
        assert_eq!(visible_bounds(&d), Some(IRect::new(10, 20, 13, 22)));
    }
}
