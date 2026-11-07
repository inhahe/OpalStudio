//! Importing and exporting ordinary image files (PNG and JPEG).

use crate::memory::{Free, Need};
use crate::pixels::{from_srgb8, from_srgb16, pixels_to_rgba8, pixels_to_rgba16};
use crate::{IoError, Progress, write_atomically};
use half::f16;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageReader};
use opal_core::composite::{composite_within_until, flatten};
use opal_core::document::{Document, Layer};
use opal_core::geom::{Affine, IRect};
use opal_core::raster::RasterImage;
use opal_core::resample::{self, Sampling};
use opal_core::tile::{Pixel, TILE_SIZE};
use rayon::prelude::*;
use std::cell::RefCell;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::Arc;

/// An image read from a file, converted to working pixels with its top-left corner at the
/// canvas origin.
pub struct Imported {
    pub width: u32,
    pub height: u32,
    pub image: RasterImage,
    /// Pixels per inch across and down, if the file says (turned with the image).
    pub resolution: Option<[f64; 2]>,
    /// Things the user should know about how the file was read.
    pub warnings: Vec<String>,
}

/// Reads a PNG or JPEG (recognized by content, not extension), turned upright according to
/// its EXIF orientation.
pub fn import(path: &Path, free: Option<Free>) -> Result<Imported, IoError> {
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    import_bytes(&bytes, path, free)
}

/// Like [`import`], from bytes already in memory; `path` is used only in messages. Refused
/// before it is decoded if it would need more memory than `free` (if known) allows, which,
/// as the need counts the bytes, should be measured before they were read (see
/// [`Free::counting`]).
pub fn import_bytes(bytes: &[u8], path: &Path, free: Option<Free>) -> Result<Imported, IoError> {
    let decode_err = |e: image::ImageError| IoError::Decode { path: path.to_owned(), message: e.to_string() };
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| IoError::io(path, e))?;
    let Some(format) = reader.format() else {
        return Err(IoError::Unsupported { path: path.to_owned(), message: "not a PNG or JPEG image".into() });
    };
    let mut decoder = reader.into_decoder().map_err(decode_err)?;
    let orientation = decoder.orientation().map_err(decode_err)?;
    let depth = Depth::of(decoder.color_type());
    // The file's bytes, held while it is decoded, too.
    let need = import_memory(decoder.dimensions(), decoder.total_bytes(), orientation, depth)
        .plus(Need::at_once(bytes.len() as u64));
    if let Some(free) = free {
        free.check(need).map_err(|short| IoError::TooLarge { path: path.to_owned(), short })?;
    }
    let _hold = free.map(|f| f.hold(need.at_once));
    let icc = decoder.icc_profile().map_err(decode_err)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(decode_err)?;
    img.apply_orientation(orientation);

    let (width, height) = (img.width(), img.height());
    let image = image_of(&img);

    let mut warnings = Vec::new();
    // Turned a quarter, the file's across is the image's down.
    let turned = matches!(
        orientation,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    );
    let resolution = crate::density::read(bytes, format).map(|[x, y]| if turned { [y, x] } else { [x, y] });
    if let Some(profile) = icc
        && !looks_like_srgb(&profile)
    {
        warnings.push(
            "This image has an embedded color profile other than sRGB. Until color management \
             arrives, its colors are read as sRGB and may look different from other programs."
                .to_owned(),
        );
    }
    Ok(Imported { width, height, image, resolution, warnings })
}

/// How a decoded image's samples become tiles (by its colour type, which [`image_of`]'s
/// choice follows): 8 bits a channel kept at 8 bits; 16 as working pixels; anything else
/// (no PNG or JPEG decodes to it) converted whole to 16 bits first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    Eight,
    Sixteen,
    Other,
}

impl Depth {
    fn of(color: image::ColorType) -> Depth {
        use image::ColorType::*;
        match color {
            L8 | La8 | Rgb8 | Rgba8 => Depth::Eight,
            L16 | La16 | Rgb16 | Rgba16 => Depth::Sixteen,
            _ => Depth::Other,
        }
    }

    /// The bytes a pixel takes in its tiles, and in one go beyond the decoder's buffer (a
    /// whole converted copy).
    fn bytes(self) -> (u64, u64) {
        match self {
            Depth::Eight => (4, 0),
            Depth::Sixteen => (8, 0),
            Depth::Other => (8, 8),
        }
    }
}

/// The memory opening an image of `dimensions` takes, given the bytes its decoder makes
/// (`decoded`) and the `depth` of its samples: in one go those, a turned copy of them if
/// `orientation` turns it, and a converted copy if the depth needs one; and its tiles,
/// whole ones however little of the edge ones the image covers.
fn import_memory(
    dimensions: (u32, u32),
    decoded: u64,
    orientation: image::metadata::Orientation,
    depth: Depth,
) -> Need {
    // Saturating: a header may claim any size, and a wrapped sum would pass the check.
    let (w, h) = (u64::from(dimensions.0), u64::from(dimensions.1));
    let t = u64::from(TILE_SIZE);
    let tiles = w.div_ceil(t).saturating_mul(h.div_ceil(t)).saturating_mul(t * t);
    let turned = if orientation == image::metadata::Orientation::NoTransforms { 0 } else { decoded };
    let (per_tile_pixel, converted) = depth.bytes();
    Need {
        at_once: decoded.saturating_add(turned).saturating_add(w.saturating_mul(h).saturating_mul(converted)),
        tiles: tiles.saturating_mul(per_tile_pixel),
    }
}

/// A decoded image as tiles, made straight from its samples whatever its channels (gray,
/// gray and alpha, RGB, RGBA), as its [`Depth`] says.
fn image_of(img: &DynamicImage) -> RasterImage {
    let (w, h) = (img.width(), img.height());
    match img {
        DynamicImage::ImageLuma8(b) => from_samples8::<1>(w, h, b.as_raw()),
        DynamicImage::ImageLumaA8(b) => from_samples8::<2>(w, h, b.as_raw()),
        DynamicImage::ImageRgb8(b) => from_samples8::<3>(w, h, b.as_raw()),
        DynamicImage::ImageRgba8(b) => from_samples8::<4>(w, h, b.as_raw()),
        DynamicImage::ImageLuma16(b) => from_samples16::<1>(w, h, b.as_raw()),
        DynamicImage::ImageLumaA16(b) => from_samples16::<2>(w, h, b.as_raw()),
        DynamicImage::ImageRgb16(b) => from_samples16::<3>(w, h, b.as_raw()),
        DynamicImage::ImageRgba16(b) => from_samples16::<4>(w, h, b.as_raw()),
        other => from_samples16::<4>(w, h, other.to_rgba16().as_raw()),
    }
}

/// An image of 8-bit samples, `C` a pixel, kept at 8 bits.
fn from_samples8<const C: usize>(width: u32, height: u32, samples: &[u8]) -> RasterImage {
    let w = width as usize;
    RasterImage::from_srgb8_rows(width, height, |y, x0, out| {
        let from = &samples[(y * w + x0) * C..][..out.len() * C];
        if C == 4 {
            out.copy_from_slice(bytemuck::cast_slice(from));
        } else {
            for (o, s) in out.iter_mut().zip(from.chunks_exact(C)) {
                *o = rgba::<u8, C>(s, u8::MAX);
            }
        }
    })
}

/// An image of 16-bit samples, `C` a pixel, as working pixels.
fn from_samples16<const C: usize>(width: u32, height: u32, samples: &[u16]) -> RasterImage {
    let w = width as usize;
    RasterImage::from_rows(width, height, |y, x0, out| {
        let from = &samples[(y * w + x0) * C..][..out.len() * C];
        for (o, s) in out.iter_mut().zip(from.chunks_exact(C)) {
            *o = from_srgb16(rgba::<u16, C>(s, u16::MAX));
        }
    })
}

