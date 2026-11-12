//! The Image tab's commands: rotating, flipping and resizing the whole image, and changing
//! the canvas.
//!
//! Whole-image changes carry every object through one transform. Pixel objects undergo
//! quarter turns and flips exactly; any other change makes them image objects (so they keep
//! their pixels), unless resampling is asked for (see `design.md` › Document model).

use crate::memory::Allowance;
use crate::state::{AppState, MessageKind};
use opal_core::document::{Canvas, Document, IdGen, ImageObject, Layer, LayerContent, ObjectKind, PixelObject};
use opal_core::geom::{Affine, FRect, GridMap, IRect};
use opal_core::raster::RasterImage;
use opal_core::resample::{self, Sampling};
use opal_io::Progress;
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

/// Carries every object of `doc`, its selection, and its coordinates (`Document::carried`),
/// through `m`. Pixel objects stay pixels
/// where `m` is exact on the grid; otherwise they are resampled with `resample` if given, else
/// become image objects drawn with `sampling`. Resampling keeps only the part within `keep` of
/// what would reach too far (see `layer_ops::drawable`); returns whether it had to, or why
/// nothing could be done: the pixels drawn (and the selection's modifiers worked out again)
/// would need more than `memory`, or the edit `progress` tells of (an object at a time) was
/// cancelled.
pub fn transform_all(
    doc: &mut Document,
    m: &Affine,
    resample: Option<Sampling>,
    sampling: Sampling,
    keep: &IRect,
    memory: Allowance,
    progress: &Progress,
) -> Result<bool, String> {
    let mut to = Drawing::new(*keep, memory);
    // An object at a time, and the selection carried (its modifiers worked out again) last.
    let mut objects = 0;
    doc.for_each_layer(&mut |l, _| objects += l.objects().len());
    // (And one for the selections carried.)
    progress.begin(objects as u64 + 1);
    fn walk(
        layer: &mut Layer,
        m: &Affine,
        resample_with: Option<Sampling>,
        sampling: Sampling,
        to: &mut Drawing,
        progress: &Progress,
    ) -> Result<(), String> {
        match &mut layer.content {
            LayerContent::Objects(objects) => {
                for o in objects {
                    crate::edits::go_on(progress)?;
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
                    progress.advance(1);
                }
            }
            LayerContent::Group(children) => {
                for c in children {
                    walk(Arc::make_mut(c), m, resample_with, sampling, to, progress)?;
                }
            }
        }
        Ok(())
    }
    for l in &mut doc.layers {
        walk(Arc::make_mut(l), m, resample, sampling, &mut to, progress)?;
    }
    // What is selected goes with what it was selected on (its modifiers worked out again,
    // within what is left of the memory); what Reselect would bring back, and the selections
    // saved, once they are brought back, unless the change since would then be past what an
    // image object may be drawn through (which a file refuses): then they are carried now, as
    // such image objects are drawn into pixels.
    let selection = doc.selection.take();
    let kept: Vec<_> = doc.deselected.iter_mut().chain(doc.saved.iter_mut().map(|s| &mut s.kept)).collect();
    let carried = crate::memory::Admission::within(to.memory, &|| progress.is_cancelled(), |b| {
        for kept in kept {
            let since = kept.since.then(*m);
            if ImageObject::transform_ok(&since) {
                kept.since = since;
            } else {
                kept.selection = Arc::new(kept.selection.transformed_within(&since, b)?);
                kept.since = Affine::IDENTITY;
            }
        }
        selection.map(|s| s.transformed_within(m, b)).transpose()
    })?;
    doc.selection = carried.filter(|s| !s.is_empty()).map(Arc::new);
    doc.carried = doc.carried.then(*m);
    progress.advance(1);
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
                                Some(a) => a.union(&b),
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

/// The bounds of what every layer holds, hidden ones and those in hidden groups too, in
/// whole pixels.
pub fn all_bounds(doc: &Document) -> Option<IRect> {
    let b = doc.layers.iter().filter_map(|l| l.bounds()).reduce(|a, b| a.union(&b))?;
    Some(IRect::covering(&b)).filter(|r| !r.is_empty())
}

/// What Trim says when there is nothing to trim.
pub const NO_BORDER: &str = "There is no border to trim.";

/// What Trim takes as the border to cut away (see [`Trim`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimBy {
    /// Pixels transparent (within the tolerance, of alpha).
    Transparent,
    /// The colour of the top-left corner's pixel.
    TopLeft,
    /// The colour of the bottom-right corner's pixel.
    BottomRight,
}