/// A pixel's `C` samples (gray, gray and alpha, RGB or RGBA) as RGBA, `opaque` the alpha of
/// those without.
fn rgba<T: Copy, const C: usize>(s: &[T], opaque: T) -> [T; 4] {
    match C {
        1 => [s[0], s[0], s[0], opaque],
        2 => [s[0], s[0], s[0], s[1]],
        3 => [s[0], s[1], s[2], opaque],
        _ => [s[0], s[1], s[2], s[3]],
    }
}

/// Whether an ICC profile is (by its description) one of the sRGB profiles, which need no
/// conversion. Checks for "sRGB" in ASCII or UTF-16, as the description tag stores it.
fn looks_like_srgb(profile: &[u8]) -> bool {
    let ascii = profile.windows(4).any(|w| w == b"sRGB");
    let utf16: [u8; 8] = [0, b's', 0, b'R', 0, b'G', 0, b'B'];
    ascii || profile.windows(8).any(|w| w == utf16)
}

/// How to export the flattened canvas.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportFormat {
    /// PNG, with transparency; 16 bits per channel when `sixteen_bit`.
    Png { sixteen_bit: bool },
    /// JPEG at `quality` (1–100). JPEG has no transparency, so transparent areas are filled
    /// with `background` (8-bit sRGB).
    Jpeg { quality: u8, background: [u8; 3] },
}

/// `w × h` pixels as a 16-bit PNG file's bytes (for the clipboard), compressed quickly,
/// recording `ppi` pixels per inch if given.
pub fn encode_png16(pixels: &[Pixel], w: u32, h: u32, ppi: Option<f64>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Sixteen);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Adaptive);
    encoder.set_pixel_dims(ppi.map(crate::density::png_dims));
    let bytes: Vec<u8> = pixels_to_rgba16(pixels).iter().flat_map(|v| v.to_be_bytes()).collect();
    let mut writer = encoder.write_header().expect("encoding into memory cannot fail");
    writer.write_image_data(&bytes).expect("encoding into memory cannot fail");
    writer.finish().expect("encoding into memory cannot fail");
    out
}

/// A size to export at other than the canvas's, and how to resample to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSize {
    pub width: u32,
    pub height: u32,
    pub sampling: Sampling,
}

/// The largest JPEG image, along either side (the format stores sizes in 16 bits).
pub const JPEG_MAX_SIDE: u32 = 65_535;

/// How many rows of the exported image are made at a time.
const BAND: i64 = TILE_SIZE as i64;

/// The canvas of a document as exported, at its own size or another, made a band of rows
/// at a time from the top: a band is all of the result ever held (with, at another size,
/// the image it is drawn from).
pub struct Exported<'d> {
    pub width: u32,
    pub height: u32,
    source: Source<'d>,
    /// The first row not made yet.
    next: u32,
}

enum Source<'d> {
    /// At the canvas's size: the layers, composited a band at a time.
    Canvas { layers: &'d [Arc<Layer>], canvas: IRect },
    /// At another size: `image` drawn through `to_size` (its pixels to the result's).
    Resized { image: Box<RasterImage>, to_size: Affine, sampling: Sampling },
}

/// The memory exporting `doc` (at `size`, if given) takes beyond the document: in one go,
/// the bands in flight (one being made, as tiles and then as rows; one waiting; one being
/// encoded, and its encoding), and the canvas rows a shrunk result is made from; and at
/// another size the tiles of the image the result is drawn from, with its edges copied
/// outward ([`frame_edges`]).
pub fn export_memory(doc: &Document, size: Option<&ExportSize>) -> Need {
    use opal_core::raster::TILE_BYTES;
    let canvas = doc.canvas.rect();
    let band = |w: u32| u64::from(w) * BAND as u64 * 8;
    let Some(s) = size.filter(|s| resizes(doc, s)) else { return Need::at_once(5 * band(doc.canvas.width)) };
    let (at, border) = resized_frame(&canvas, s);
    let (rows, source) = if shrink_factors(&canvas, s) == (1, 1) {
        (0, opal_core::composite::composite_cost(&doc.layers, Some(&canvas)))
    } else {
        (2 * band(doc.canvas.width), tiles_over(&at) * TILE_BYTES)
    };
    // The tiles the edges are copied into: those reaching past the canvas's pixels (which,
    // shared with a layer, are copied first).
    let edges = tiles_over(&at.expand(border)) - tiles_within(&at);
    Need { at_once: 5 * band(s.width) + rows, tiles: source + edges * TILE_BYTES }
}

/// How many tiles `r` touches.
fn tiles_over(r: &IRect) -> u64 {
    let t = i64::from(TILE_SIZE);
    let n = |a: i64, b: i64| if b > a { ((b - 1).div_euclid(t) - a.div_euclid(t) + 1) as u64 } else { 0 };
    n(r.x0, r.x1) * n(r.y0, r.y1)
}

/// How many tiles lie wholly within `r`.
fn tiles_within(r: &IRect) -> u64 {
    let t = i64::from(TILE_SIZE);
    let n = |a: i64, b: i64| (b.div_euclid(t) - (a + t - 1).div_euclid(t)).max(0) as u64;
    n(r.x0, r.x1) * n(r.y0, r.y1)
}

/// For an export of `canvas` at `s`: where the canvas's pixels are in the image the result
/// is drawn from (at the origin, averaged in blocks, when shrinking by 2 or more; the
/// canvas itself otherwise), and how far past them its edge pixels are copied (as far as
/// the filter reaches).
fn resized_frame(canvas: &IRect, s: &ExportSize) -> (IRect, i64) {
    let (kx, ky) = shrink_factors(canvas, s);
    // Blocks along the right and bottom edges may be part blocks (see `shrunk`).
    let blocks = |n: i64, k: u32| (n + i64::from(k) - 1) / i64::from(k);
    let at =
        if kx * ky >= 2 { IRect::from_size(blocks(canvas.width(), kx), blocks(canvas.height(), ky)) } else { *canvas };
    (at, resample::reach(&to_size(&at, s), &s.sampling).ceil() as i64 + 1)
}

/// The transform from the pixels at `at` to those of the result at `s`.
fn to_size(at: &IRect, s: &ExportSize) -> Affine {
    Affine::translate(-at.x0 as f64, -at.y0 as f64)
        .then(Affine::scale(f64::from(s.width) / at.width() as f64, f64::from(s.height) / at.height() as f64))
}

/// Whether exporting `doc` at `s` draws it at another size.
fn resizes(doc: &Document, s: &ExportSize) -> bool {
    let (w, h) = (doc.canvas.width, doc.canvas.height);
    (s.width, s.height) != (w, h) && w > 0 && h > 0
}

/// The whole factors `canvas` is averaged down by before resampling to `s` (1 where it is
/// not shrunk by 2 or more), so that the resampler's factor is under 2 along each axis.
fn shrink_factors(canvas: &IRect, s: &ExportSize) -> (u32, u32) {
    let (w, h) = (canvas.width() as u32, canvas.height() as u32);
    ((w / s.width.max(1)).max(1), (h / s.height.max(1)).max(1))
}

impl<'d> Exported<'d> {
    /// The canvas of `doc` to export, at `size` if given. `progress` is told the work there
    /// is, in pixels: those of the result, and at another size those of the canvas too,
    /// which is composited here (and counted as it is). `None` if `progress` is cancelled
    /// meanwhile.
    pub fn new(doc: &'d Document, size: Option<&ExportSize>, progress: &Progress) -> Option<Exported<'d>> {
        let canvas = doc.canvas.rect();
        let (w, h) = (doc.canvas.width, doc.canvas.height);
        let pixels = |w: u32, h: u32| u64::from(w) * u64::from(h);
        Some(match size {
            Some(s) if resizes(doc, s) => {
                progress.begin(pixels(w, h) + pixels(s.width, s.height));
                let source = resized(&doc.layers, &canvas, s, progress)?;
                Exported { width: s.width, height: s.height, source, next: 0 }
            }
            _ => {
                progress.begin(pixels(w, h));
                Exported { width: w, height: h, source: Source::Canvas { layers: &doc.layers, canvas }, next: 0 }
            }
        })
    }

    /// The next band of rows, top to bottom: its pixels row by row, `width` to a row.
    /// `None` once every row has been made.
    pub fn band(&mut self) -> Option<Vec<Pixel>> {
        if self.next >= self.height {
            return None;
        }
        let y0 = i64::from(self.next);
        let px = match &self.source {
            Source::Canvas { layers, canvas } => {
                // Ending on the document's tile rows, so that each tile is composited once.
                let top = canvas.y0 + y0;
                let y1 = ((top.div_euclid(BAND) + 1) * BAND).min(canvas.y1);
                self.next = (y1 - canvas.y0) as u32;
                flatten(layers, &IRect::new(canvas.x0, top, canvas.x1, y1))
            }
            Source::Resized { image, to_size, sampling } => {
                let y1 = (y0 + BAND).min(i64::from(self.height));
                self.next = y1 as u32;
                let out = IRect::new(0, y0, i64::from(self.width), y1);
                resample::render_image(image, to_size, sampling, Some(&out)).read_rect(&out)
            }
        };
        Some(px)
    }
}

/// What an export at another size is drawn from: the canvas composited (averaged in whole
/// blocks first when shrinking by 2 or more, which leaves the resampler a factor under 2
/// along each axis, and so a narrow reach), with its edge pixels copied outward as far as
/// the filter reaches. The canvas is the whole image: what lies past it must not bleed into
/// the edges, and the edges must not fade out either. The canvas's pixels composited
/// advance `progress`; `None` if it is cancelled meanwhile.
fn resized<'d>(layers: &'d [Arc<Layer>], canvas: &IRect, s: &ExportSize, progress: &Progress) -> Option<Source<'d>> {
    let (kx, ky) = shrink_factors(canvas, s);
    let (at, border) = resized_frame(canvas, s);
    let mut image =
        if kx * ky >= 2 { shrunk(layers, canvas, kx, ky, progress)?.0 } else { composited(layers, canvas, progress)? };
    frame_edges(&mut image, &at, border);
    Some(Source::Resized { image: Box::new(image), to_size: to_size(&at, s), sampling: s.sampling })
}

/// The canvas composited ([`composite_within_until`], tiles alike made once across all of
/// it), advancing `progress` by the canvas's pixels as its tiles are made; `None` if it is
/// cancelled meanwhile.
fn composited(layers: &[Arc<Layer>], canvas: &IRect, progress: &Progress) -> Option<RasterImage> {
    let pixels = (canvas.width() * canvas.height()) as u64;
    let mut told = 0;
    let mut tell = |placed: usize, total: usize| {
        let now = pixels * placed as u64 / total.max(1) as u64;
        progress.advance(now - told);
        told = now;
    };
    let image = composite_within_until(layers, canvas, &mut |placed, total| {
        tell(placed, total);
        !progress.is_cancelled()
    })?;
    tell(1, 1);
    Some(image)
}

/// The canvas averaged in blocks of `kx × ky` pixels (the blocks along the right and bottom
/// edges average the pixels they hold, when the factors do not divide the size), placed at
/// the origin, with its width and height. The canvas is composited a tile row at a time;
/// the block rows a band holds whole are averaged in parallel, and one a band ends inside
/// is summed on into the next. Each band advances `progress` by its pixels; `None` if it
/// is cancelled meanwhile.
fn shrunk(
    layers: &[Arc<Layer>],
    canvas: &IRect,
    kx: u32,
    ky: u32,
    progress: &Progress,
) -> Option<(RasterImage, u32, u32)> {
    let (w, h, kx, ky) = (canvas.width() as usize, canvas.height() as usize, kx as usize, ky as usize);
    let (sw, sh) = (w.div_ceil(kx), h.div_ceil(ky));
    // Adds the rows `rows` to the block sums `acc`, groups of rows in parallel, each into
    // sums of its own that are then added up (a block taller than a band is summed across
    // bands, a band at a time, so the rows are what there is to share out).
    const ROWS: usize = 16;
    let sum = |acc: &mut [[f32; 4]], rows: &[Pixel]| {
        let part = rows
            .par_chunks(w * ROWS)
            .map(|rows| {
                let mut part = vec![[0.0f32; 4]; sw];
                for row in rows.chunks_exact(w) {
                    for (i, a) in part.iter_mut().enumerate() {
                        for p in &row[i * kx..((i + 1) * kx).min(w)] {
                            for c in 0..4 {
                                a[c] += p[c].to_f32();
                            }
                        }
                    }
                }
                part
            })
            .reduce_with(|mut a, b| {
                a.iter_mut().zip(&b).for_each(|(x, y)| (0..4).for_each(|c| x[c] += y[c]));
                a
            });
        if let Some(part) = part {
            acc.iter_mut().zip(&part).for_each(|(x, y)| (0..4).for_each(|c| x[c] += y[c]));
        }
    };
    // The block averages from sums over `rows` rows.
    let average = |acc: &[[f32; 4]], rows: usize| -> Vec<Pixel> {
        let n = |i: usize| ((((i + 1) * kx).min(w) - i * kx) * rows) as f32;
        acc.iter().enumerate().map(|(i, a)| a.map(|v| f16::from_f32(v / n(i)))).collect()
    };
    let mut out = RasterImage::new();
    // A block row begun in an earlier band, and how many of its rows are summed.
    let mut acc = vec![[0.0f32; 4]; sw];
    let mut summed = 0;
    // Finished block rows not yet written, from result row `written`.
    let mut done: Vec<Pixel> = Vec::new();
    let mut written = 0;
    let mut y = 0;
    while y < h {
        if progress.is_cancelled() {
            return None;
        }
        let top = canvas.y0 + y as i64;
        let y1 = (((top.div_euclid(BAND) + 1) * BAND - canvas.y0) as usize).min(h);
        let px = flatten(layers, &IRect::new(canvas.x0, top, canvas.x1, canvas.y0 + y1 as i64));
        let rows = |r0: usize, r1: usize| &px[(r0 - y) * w..(r1 - y) * w];
        let mut r = y;
        if summed > 0 {
            let block_end = ((r / ky + 1) * ky).min(h);
            let end = block_end.min(y1);
            sum(&mut acc, rows(r, end));
            summed += end - r;
            r = end;
            if r == block_end {
                done.extend(average(&acc, summed));
                acc.fill([0.0; 4]);
                summed = 0;
            }
        }
        if summed == 0 {
            let whole = if y1 == h { (y1 - r).div_ceil(ky) } else { (y1 - r) / ky };
            let blocks: Vec<Vec<Pixel>> = (0..whole)
                .into_par_iter()
                .map(|b| {
                    let (r0, r1) = (r + b * ky, (r + (b + 1) * ky).min(h));
                    let mut a = vec![[0.0f32; 4]; sw];
                    sum(&mut a, rows(r0, r1));
                    average(&a, r1 - r0)
                })
                .collect();
            blocks.into_iter().for_each(|b| done.extend(b));
            r = (r + whole * ky).min(h);
            if r < y1 {
                sum(&mut acc, rows(r, y1));
                summed = y1 - r;
            }
        }
        let ready = done.len() / sw;
        if ready >= TILE_SIZE as usize || (y1 == h && ready > 0) {
            out.write_rect(&IRect::new(0, written as i64, sw as i64, (written + ready) as i64), &done);
            written += ready;
            done.clear();
        }
        progress.advance(((y1 - y) * w) as u64);
        y = y1;
    }
    Some((out, sw as u32, sh as u32))
}