/// The Trim dialog's fields (design.md, *Image size, crop and rotation*): what the border is,
/// how far a pixel may differ from it and still be border (0 to 255, the largest difference
/// of a channel, alpha included, in 8-bit sRGB terms), and the sides cut in from: top,
/// bottom, left, right.
#[derive(Clone, Debug, PartialEq)]
pub struct Trim {
    pub by: TrimBy,
    pub tolerance: f64,
    pub sides: [bool; 4],
}

impl Default for Trim {
    fn default() -> Trim {
        Trim { by: TrimBy::Transparent, tolerance: 0.0, sides: [true; 4] }
    }
}

/// How many rows (or columns) of `[from, to)` to read next from its start (`forward`) or its
/// end: to the next edge between tiles, so that each strip read lies within one row (or
/// column) of tiles, which the image is composited by.
fn strip(from: i64, to: i64, forward: bool) -> i64 {
    let tile = i64::from(opal_core::tile::TILE_SIZE);
    let n = if forward { tile - from.rem_euclid(tile) } else { (to - 1).rem_euclid(tile) + 1 };
    n.min(to - from)
}

/// What reads the image for Trim (see [`trimmed`]): a rectangle's 8-bit sRGB pixels with
/// straight alpha, row by row; or why not (the edit cancelled).
pub type StripReader<'a> = dyn Fn(&IRect) -> Result<Vec<[u8; 4]>, String> + 'a;

/// How many of `band`'s lines (its rows, or with `rows` false its columns), counted from its
/// start (`forward`) or its end, are border (`border`) all along: read through `read` a tile
/// at a time along them (the band lies within one row, or column, of tiles), each but the
/// first only as far as those before left lines all border.
fn border_lines(
    band: &IRect,
    rows: bool,
    forward: bool,
    read: &StripReader<'_>,
    border: &dyn Fn(&[u8; 4]) -> bool,
) -> Result<i64, String> {
    let n = if rows { band.height() } else { band.width() };
    let (mut at, to) = if rows { (band.x0, band.x1) } else { (band.y0, band.y1) };
    let mut all = n;
    while at < to && all > 0 {
        let step = strip(at, to, true);
        let piece = if rows {
            IRect::new(at, band.y0, at + step, band.y1)
        } else {
            IRect::new(band.x0, at, band.x1, at + step)
        };
        let px = read(&piece)?;
        let w = piece.width() as usize;
        let line = |i: usize| {
            if rows { px[i * w..(i + 1) * w].iter().all(border) } else { px.chunks(w).all(|row| border(&row[i])) }
        };
        let mut k = 0;
        while k < all && line(if forward { k } else { n - 1 - k } as usize) {
            k += 1;
        }
        all = k;
        at += step;
    }
    Ok(all)
}