/// Copies the pixels along the edges of `rect` in `image` outward, `border` deep on every
/// side (the corners from the corner pixels).
fn frame_edges(image: &mut RasterImage, rect: &IRect, border: i64) {
    let (x0, y0, x1, y1) = (rect.x0, rect.y0, rect.x1 - 1, rect.y1 - 1);
    for y in y0..=y1 {
        let (left, right) = (image.pixel(x0, y), image.pixel(x1, y));
        for b in 1..=border {
            image.set_pixel(x0 - b, y, left);
            image.set_pixel(x1 + b, y, right);
        }
    }
    for x in x0 - border..=x1 + border {
        let (top, bottom) = (image.pixel(x, y0), image.pixel(x, y1));
        for b in 1..=border {
            image.set_pixel(x, y0 - b, top);
            image.set_pixel(x, y1 + b, bottom);
        }
    }
}

/// Flattens the canvas of `doc`, at its own size or at `size`, and writes it to `path`
/// (atomically). The image is made and encoded a band of rows at a time, the next band made
/// while the encoder compresses this one, so a canvas exported at its own size is never
/// held whole.
pub fn export(doc: &Document, path: &Path, format: &ExportFormat, size: Option<&ExportSize>) -> Result<(), IoError> {
    export_in(doc, path, format, size, None, &Progress::default())
}

/// [`export`], compositing in `pool` (rayon's global pool if `None`), telling `progress`
/// how far it has got (see [`Exported::new`]: the result's pixels count as the encoder
/// takes them), and stopping if it is cancelled. The bands are made on a thread of their
/// own, which gives its parallel work to `pool`; the encoder, which waits for them, runs
/// on the calling thread (which may be one of `pool`'s, if it has another).
pub fn export_in(
    doc: &Document,
    path: &Path,
    format: &ExportFormat,
    size: Option<&ExportSize>,
    pool: Option<&rayon::ThreadPool>,
    progress: &Progress,
) -> Result<(), IoError> {
    if let ExportFormat::Jpeg { .. } = format {
        let (w, h) = size.map_or((doc.canvas.width, doc.canvas.height), |s| (s.width, s.height));
        if w.max(h) > JPEG_MAX_SIDE {
            let message = format!("a JPEG image can be at most {JPEG_MAX_SIDE} pixels wide and tall");
            return Err(IoError::Unsupported { path: path.to_owned(), message });
        }
    }
    let encode_err = |message: String| IoError::Encode { path: path.to_owned(), message };
    in_bands(doc, size, pool, progress, |width, height, bands| {
        write_atomically(path, Some(progress), |out| match format {
            ExportFormat::Png { sixteen_bit } => {
                write_png(out, width, height, bands, *sixteen_bit, Some(doc.resolution))
                    .map_err(|e| encode_err(e.to_string()))
            }
            ExportFormat::Jpeg { quality, background } => {
                let bg = from_srgb8([background[0], background[1], background[2], 255]);
                let rows = JpegRows::new(bands, width, height, bg);
                let mut encoder = JpegEncoder::new_with_quality(out, (*quality).clamp(1, 100));
                encoder.set_pixel_density(crate::density::jpeg_density(doc.resolution));
                encoder.encode_image(&rows).map_err(|e| encode_err(e.to_string()))
            }
        })
    })
    .unwrap_or_else(|| Err(IoError::Cancelled { path: path.to_owned() }))
}

/// The canvas of `doc`, at its own size or at `size`, as an 8-bit PNG file's bytes, made
/// as [`export_in`] makes a file (compositing in `pool`).
pub fn png_in_memory(
    doc: &Document,
    size: Option<&ExportSize>,
    pool: Option<&rayon::ThreadPool>,
) -> Result<Vec<u8>, String> {
    in_bands(doc, size, pool, &Progress::default(), |width, height, bands| {
        let mut out = Vec::new();
        write_png(&mut out, width, height, bands, false, None).map(|()| out).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|| Err("cancelled".to_owned()))
}

/// Makes the canvas of `doc` (at `size`) a band of rows at a time, top to bottom, on a
/// thread of its own that gives its parallel work to `pool` (rayon's global pool if
/// `None`), and hands `take` the image's width and height and the bands as they come, each
/// told to `progress` as it is taken. One band waits beside the one being taken; a `take`
/// that stops early stops the bands. `None` if cancelled before it began.
fn in_bands<R>(
    doc: &Document,
    size: Option<&ExportSize>,
    pool: Option<&rayon::ThreadPool>,
    progress: &Progress,
    take: impl FnOnce(u32, u32, &mut dyn Iterator<Item = Vec<Pixel>>) -> R,
) -> Option<R> {
    let mut rows = Exported::new(doc, size, progress)?;
    let (width, height) = (rows.width, rows.height);
    Some(std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            let mut make = move || {
                while let Some(band) = rows.band() {
                    if tx.send(band).is_err() {
                        break;
                    }
                }
            };
            match pool {
                Some(pool) => pool.install(make),
                None => make(),
            }
        });
        let mut bands = rx.iter().inspect(|band| progress.advance(band.len() as u64));
        take(width, height, &mut bands)
    }))
}

/// Writes a `width × height` PNG with transparency, at 8 or (`sixteen`) 16 bits a channel,
/// from `bands` of rows, top to bottom, recording `ppi` pixels per inch if given.
fn write_png(
    out: impl Write,
    width: u32,
    height: u32,
    bands: impl Iterator<Item = Vec<Pixel>>,
    sixteen: bool,
    ppi: Option<f64>,
) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(out, width, height);
    encoder.set_pixel_dims(ppi.map(crate::density::png_dims));
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(if sixteen { png::BitDepth::Sixteen } else { png::BitDepth::Eight });
    encoder.set_compression(png::Compression::Balanced);
    encoder.set_filter(png::Filter::Adaptive);
    let mut writer = encoder.write_header()?;
    let mut stream = writer.stream_writer()?;
    for band in bands {
        let bytes: Vec<u8> = if sixteen {
            pixels_to_rgba16(&band).iter().flat_map(|v| v.to_be_bytes()).collect()
        } else {
            pixels_to_rgba8(&band)
        };
        stream.write_all(&bytes)?;
    }
    stream.finish()?;
    writer.finish()
}

/// Bands of rows as the JPEG encoder reads them: 8-row blocks from the top, each pixel
/// once, over an opaque background. Rows are taken from the bands as the encoder gets to
/// them, and dropped once it has gone past.
struct JpegRows<I> {
    bands: RefCell<I>,
    width: u32,
    height: u32,
    background: Pixel,
    /// The rows held: from row `.0`, `width` pixels each.
    held: RefCell<(u32, Vec<[u8; 3]>)>,
}

impl<I: Iterator<Item = Vec<Pixel>>> JpegRows<I> {
    fn new(bands: I, width: u32, height: u32, background: Pixel) -> JpegRows<I> {
        JpegRows { bands: RefCell::new(bands), width, height, background, held: RefCell::new((0, Vec::new())) }
    }
}