/// `canvas` cut in to what is not border, as `spec` says, the image read through `read`
/// (8-bit sRGB pixels with straight alpha, row by row, of a rectangle; an error stops it,
/// the edit cancelled): from each side asked for, a row or column goes while every pixel of
/// it (within what is left) is border. None if every pixel is.
pub fn trimmed(canvas: IRect, spec: &Trim, read: &StripReader<'_>) -> Result<Option<IRect>, String> {
    if canvas.is_empty() {
        return Ok(None);
    }
    let corner = |x: i64, y: i64| read(&IRect::new(x, y, x + 1, y + 1)).map(|p| p[0]);
    let reference = match spec.by {
        TrimBy::Transparent => None,
        TrimBy::TopLeft => Some(corner(canvas.x0, canvas.y0)?),
        TrimBy::BottomRight => Some(corner(canvas.x1 - 1, canvas.y1 - 1)?),
    };
    let tol = spec.tolerance.round().clamp(0.0, 255.0) as i32;
    let border = |p: &[u8; 4]| match reference {
        None => i32::from(p[3]) <= tol,
        // Transparent both: alike, whatever colour neither shows.
        Some(r) if r[3] == 0 && i32::from(p[3]) <= tol => true,
        Some(r) => (0..4).all(|c| (i32::from(p[c]) - i32::from(r[c])).abs() <= tol),
    };
    let mut r = canvas;
    // From the top and the bottom: rows across all of it.
    let rows = |r: &IRect, from_top: bool| -> Result<i64, String> {
        let mut gone = 0;
        while gone < r.height() {
            let n = strip(r.y0 + gone * i64::from(from_top), r.y1 - gone * i64::from(!from_top), from_top);
            let (y0, y1) = if from_top { (r.y0 + gone, r.y0 + gone + n) } else { (r.y1 - gone - n, r.y1 - gone) };
            let k = border_lines(&IRect::new(r.x0, y0, r.x1, y1), true, from_top, read, &border)?;
            gone += k;
            if k < n {
                return Ok(gone);
            }
        }
        Ok(gone)
    };
    if spec.sides[0] {
        r.y0 += rows(&r, true)?;
    }
    if r.is_empty() {
        return Ok(None);
    }
    if spec.sides[1] {
        r.y1 -= rows(&r, false)?;
    }
    if r.is_empty() {
        return Ok(None);
    }
    // From the left and the right: columns down what is left.
    let columns = |r: &IRect, from_left: bool| -> Result<i64, String> {
        let mut gone = 0;
        while gone < r.width() {
            let n = strip(r.x0 + gone * i64::from(from_left), r.x1 - gone * i64::from(!from_left), from_left);
            let (x0, x1) = if from_left { (r.x0 + gone, r.x0 + gone + n) } else { (r.x1 - gone - n, r.x1 - gone) };
            let k = border_lines(&IRect::new(x0, r.y0, x1, r.y1), false, from_left, read, &border)?;
            gone += k;
            if k < n {
                return Ok(gone);
            }
        }
        Ok(gone)
    };
    if spec.sides[2] {
        r.x0 += columns(&r, true)?;
    }
    if r.is_empty() {
        return Ok(None);
    }
    if spec.sides[3] {
        r.x1 -= columns(&r, false)?;
    }
    Ok((!r.is_empty()).then_some(r))
}

/// The canvas size limit for the commands here, as for New Image.
const MAX_SIDE: u32 = crate::dialogs::MAX_SIDE;

impl AppState {
    pub(crate) fn turn(&mut self, turn: Turn) {
        let sampling = crate::prefs::sampling(&self.prefs, "transform");
        let doc = self.active_doc.expect("available");
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut IdGen, progress: &Progress| {
            let c = doc.canvas;
            let m = turn.map(&c);
            transform_all(doc, &m, None, sampling, &c.rect(), measure.now(), progress)?;
            if matches!(turn, Turn::Clockwise | Turn::Counterclockwise) {
                doc.canvas = Canvas { x: c.x, y: c.y, width: c.height, height: c.width };
            }
            Ok(())
        };
        self.edit_later(doc, turn.label(), Allowance::unlimited(), work, |_, _, ()| {});
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
        let spec = spec.clone();
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut IdGen, progress: &Progress| {
            doc.resolution = spec.resolution;
            let c = doc.canvas;
            let (x, y) = (c.x as f64, c.y as f64);
            let sx = f64::from(spec.width) / f64::from(c.width);
            let sy = f64::from(spec.height) / f64::from(c.height);
            let m = Affine::scale(sx, sy).about(x, y);
            let canvas = Canvas { width: spec.width, height: spec.height, ..c };
            let memory = measure.now();
            let resampled = resample.then_some(sampling);
            let clipped = transform_all(doc, &m, resampled, sampling, &canvas.rect(), memory, progress)?;
            doc.canvas = canvas;
            Ok(clipped)
        };
        let doc = d.id;
        self.edit_later(doc, "Resize image", Allowance::unlimited(), work, |st, _, clipped| {
            if clipped {
                st.notify(crate::layer_ops::CLIPPED);
            }
        });
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

    /// Trim (see [`Trim`]): the active image's canvas cut in to what is not border, worked
    /// out off the interface thread from the image as it shows (its visible layers
    /// composited), a strip of tiles at a time from the edges in. One history state; told
    /// why not, if there is nothing to trim or nothing but border.
    pub(crate) fn trim(&mut self, spec: &Trim) {
        let doc = self.active_doc.expect("available");
        let spec = spec.clone();
        let work = move |doc: &mut Document, _: &mut IdGen, progress: &Progress| {
            let layers = &doc.layers;
            // (How far it has got cannot be told: it stops where the border does.)
            let read = |r: &IRect| {
                let strip =
                    opal_core::composite::composite_until(layers, Some(r), &mut |_, _| !progress.is_cancelled())
                        .ok_or_else(|| crate::edits::CANCELLED.to_owned())?;
                Ok(strip.read_rect(r).into_iter().map(opal_io::pixels::to_srgb8).collect::<Vec<[u8; 4]>>())
            };
            let canvas = doc.canvas.rect();
            let Some(r) = trimmed(canvas, &spec, &read)? else {
                return Err("The image is all border: there would be nothing left.".to_owned());
            };
            if r == canvas {
                return Ok(false);
            }
            doc.canvas = Canvas { x: r.x0, y: r.y0, width: r.width() as u32, height: r.height() as u32 };
            Ok(true)
        };
        self.edit_later(doc, "Trim", Allowance::now(), work, |st, _, trimmed| {
            if !trimmed {
                st.notify(NO_BORDER);
            }
        });
    }

    /// Reveal All: the active image's canvas grown to hold every layer's content, hidden
    /// layers too, as well as what it holds now (never shrunk). One history state; told so,
    /// if all is shown already.
    pub(crate) fn reveal_all(&mut self) {
        let d = self.active().expect("available");
        let canvas = d.doc().canvas.rect();
        let all = all_bounds(d.doc()).map_or(canvas, |b| b.union(&canvas));
        let side = i64::from(MAX_SIDE);
        if all.width() > side || all.height() > side {
            let why = format!(
                "All the layers hold spans {} × {} pixels, more than a canvas may ({side} a side): move what lies \
                 far out nearer first.",
                all.width(),
                all.height()
            );
            return self.message(MessageKind::Info, "Reveal All", why);
        }
        self.crop_canvas_to(&all, "Reveal all", "Everything there is shows already.");
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
        let doc = self.active_doc.expect("available");
        let spec = spec.clone();
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut IdGen, progress: &Progress| {
            let c = doc.canvas;
            let r = c.rect().to_f();
            let [cx, cy] = r.center();
            let m = Affine::rotate(spec.degrees.to_radians()).about(cx, cy);
            transform_all(doc, &m, None, sampling, &c.rect(), measure.now(), progress)?;
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
            Ok(())
        };
        self.edit_later(doc, "Rotate", Allowance::unlimited(), work, |_, _, ()| {});
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
        transform_all(
            &mut d,
            &Turn::Clockwise.map(&c),
            None,
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
        assert!(
            close(pixel_at(&d, 10, 20), 0.4) && close(pixel_at(&d, 11, 20), 0.1) && close(pixel_at(&d, 11, 22), 0.2)
        );
        let mut d = corners();
        transform_all(
            &mut d,
            &Turn::Counterclockwise.map(&c),
            None,
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.2) && close(pixel_at(&d, 10, 22), 0.1));
        let mut d = corners();
        transform_all(
            &mut d,
            &Turn::Half.map(&c),
            None,
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.3) && close(pixel_at(&d, 12, 21), 0.1));
        let mut d = corners();
        transform_all(
            &mut d,
            &Turn::FlipHorizontal.map(&c),
            None,
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.2) && close(pixel_at(&d, 12, 21), 0.4));
        let mut d = corners();
        transform_all(
            &mut d,
            &Turn::FlipVertical.map(&c),
            None,
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
        assert!(close(pixel_at(&d, 10, 20), 0.4) && close(pixel_at(&d, 12, 21), 0.2));
    }