impl<I: Iterator<Item = Vec<Pixel>>> GenericImageView for JpegRows<I> {
    type Pixel = image::Rgb<u8>;

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn get_pixel(&self, x: u32, y: u32) -> image::Rgb<u8> {
        let w = self.width as usize;
        let mut held = self.held.borrow_mut();
        let (first, px) = &mut *held;
        // `encode_image` promises to read each pixel once, in 8 × 8 blocks from the top.
        assert!(y >= *first, "the JPEG encoder went back to row {y}, which it had gone past");
        while y >= *first + (px.len() / w) as u32 {
            // The encoder never goes back past the 8-row block it is in; 16 rows are kept, so
            // that blocks read in pairs of rows (as 4:2:0 subsampling would) work too.
            let keep = (y / 16 * 16).max(*first);
            px.drain(..(keep - *first) as usize * w);
            *first = keep;
            let band = self.bands.borrow_mut().next().expect("a row within the image");
            let bg = self.background;
            let rgb: Vec<[u8; 3]> = band
                .par_iter()
                .map(|p| {
                    let [r, g, b, _] = crate::pixels::to_srgb8(crate::pixels::over_opaque(*p, bg));
                    [r, g, b]
                })
                .collect();
            px.extend(rgb);
        }
        image::Rgb(px[(y - *first) as usize * w + x as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::png::PngEncoder;
    use opal_core::document::{IdGen, Layer, Position};

    /// The whole exported image at once, with its width and height.
    fn flatten_canvas(doc: &Document, size: Option<&ExportSize>) -> (Vec<Pixel>, u32, u32) {
        let mut rows = Exported::new(doc, size, &Progress::default()).expect("not cancelled");
        let (w, h) = (rows.width, rows.height);
        let mut out = Vec::new();
        while let Some(band) = rows.band() {
            out.extend(band);
        }
        (out, w, h)
    }

    /// A file `name` in a temporary folder the tests share (each test removes its files).
    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("opal-io-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn doc_from(imported: Imported) -> Document {
        let mut ids = IdGen::default();
        let mut d = Document::new(imported.width, imported.height);
        let layer = Layer::new_raster(ids.layer(), ids.object(), "Background", imported.image);
        d.insert(Position { parent: None, index: 0 }, layer).unwrap();
        d
    }

    fn gradient_png(w: u32, h: u32) -> Vec<u8> {
        let img =
            image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 7) as u8, (y * 3) as u8, 200, (x + y) as u8]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img).write_with_encoder(PngEncoder::new(&mut bytes)).unwrap();
        bytes
    }

    #[test]
    fn an_eight_bit_image_opens_at_eight_bits_and_a_sixteen_bit_one_as_working_pixels() {
        let eight = import_bytes(&gradient_png(300, 260), Path::new("8.png"), None).unwrap();
        assert!(eight.image.tiles().all(|(_, t)| t.is_srgb8()));
        // What it reads as is what converting each pixel gives.
        let img = image::load_from_memory(&gradient_png(300, 260)).unwrap().to_rgba8();
        let whole = IRect::from_size(300, 260);
        let want: Vec<Pixel> = img.pixels().map(|p| from_srgb8(p.0)).collect();
        assert!(eight.image.read_rect(&whole) == want);
        let px = vec![opal_core::tile::pixel_from_f32([0.3, 0.6, 0.9, 1.0]); 300 * 260];
        let sixteen = import_bytes(&encode_png16(&px, 300, 260, None), Path::new("16.png"), None).unwrap();
        assert!(sixteen.image.tiles().all(|(_, t)| !t.is_srgb8()), "16 bits kept at 8");
    }

    #[test]
    fn png16_for_the_clipboard_keeps_16_bits() {
        use opal_core::tile::pixel_from_f32;
        // A value between two 8-bit levels survives.
        let px = vec![pixel_from_f32([0.30001, 0.5, 0.7, 1.0]), pixel_from_f32([0.0, 0.0, 0.0, 0.25])];
        let bytes = encode_png16(&px, 2, 1, Some(300.0));
        let back = import_bytes(&bytes, Path::new("clip.png"), None).unwrap();
        // And the resolution of the image it came from.
        assert_eq!(back.resolution, Some([300.0, 300.0]));
        let p = back.image.pixel(0, 0);
        let expect = crate::pixels::to_srgb16(px[0]);
        assert_eq!(crate::pixels::to_srgb16(p), expect);
        assert_eq!(crate::pixels::to_srgb16(back.image.pixel(1, 0))[3], crate::pixels::to_srgb16(px[1])[3]);
    }

    #[test]
    fn png_round_trips_exactly() {
        let src = gradient_png(37, 300);
        let imported = import_bytes(&src, Path::new("test.png"), None).unwrap();
        assert_eq!((imported.width, imported.height), (37, 300));
        assert!(imported.warnings.is_empty());
        let path = temp("round.png");
        export(&doc_from(imported), &path, &ExportFormat::Png { sixteen_bit: false }, None).unwrap();
        let back = image::open(&path).unwrap().to_rgba8();
        std::fs::remove_file(&path).unwrap();
        let orig = image::load_from_memory(&src).unwrap().to_rgba8();
        for (a, b) in back.pixels().zip(orig.pixels()) {
            // Fully transparent pixels lose their (invisible) color, by design.
            if b[3] == 0 {
                assert_eq!(a[3], 0);
            } else {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn the_resolution_is_written_on_export_and_read_on_import() {
        let mut d = doc_from(import_bytes(&gradient_png(4, 4), Path::new("t.png"), None).unwrap());
        // A PNG without a resolution says nothing; the document then has the default.
        assert_eq!(import_bytes(&gradient_png(4, 4), Path::new("t.png"), None).unwrap().resolution, None);
        for (ppi, format) in [
            (300.0, ExportFormat::Png { sixteen_bit: false }),
            (72.0, ExportFormat::Png { sixteen_bit: true }),
            (300.0, ExportFormat::Jpeg { quality: 90, background: [0; 3] }),
            // 118 pixels per centimeter.
            (299.72, ExportFormat::Jpeg { quality: 90, background: [0; 3] }),
        ] {
            d.resolution = ppi;
            let path = temp("ppi.img");
            export(&d, &path, &format, None).unwrap();
            let back = import(&path, None).unwrap();
            std::fs::remove_file(&path).unwrap();
            let [read, _] = back.resolution.unwrap();
            assert!((read - ppi).abs() < 1e-9, "{format:?} at {ppi} read back as {read}");
        }
    }

    #[test]
    fn a_turned_image_s_resolution_across_is_its_file_s_down() {
        use image::codecs::jpeg::{PixelDensity, PixelDensityUnit};
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 90);
        encoder.set_pixel_density(PixelDensity { density: (100, 200), unit: PixelDensityUnit::Inches });
        DynamicImage::ImageRgb8(image::RgbImage::new(16, 8)).write_with_encoder(encoder).unwrap();
        let level = import_bytes(&jpeg, Path::new("level.jpg"), None).unwrap();
        assert_eq!(level.resolution, Some([100.0, 200.0]));
        let turned = import_bytes(&insert_exif_orientation(&jpeg, 6), Path::new("turned.jpg"), None).unwrap();
        assert_eq!(turned.resolution, Some([200.0, 100.0]));
    }

    #[test]
    fn jpeg_fills_transparency_with_the_background() {
        let mut ids = IdGen::default();
        let mut d = Document::new(8, 8);
        d.insert(Position { parent: None, index: 0 }, Layer::new_empty(ids.layer(), "empty")).unwrap();
        let path = temp("bg.jpg");
        export(&d, &path, &ExportFormat::Jpeg { quality: 95, background: [255, 0, 0] }, None).unwrap();
        let px = image::open(&path).unwrap().to_rgb8();
        std::fs::remove_file(&path).unwrap();
        let p = px.get_pixel(4, 4);
        assert!(p[0] > 240 && p[1] < 20 && p[2] < 20, "{p:?}");
    }

    #[test]
    fn exports_at_a_chosen_size_cover_the_canvas_wherever_it_lies() {
        use opal_core::tile::{pixel_from_f32, pixel_to_f32};
        let mut ids = IdGen::default();
        // A 40×20 canvas at (100, 50): its left half red, its right half blue, and green
        // outside it, which must not show.
        let mut img = RasterImage::new();
        for y in 40..80 {
            for x in 90..150 {
                let c = if !(100..140).contains(&x) || !(50..70).contains(&y) {
                    [0.0, 1.0, 0.0, 1.0]
                } else if x < 120 {
                    [1.0, 0.0, 0.0, 1.0]
                } else {
                    [0.0, 0.0, 1.0, 1.0]
                };
                img.set_pixel(x, y, pixel_from_f32(c));
            }
        }
        let mut d = Document::new(40, 20);
        d.canvas.x = 100;
        d.canvas.y = 50;
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img)).unwrap();
        let size = ExportSize { width: 10, height: 5, sampling: Sampling::default() };
        let (px, w, h) = flatten_canvas(&d, Some(&size));
        assert_eq!((w, h, px.len()), (10, 5, 50));
        let at = |x: usize, y: usize| pixel_to_f32(px[y * 10 + x]);
        assert!(at(1, 2)[0] > 0.99 && at(8, 2)[2] > 0.99, "red left, blue right");
        // The edges are the canvas's own pixels, not the green beyond it, and not faded.
        assert!(at(0, 0)[1] < 0.01 && at(9, 4)[1] < 0.01, "{:?} {:?}", at(0, 0), at(9, 4));
        assert!(at(0, 0)[3] > 0.99 && at(9, 4)[3] > 0.99, "{:?} {:?}", at(0, 0), at(9, 4));
        let (same, w, h) = flatten_canvas(&d, None);
        assert_eq!((w, h), (40, 20));
        assert_eq!(pixel_to_f32(same[0]), [1.0, 0.0, 0.0, 1.0]);
        // Shrunk a lot: averaged in blocks first, with the same clean edges.
        let tiny = ExportSize { width: 2, height: 1, sampling: Sampling::default() };
        let (px, w, h) = flatten_canvas(&d, Some(&tiny));
        assert_eq!((w, h), (2, 1));
        let (l, r) = (pixel_to_f32(px[0]), pixel_to_f32(px[1]));
        assert!(l[0] > 0.95 && l[1] < 0.01 && l[3] > 0.99, "{l:?}");
        assert!(r[2] > 0.95 && r[1] < 0.01 && r[3] > 0.99, "{r:?}");
    }