    #[test]
    fn selections_kept_are_carried_at_once_past_what_an_image_object_may_be_drawn_through() {
        use opal_core::document::{KeptSelection, SavedSelection};
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        let mut d = corners();
        let shape = Shape::rectangle(&IRect::new(10, 20, 12, 22).to_f(), false);
        let kept = KeptSelection::new(Arc::new(Selection::new(Step::Shape { shape, combine: Combine::Replace })));
        d.saved = vec![SavedSelection { name: "Corner".into(), kept: kept.clone() }];
        d.deselected = Some(kept);
        // Grown a thousandfold, then a little more: past what an image may be drawn through (and
        // a file keeps), so they are carried then and there.
        for k in [1000.0, 1001.0] {
            transform_all(
                &mut d,
                &Affine::scale(k, k).about(10.0, 20.0),
                None,
                Sampling::default(),
                &IRect::from_size(100, 100),
                Allowance::now(),
                &Progress::default(),
            )
            .unwrap();
        }
        for kept in [&d.saved[0].kept, d.deselected.as_ref().unwrap()] {
            assert!(ImageObject::transform_ok(&kept.since), "carried later, past what a file keeps: {:?}", kept.since);
        }
        assert_eq!(d.saved[0].kept.since, Affine::IDENTITY);
    }