    #[test]
    fn block_shrinking_averages_partial_blocks_over_what_they_hold() {
        use opal_core::tile::{pixel_from_f32, pixel_to_f32};
        // 3 × 1: two pixels of 0 and one of 1, in blocks of 2: [0, 0] and [1].
        let px: Vec<Pixel> = [0.0, 0.0, 1.0].iter().map(|&v| pixel_from_f32([v, v, v, 1.0])).collect();
        let mut ids = IdGen::default();
        let mut d = Document::new(3, 1);
        let layer = Layer::new_raster(ids.layer(), ids.object(), "l", RasterImage::from_pixels(3, 1, &px));
        d.insert(Position { parent: None, index: 0 }, layer).unwrap();
        let (out, w, h) = shrunk(&d.layers, &d.canvas.rect(), 2, 1, &Progress::default()).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(pixel_to_f32(out.pixel(0, 0))[0], 0.0);
        assert_eq!(pixel_to_f32(out.pixel(1, 0))[0], 1.0);
    }

    #[test]
    fn exif_orientation_is_applied() {
        // A 2×1 JPEG, red on the left, carrying EXIF orientation 6 (rotate 90° clockwise)
        // comes out 1×2 with red on top.
        let img =
            image::RgbImage::from_fn(
                16,
                8,
                |x, _| if x < 8 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) },
            );
        let mut jpeg = Vec::new();
        DynamicImage::ImageRgb8(img).write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, 95)).unwrap();
        let with_exif = insert_exif_orientation(&jpeg, 6);
        let imported = import_bytes(&with_exif, Path::new("rotated.jpg"), None).unwrap();
        assert_eq!((imported.width, imported.height), (8, 16));
        let top = crate::pixels::to_srgb8(imported.image.pixel(4, 2));
        let bottom = crate::pixels::to_srgb8(imported.image.pixel(4, 13));
        assert!(top[0] > 200 && top[2] < 60, "top {top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 60, "bottom {bottom:?}");
    }

    /// Inserts an APP1 EXIF segment holding only an orientation tag after the SOI marker.
    fn insert_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes()); // offset of IFD0
        tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_le_bytes()); // count
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend_from_slice(&tiff);
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((seg.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&seg);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn non_images_are_refused_with_a_message() {
        let err = import_bytes(b"definitely not an image", Path::new("x.txt"), None).err().unwrap();
        assert!(err.to_string().contains("not a PNG or JPEG"), "{err}");
    }

    #[test]
    fn srgb_profiles_are_recognized() {
        assert!(looks_like_srgb(b"....desc....sRGB IEC61966-2.1"));
        assert!(!looks_like_srgb(b"....desc....Display P3"));
    }

    /// A document whose canvas does not start on a tile row and spans several bands, with
    /// a pattern that differs from pixel to pixel (and transparency).
    fn tall_document() -> Document {
        use opal_core::tile::pixel_from_f32;
        let mut ids = IdGen::default();
        let (w, h) = (300u32, 700u32);
        let px: Vec<Pixel> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let a = 0.5 + 0.5 * ((x + y) / 1000.0);
                pixel_from_f32([a * x / 300.0, a * y / 700.0, a * 0.3, a])
            })
            .collect();
        let mut img = RasterImage::new();
        img.write_rect(&IRect::new(20, 100, 320, 800), &px);
        let mut d = Document::new(w, h);
        d.canvas.x = 20;
        d.canvas.y = 100;
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img)).unwrap();
        d
    }

    #[test]
    fn png_exports_made_in_bands_hold_the_whole_canvas() {
        let d = tall_document();
        let whole = flatten(&d.layers, &d.canvas.rect());
        for sixteen_bit in [false, true] {
            let path = temp(&format!("bands{sixteen_bit}.png"));
            export(&d, &path, &ExportFormat::Png { sixteen_bit }, None).unwrap();
            let back = image::open(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert_eq!((back.width(), back.height()), (300, 700));
            if sixteen_bit {
                assert_eq!(back.to_rgba16().into_raw(), pixels_to_rgba16(&whole));
            } else {
                assert_eq!(back.to_rgba8().into_raw(), pixels_to_rgba8(&whole));
            }
        }
    }

    #[test]
    fn jpeg_exports_made_in_bands_match_one_encoded_whole() {
        let d = tall_document();
        let bg = from_srgb8([10, 200, 30, 255]);
        let whole: Vec<u8> = flatten(&d.layers, &d.canvas.rect())
            .iter()
            .flat_map(|p| {
                let [r, g, b, _] = crate::pixels::to_srgb8(crate::pixels::over_opaque(*p, bg));
                [r, g, b]
            })
            .collect();
        let mut expected = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut expected, 90);
        encoder.set_pixel_density(crate::density::jpeg_density(d.resolution));
        encoder.encode(&whole, 300, 700, image::ExtendedColorType::Rgb8).unwrap();
        let path = temp("bands.jpg");
        export(&d, &path, &ExportFormat::Jpeg { quality: 90, background: [10, 200, 30] }, None).unwrap();
        let got = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(got == expected, "the banded JPEG differs from the one encoded whole");
    }

    #[test]
    fn resized_exports_made_in_bands_match_one_drawn_whole() {
        let d = tall_document();
        for (w, h) in [(450, 1100), (200, 500), (40, 2)] {
            let size = ExportSize { width: w, height: h, sampling: Sampling::default() };
            let (banded, ..) = flatten_canvas(&d, Some(&size));
            let Source::Resized { image, to_size, sampling } =
                resized(&d.layers, &d.canvas.rect(), &size, &Progress::default()).unwrap()
            else {
                panic!("a resized source")
            };
            let out = IRect::from_size(i64::from(w), i64::from(h));
            let whole = resample::render_image(&image, &to_size, &sampling, Some(&out)).read_rect(&out);
            assert!(banded == whole, "{w} × {h}");
        }
    }

    #[test]
    fn blocks_are_averaged_whole_across_bands() {
        let d = tall_document();
        // 700 rows from row 100: bands of 156, 256, 256 and 32 rows. Blocks of 300 rows (two
        // whole and one of 100, each across bands), and of 3 and 7 (some across band edges).
        for (kx, ky) in [(7, 300), (3, 3), (5, 7)] {
            blocks_average(&d, kx, ky);
        }
    }

    fn blocks_average(d: &Document, kx: usize, ky: usize) {
        use opal_core::tile::pixel_to_f32;
        let (out, sw, sh) = shrunk(&d.layers, &d.canvas.rect(), kx as u32, ky as u32, &Progress::default()).unwrap();
        assert_eq!((sw as usize, sh as usize), (300usize.div_ceil(kx), 700usize.div_ceil(ky)));
        let whole = flatten(&d.layers, &d.canvas.rect());
        let (last_x, last_y) = (sw as usize - 1, sh as usize - 1);
        for (i, j) in [(0, 0), (sw as usize / 2, sh as usize / 2), (last_x, last_y), (1, 156 / ky), (2, 412 / ky)] {
            let (x0, x1, y0, y1) = (i * kx, ((i + 1) * kx).min(300), j * ky, ((j + 1) * ky).min(700));
            let mut sum = [0.0f64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = pixel_to_f32(whole[y * 300 + x]);
                    (0..4).for_each(|c| sum[c] += f64::from(p[c]));
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let got = pixel_to_f32(out.pixel(i as i64, j as i64));
            for c in 0..4 {
                assert!((f64::from(got[c]) - sum[c] / n).abs() < 2e-3, "{kx} × {ky}, block ({i}, {j}): {got:?}");
            }
        }
    }

    #[test]
    fn an_export_in_a_pool_composites_there() {
        use std::sync::{Condvar, Mutex, mpsc};
        use std::time::Duration;
        // Every thread of rayon's global pool held while exporting in another.
        let n = rayon::current_num_threads();
        let (started, all_started) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        for _ in 0..n {
            let (started, release) = (started.clone(), release.clone());
            rayon::spawn(move || {
                started.send(()).unwrap();
                let (m, c) = &*release;
                let mut go = m.lock().unwrap();
                while !*go {
                    go = c.wait(go).unwrap();
                }
            });
        }
        for _ in 0..n {
            all_started.recv().unwrap();
        }
        let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
        let path = temp("in-a-pool.png");
        let (tx, rx) = mpsc::channel();
        let (p, at) = (pool.clone(), path.clone());
        std::thread::spawn(move || {
            let d = tall_document();
            let png = ExportFormat::Png { sixteen_bit: false };
            let r = p.install(|| export_in(&d, &at, &png, None, Some(&p), &Progress::default()));
            let _ = tx.send(r.is_ok());
        });
        let done = rx.recv_timeout(Duration::from_secs(10));
        let (m, c) = &*release;
        *m.lock().unwrap() = true;
        c.notify_all();
        assert_eq!(done, Ok(true), "the export waited for the global pool");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_image_needing_more_memory_than_is_free_is_refused_before_decoding() {
        let png = gradient_png(100, 50);
        // 5,000 pixels at 8 bits: in one go the file and 20 KB decoded; a whole tile, kept
        // at 8 bits, of tiles.
        let (file, tile) = (png.len() as u64, (TILE_SIZE * TILE_SIZE * 4) as u64);
        let memory = |bytes| Some(Free::memory(bytes));
        let r = import_bytes(&png, Path::new("big.png"), memory(30_000));
        let Err(IoError::TooLarge { short, .. }) = r else { panic!("not refused: {:?}", r.map(|i| i.width)) };
        assert_eq!((short.need, short.have), (file + 20_000 + tile, 30_000));
        assert!(import_bytes(&png, Path::new("big.png"), memory(file + 20_000 + tile)).is_ok());
        assert!(import_bytes(&png, Path::new("big.png"), None).is_ok());
        // The tiles may go to disk; what is taken in one go may not.
        let with_disk = |memory| Some(Free { memory, reclaimable: 0, disk: 1 << 30, resident: 0 });
        assert!(import_bytes(&png, Path::new("big.png"), with_disk(file + 20_000)).is_ok());
        let r = import_bytes(&png, Path::new("big.png"), with_disk(file + 19_999));
        assert!(matches!(r, Err(IoError::TooLarge { short, .. }) if short.need == file + 20_000), "{:?}", r.err());
        // A thin image takes whole tiles; one converted whole, a copy in one go.
        let upright = image::metadata::Orientation::NoTransforms;
        let thin = import_memory((300_000, 1), 1_200_000, upright, Depth::Eight);
        assert_eq!(thin, Need { at_once: 1_200_000, tiles: 1172 * tile });
        let float = import_memory((256, 256), 1 << 20, upright, Depth::Other);
        assert_eq!(float, Need { at_once: (1 << 20) + 256 * 256 * 8, tiles: 2 * tile });
        // A size no sum of which may wrap past the check.
        let huge =
            import_memory((u32::MAX, u32::MAX), u64::MAX - 1, image::metadata::Orientation::Rotate90, Depth::Sixteen);
        assert_eq!((huge.at_once, huge.total()), (u64::MAX, u64::MAX));
    }

    #[test]
    fn every_kind_of_decoded_image_becomes_the_pixels_converting_it_whole_gives() {
        use image::{ImageBuffer, Luma, LumaA, Rgb, Rgba};
        // 3 × 2 whole tiles: the first column clear where the image has alpha, the last
        // flat, the middle each pixel its own.
        let (w, h) = (768u32, 512u32);
        let v = |x: u32, y: u32, k: u32| if x >= 512 { 77 } else { ((x * 7 + y * 13 + k * 101) % 256) as u8 };
        let a = |x: u32, y: u32| {
            if x < 256 {
                0
            } else if x >= 512 {
                255
            } else {
                v(x, y, 3)
            }
        };
        let wide = |b: u8| u16::from(b) * 251 + 3;
        let images = [
            DynamicImage::ImageLuma8(ImageBuffer::from_fn(w, h, |x, y| Luma([v(x, y, 0)]))),
            DynamicImage::ImageLumaA8(ImageBuffer::from_fn(w, h, |x, y| LumaA([v(x, y, 0), a(x, y)]))),
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |x, y| Rgb([v(x, y, 0), v(x, y, 1), 9]))),
            DynamicImage::ImageRgba8(ImageBuffer::from_fn(w, h, |x, y| Rgba([v(x, y, 0), 4, v(x, y, 2), a(x, y)]))),
            DynamicImage::ImageLuma16(ImageBuffer::from_fn(w, h, |x, y| Luma([wide(v(x, y, 0))]))),
            DynamicImage::ImageLumaA16(ImageBuffer::from_fn(w, h, |x, y| {
                LumaA([wide(v(x, y, 0)), u16::from(a(x, y)) * 257])
            })),
            DynamicImage::ImageRgb16(ImageBuffer::from_fn(w, h, |x, y| Rgb([wide(v(x, y, 0)), 700, 65535]))),
            DynamicImage::ImageRgba16(ImageBuffer::from_fn(w, h, |x, y| {
                Rgba([wide(v(x, y, 1)), 5, wide(v(x, y, 2)), u16::from(a(x, y)) * 257])
            })),
        ];
        let all = IRect::from_size(i64::from(w), i64::from(h));
        for img in images {
            let what = img.color();
            let got = image_of(&img);
            // Made another way: the pixels converted whole, written in.
            let px: Vec<Pixel> = match Depth::of(what) {
                Depth::Eight => img.to_rgba8().pixels().map(|p| from_srgb8(p.0)).collect(),
                _ => img.to_rgba16().pixels().map(|p| from_srgb16(p.0)).collect(),
            };
            let mut want = RasterImage::new();
            want.write_rect(&all, &px);
            assert!(got.read_rect(&all) == px, "{what:?} read otherwise");
            let coords = |i: &RasterImage| i.tiles().map(|(c, _)| c).collect::<std::collections::HashSet<_>>();
            assert_eq!(coords(&got), coords(&want), "{what:?}: clear tiles left out, and only those");
            assert_eq!(got.tile_count(), if what.has_alpha() { 4 } else { 6 }, "{what:?}");
            let at = |x, y| got.tile(opal_core::tile::TileCoord::new(x, y)).unwrap();
            let (top, bottom) = (at(2, 0), at(2, 1));
            assert!(Arc::ptr_eq(top, bottom), "{what:?}: flat tiles of one value are one");
            assert_eq!(got.read_rect_srgb8(&all).is_some(), Depth::of(what) == Depth::Eight, "{what:?} kept so");
        }
    }

    #[test]
    fn a_jpeg_too_large_for_the_format_is_refused_before_anything_is_made() {
        let d = Document::new(70_000, 10);
        let path = temp("too-wide.jpg");
        let jpeg = ExportFormat::Jpeg { quality: 90, background: [255; 3] };
        let p = Progress::default();
        let r = export_in(&d, &path, &jpeg, None, None, &p);
        assert!(matches!(&r, Err(IoError::Unsupported { message, .. }) if message.contains("65535")), "{r:?}");
        assert_eq!(p.fraction(), None, "begun all the same");
        assert!(!path.exists());
        // At a size within the limit, it is written.
        let size = ExportSize { width: 7000, height: 1, sampling: Sampling::default() };
        export_in(&d, &path, &jpeg, Some(&size), None, &p).unwrap();
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_enlarged_export_composites_tiles_alike_once_across_the_canvas() {
        let mut ids = IdGen::default();
        let mut d = Document::new(2048, 2048);
        let img =
            RasterImage::filled(&IRect::from_size(2048, 2048), opal_core::tile::pixel_from_f32([0.2, 0.4, 0.6, 1.0]));
        let l = ids.layer();
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(l, ids.object(), "l", img)).unwrap();
        d.layer_mut(l).unwrap().opacity = 0.5;
        let size = ExportSize { width: 3000, height: 3000, sampling: Sampling::default() };
        let Some(Source::Resized { image, .. }) = resized(&d.layers, &d.canvas.rect(), &size, &Progress::default())
        else {
            panic!("not resized")
        };
        // The canvas's 64 tiles are one composite (its edges are copied into tiles past it).
        let inside: std::collections::HashSet<*const opal_core::tile::Tile> = image
            .tiles()
            .filter(|(c, _)| (0..8).contains(&c.x) && (0..8).contains(&c.y))
            .map(|(_, t)| Arc::as_ptr(t))
            .collect();
        assert_eq!(inside.len(), 1);
    }

    #[test]
    fn an_export_takes_its_bands_and_what_it_is_drawn_from() {
        use opal_core::raster::TILE_BYTES;
        let band = |w: u64| w * 256 * 8;
        let size = |width, height| ExportSize { width, height, sampling: Sampling::default() };
        let mut ids = IdGen::default();
        let mut d = Document::new(3000, 2000);
        assert_eq!(export_memory(&d, None), Need::at_once(5 * band(3000)));
        assert_eq!(export_memory(&d, Some(&size(3000, 2000))), Need::at_once(5 * band(3000)), "the canvas's own size");
        // Halved: averaged into 1500 × 1000 (6 × 4 tiles), from bands of the canvas; its
        // edges copied 5 pixels out (Bicubic reaches 4) into the tiles past the 5 × 3
        // within it, of the 7 × 5 the copies reach.
        let halved = Need { at_once: 5 * band(1500) + 2 * band(3000), tiles: 24 * TILE_BYTES + (35 - 15) * TILE_BYTES };
        assert_eq!(export_memory(&d, Some(&size(1500, 1000))), halved);
        // Enlarged: drawn from the canvas composited, here one tile made; its edges copied
        // into the tiles past the 11 × 7 within it, of the 13 × 9 the copies reach.
        let mut img = RasterImage::new();
        img.set_pixel(5, 5, opal_core::tile::pixel_from_f32([1.0; 4]));
        let l = ids.layer();
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(l, ids.object(), "l", img)).unwrap();
        d.layer_mut(l).unwrap().opacity = 0.5;
        let enlarged = Need { at_once: 5 * band(4000), tiles: TILE_BYTES + (117 - 77) * TILE_BYTES };
        assert_eq!(export_memory(&d, Some(&size(4000, 3000))), enlarged);
    }

    /// The exports to try progress on: at the canvas's size, shrunk (by block averages), and
    /// enlarged (from the canvas composited whole), in each format.
    fn kinds_of_export() -> Vec<(ExportFormat, Option<ExportSize>)> {
        let png = ExportFormat::Png { sixteen_bit: false };
        let jpeg = ExportFormat::Jpeg { quality: 90, background: [255; 3] };
        let shrink = ExportSize { width: 100, height: 200, sampling: Sampling::default() };
        let enlarge = ExportSize { width: 450, height: 800, sampling: Sampling::default() };
        vec![
            (png.clone(), None),
            (jpeg.clone(), None),
            (png.clone(), Some(shrink)),
            (jpeg, Some(shrink)),
            (png, Some(enlarge)),
        ]
    }

    #[test]
    fn an_export_counts_all_its_work_once() {
        use std::sync::atomic::Ordering::Relaxed;
        let d = tall_document();
        let path = temp("counted.png");
        for (format, size) in kinds_of_export() {
            let p = Progress::default();
            export_in(&d, &path, &format, size.as_ref(), None, &p).unwrap();
            let (done, total) = (p.done.load(Relaxed), p.total.load(Relaxed));
            assert!(total > 0 && done == total, "{format:?} at {size:?}: {done} of {total}");
            assert_eq!(p.fraction(), Some(1.0));
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_cancelled_export_leaves_the_file_as_it_was() {
        let d = tall_document();
        let path = temp("cancelled.png");
        std::fs::write(&path, b"before").unwrap();
        for (format, size) in kinds_of_export() {
            let p = Progress::default();
            p.cancel();
            let r = export_in(&d, &path, &format, size.as_ref(), None, &p);
            assert!(matches!(r, Err(IoError::Cancelled { .. })), "{format:?} at {size:?}: {r:?}");
            assert_eq!(std::fs::read(&path).unwrap(), b"before");
        }
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".cancelled.png"))
            .collect();
        std::fs::remove_file(&path).unwrap();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn a_resized_export_stops_compositing_once_cancelled() {
        let d = tall_document();
        for (_, size) in kinds_of_export() {
            let p = Progress::default();
            p.cancel();
            assert!(Exported::new(&d, size.as_ref(), &p).is_none() == size.is_some(), "{size:?}");
        }
    }
}