    #[test]
    fn resizing_keeps_pixels_as_an_image_unless_resampling() {
        let mut d = corners();
        let m = Affine::scale(2.0, 2.0).about(10.0, 20.0);
        transform_all(
            &mut d,
            &m,
            None,
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
        assert!(matches!(d.layers[0].objects()[0].kind, ObjectKind::Image(_)), "kept as an image object");
        let mut d = corners();
        transform_all(
            &mut d,
            &m,
            Some(Sampling::default()),
            Sampling::default(),
            &IRect::from_size(100, 100),
            Allowance::now(),
            &Progress::default(),
        )
        .unwrap();
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

    /// What reads a `w × h` image whose pixel at `(x, y)` is `at(x, y)`: never past it, and
    /// (but for a corner) never a rectangle past an edge between tiles.
    fn reader(w: i64, h: i64, at: impl Fn(i64, i64) -> [u8; 4]) -> impl Fn(&IRect) -> Result<Vec<[u8; 4]>, String> {
        move |r: &IRect| {
            assert!(r.x0 >= 0 && r.y0 >= 0 && r.x1 <= w && r.y1 <= h, "read past the image: {r:?}");
            let tile = |v: i64| v.div_euclid(i64::from(opal_core::tile::TILE_SIZE));
            assert!(tile(r.x0) == tile(r.x1 - 1) && tile(r.y0) == tile(r.y1 - 1), "a read across tiles: {r:?}");
            Ok((r.y0..r.y1).flat_map(|y| (r.x0..r.x1).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect())
        }
    }

    #[test]
    fn trim_cuts_in_from_each_side_asked_while_rows_and_columns_are_border() {
        let canvas = IRect::new(0, 0, 300, 200);
        // A white margin, a little uneven (a scan's), round a red block.
        let read = reader(300, 200, |x, y| match (x, y) {
            _ if (70..230).contains(&x) && (40..170).contains(&y) => [200, 0, 0, 255],
            _ if (x + y) % 7 == 0 => [247, 250, 252, 255],
            _ => [255, 255, 255, 255],
        });
        let by_corner = |tolerance: f64, sides: [bool; 4]| Trim { by: TrimBy::TopLeft, tolerance, sides };
        assert_eq!(trimmed(canvas, &by_corner(10.0, [true; 4]), &read), Ok(Some(IRect::new(70, 40, 230, 170))));
        // Too strict for the scan's unevenness: stopped by its first uneven row and column.
        assert_eq!(trimmed(canvas, &by_corner(0.0, [true; 4]), &read), Ok(Some(canvas)));
        // Only the top and the left.
        let some = by_corner(10.0, [true, false, true, false]);
        assert_eq!(trimmed(canvas, &some, &read), Ok(Some(IRect::new(70, 40, 300, 200))));
        // By the bottom-right corner's colour: a dark corner, nothing else of it.
        let cornered = reader(300, 200, |x, y| if (x, y) == (299, 199) { [0, 0, 0, 255] } else { [255; 4] });
        let dark = Trim { by: TrimBy::BottomRight, tolerance: 10.0, sides: [true; 4] };
        assert_eq!(trimmed(canvas, &dark, &cornered), Ok(Some(canvas)));
        // By the top-left's (white), on the same: all but that dark pixel goes.
        let light = Trim { by: TrimBy::TopLeft, ..dark };
        assert_eq!(trimmed(canvas, &light, &cornered), Ok(Some(IRect::new(299, 199, 300, 200))));
        // Transparent pixels round a block; and all border, none left.
        let clear =
            reader(
                300,
                200,
                |x, y| {
                    if (10..20).contains(&x) && (5..9).contains(&y) { [9, 9, 9, 255] } else { [0, 0, 0, 0] }
                },
            );
        assert_eq!(trimmed(canvas, &Trim::default(), &clear), Ok(Some(IRect::new(10, 5, 20, 9))));
        let empty = reader(300, 200, |_, _| [0, 0, 0, 0]);
        assert_eq!(trimmed(canvas, &Trim::default(), &empty), Ok(None));
        // All border, trimmed from the bottom up (not the top): nothing left after it, and no
        // column read then.
        let bottom = Trim { sides: [false, true, true, true], ..Trim::default() };
        let reads = std::cell::RefCell::new(Vec::new());
        let kept = |r: &IRect| {
            reads.borrow_mut().push(*r);
            empty(r)
        };
        assert_eq!(trimmed(canvas, &bottom, &kept), Ok(None));
        assert!(reads.borrow().iter().all(|r| !r.is_empty()), "read past where all of it went: {:?}", reads.borrow());
        // Taller than a tile, from the edges in: each strip within a row of tiles (as the
        // reader checks).
        let tall = IRect::new(0, 100, 50, 700);
        let read_tall = reader(50, 700, |_, y| if y >= 650 { [9, 9, 9, 255] } else { [0; 4] });
        assert_eq!(trimmed(tall, &Trim::default(), &read_tall), Ok(Some(IRect::new(0, 650, 50, 700))));
        // Cancelled: given up, saying so.
        let cancelled = |_: &IRect| Err(crate::edits::CANCELLED.to_owned());
        assert_eq!(trimmed(canvas, &Trim::default(), &cancelled), Err(crate::edits::CANCELLED.to_owned()));
    }

    #[test]
    fn trim_cuts_the_image_s_canvas_in_and_reveal_all_brings_back_hidden_layers_too() {
        use crate::state::{Background, NewImage};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        // A block of red on a layer of its own; and a hidden layer reaching past the canvas.
        let d = st.active_mut().unwrap();
        d.apply("Paint", |doc, ids| {
            let mut block = RasterImage::new();
            let mut far = RasterImage::new();
            for y in 40..170 {
                for x in 70..230 {
                    block.set_pixel(x, y, pixel_from_f32([0.6, 0.0, 0.0, 1.0]));
                }
            }
            far.set_pixel(-50, 250, pixel_from_f32([0.0, 0.0, 1.0, 1.0]));
            let mut hidden = Layer::new_raster(ids.layer(), ids.object(), "far", far);
            hidden.visible = false;
            doc.insert(
                Position { parent: None, index: 1 },
                Layer::new_raster(ids.layer(), ids.object(), "block", block),
            )?;
            doc.insert(Position { parent: None, index: 2 }, hidden)?;
            Ok::<(), opal_core::document::TreeError>(())
        })
        .unwrap();
        st.trim(&Trim { by: TrimBy::TopLeft, tolerance: 0.0, sides: [true; 4] });
        st.finish_edits();
        let d = st.active().unwrap();
        assert_eq!(d.doc().canvas.rect(), IRect::new(70, 40, 230, 170));
        assert_eq!(d.history.undo_label(), Some("Trim"));
        // Trimmed again by its corner: all of it is border now, and nothing is trimmed. By
        // transparency: there is none, which is said, not refused.
        st.trim(&Trim { by: TrimBy::TopLeft, tolerance: 0.0, sides: [true; 4] });
        st.finish_edits();
        assert!(st.messages.back().is_some_and(|m| m.body.contains("all border")), "{:?}", st.messages);
        let told = st.messages.len();
        st.trim(&Trim::default());
        st.finish_edits();
        assert_eq!(st.notice.as_ref().map(|n| n.0.as_str()), Some(NO_BORDER));
        assert_eq!(st.messages.len(), told, "refused, not said");
        assert_eq!(st.active().unwrap().doc().canvas.rect(), IRect::new(70, 40, 230, 170));
        // Reveal All: the white background's, and the hidden layer's pixel past it.
        st.reveal_all();
        let d = st.active().unwrap();
        assert_eq!(d.doc().canvas.rect(), IRect::new(-50, 0, 300, 251));
        assert_eq!(d.history.undo_label(), Some("Reveal all"));
        st.reveal_all();
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Reveal all"));
        assert!(st.notice.as_ref().is_some_and(|n| n.0.contains("shows already")), "{:?}", st.notice);
        // Content smaller than the canvas: the canvas is not shrunk to it.
        st.create_image(&NewImage { width: 100, height: 100, background: Background::Transparent, resolution: 72.0 });
        st.active_mut()
            .unwrap()
            .apply("Dot", |doc, ids| {
                let mut dot = RasterImage::new();
                dot.set_pixel(5, 5, pixel_from_f32([1.0, 1.0, 1.0, 1.0]));
                doc.insert(
                    Position { parent: None, index: 1 },
                    Layer::new_raster(ids.layer(), ids.object(), "dot", dot),
                )
            })
            .unwrap();
        st.reveal_all();
        assert_eq!(st.active().unwrap().doc().canvas.rect(), IRect::new(0, 0, 100, 100), "shrunk");
        // Content further out than a canvas may reach: refused, saying so, the canvas as it was.
        st.active_mut()
            .unwrap()
            .apply("Far", |doc, ids| {
                let mut far = RasterImage::new();
                far.set_pixel(-400_000, 5, pixel_from_f32([1.0, 1.0, 1.0, 1.0]));
                doc.insert(
                    Position { parent: None, index: 2 },
                    Layer::new_raster(ids.layer(), ids.object(), "far", far),
                )
            })
            .unwrap();
        st.reveal_all();
        assert_eq!(st.active().unwrap().doc().canvas.rect(), IRect::new(0, 0, 100, 100), "moved off what it showed");
        assert!(st.messages.back().is_some_and(|m| m.body.contains("more than a canvas may")), "{:?}", st.messages);
    }
}
