//! Importing and exporting ordinary image files (PNG and JPEG).

use crate::pixels::{from_srgb8, pixels_from_rgba8, pixels_from_rgba16, pixels_to_rgba8, pixels_to_rgba16};
use crate::{IoError, write_atomically};
use half::f16;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{DynamicImage, ImageDecoder, ImageReader};
use opal_core::composite::flatten;
use opal_core::document::Document;
use opal_core::geom::{Affine, IRect};
use opal_core::raster::RasterImage;
use opal_core::resample::{self, Sampling};
use opal_core::tile::Pixel;
use rayon::prelude::*;
use std::io::Cursor;
use std::path::Path;

/// An image read from a file, converted to working pixels with its top-left corner at the
/// canvas origin.
pub struct Imported {
    pub width: u32,
    pub height: u32,
    pub image: RasterImage,
    /// Things the user should know about how the file was read.
    pub warnings: Vec<String>,
}

/// Reads a PNG or JPEG (recognized by content, not extension), turned upright according to
/// its EXIF orientation.
pub fn import(path: &Path) -> Result<Imported, IoError> {
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    import_bytes(&bytes, path)
}

/// Like [`import`], from bytes already in memory; `path` is used only in messages.
pub fn import_bytes(bytes: &[u8], path: &Path) -> Result<Imported, IoError> {
    let decode_err = |e: image::ImageError| IoError::Decode { path: path.to_owned(), message: e.to_string() };
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| IoError::io(path, e))?;
    if reader.format().is_none() {
        return Err(IoError::Unsupported { path: path.to_owned(), message: "not a PNG or JPEG image".into() });
    }
    let mut decoder = reader.into_decoder().map_err(decode_err)?;
    let orientation = decoder.orientation().map_err(decode_err)?;
    let icc = decoder.icc_profile().map_err(decode_err)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(decode_err)?;
    img.apply_orientation(orientation);

    let (width, height) = (img.width(), img.height());
    let sixteen_bit = matches!(
        img,
        DynamicImage::ImageLuma16(_)
            | DynamicImage::ImageLumaA16(_)
            | DynamicImage::ImageRgb16(_)
            | DynamicImage::ImageRgba16(_)
    );
    let pixels = if sixteen_bit {
        pixels_from_rgba16(img.to_rgba16().as_raw())
    } else {
        pixels_from_rgba8(img.to_rgba8().as_raw())
    };

    let mut warnings = Vec::new();
    if let Some(profile) = icc
        && !looks_like_srgb(&profile)
    {
        warnings.push(
            "This image has an embedded color profile other than sRGB. Until color management \
             arrives, its colors are read as sRGB and may look different from other programs."
                .to_owned(),
        );
    }
    Ok(Imported { width, height, image: RasterImage::from_pixels(width, height, &pixels), warnings })
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

/// A size to export at other than the canvas's, and how to resample to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSize {
    pub width: u32,
    pub height: u32,
    pub sampling: Sampling,
}

/// The canvas of `doc` flattened, at the canvas's size or at `size`: the pixels row by
/// row, with the width and height.
pub fn flatten_canvas(doc: &Document, size: Option<&ExportSize>) -> (Vec<Pixel>, u32, u32) {
    let canvas = doc.canvas.rect();
    let (w, h) = (doc.canvas.width, doc.canvas.height);
    let flat = flatten(&doc.layers, &canvas);
    if w == 0 || h == 0 {
        return (flat, w, h);
    }
    match size {
        Some(s) if (s.width, s.height) != (w, h) => {
            let to_size = Affine::scale(f64::from(s.width) / f64::from(w), f64::from(s.height) / f64::from(h));
            // The canvas is the whole image: what lies past it must not bleed into the
            // edges, and the edges must not fade out either. So the resampler gets the
            // canvas alone, framed by copies of its edge pixels as far as its filter reaches.
            let border = resample::reach(&to_size, &s.sampling).ceil() as u32 + 1;
            let framed = extend_edges(&flat, w, h, border);
            let source = RasterImage::from_pixels(w + 2 * border, h + 2 * border, &framed);
            let b = f64::from(border);
            let out = IRect::from_size(i64::from(s.width), i64::from(s.height));
            let resized =
                resample::render_image(&source, &Affine::translate(-b, -b).then(to_size), &s.sampling, Some(&out));
            (resized.read_rect(&out), s.width, s.height)
        }
        _ => (flat, w, h),
    }
}

/// `w × h` pixels with `border` more on every side, each a copy of the nearest edge pixel.
fn extend_edges(px: &[Pixel], w: u32, h: u32, border: u32) -> Vec<Pixel> {
    let (w, h, b) = (w as usize, h as usize, border as usize);
    let ew = w + 2 * b;
    let mut out = Vec::with_capacity(ew * (h + 2 * b));
    for y in 0..h + 2 * b {
        let row = &px[y.saturating_sub(b).min(h - 1) * w..][..w];
        out.extend(std::iter::repeat_n(row[0], b));
        out.extend_from_slice(row);
        out.extend(std::iter::repeat_n(row[w - 1], b));
    }
    out
}

/// Flattens the canvas of `doc`, at its own size or at `size`, and writes it to `path`
/// (atomically).
pub fn export(doc: &Document, path: &Path, format: &ExportFormat, size: Option<&ExportSize>) -> Result<(), IoError> {
    let (pixels, w, h) = flatten_canvas(doc, size);
    let encode_err = |e: image::ImageError| IoError::Encode { path: path.to_owned(), message: e.to_string() };
    let img = match format {
        ExportFormat::Png { sixteen_bit: false } => {
            DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, pixels_to_rgba8(&pixels)).expect("buffer size"))
        }
        ExportFormat::Png { sixteen_bit: true } => DynamicImage::ImageRgba16(
            image::ImageBuffer::from_raw(w, h, pixels_to_rgba16(&pixels)).expect("buffer size"),
        ),
        ExportFormat::Jpeg { background, .. } => {
            let bg = from_srgb8([background[0], background[1], background[2], 255]);
            let rgb: Vec<u8> = pixels
                .par_iter()
                .flat_map_iter(|p| {
                    let [r, g, b, _] = crate::pixels::to_srgb8(over_opaque(*p, bg));
                    [r, g, b]
                })
                .collect();
            DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, rgb).expect("buffer size"))
        }
    };
    write_atomically(path, |out| {
        match format {
            ExportFormat::Png { .. } => img.write_with_encoder(PngEncoder::new(out)),
            ExportFormat::Jpeg { quality, .. } => {
                img.write_with_encoder(JpegEncoder::new_with_quality(out, (*quality).clamp(1, 100)))
            }
        }
        .map_err(encode_err)
    })
}

/// `p` over the opaque pixel `bg`, both premultiplied.
fn over_opaque(p: Pixel, bg: Pixel) -> Pixel {
    let a = p[3].to_f32();
    let mix = |i: usize| f16::from_f32(p[i].to_f32() + bg[i].to_f32() * (1.0 - a));
    [mix(0), mix(1), mix(2), f16::ONE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::document::{IdGen, Layer, Position};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("opal-io-{}", std::process::id()));
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
    fn png_round_trips_exactly() {
        let src = gradient_png(37, 300);
        let imported = import_bytes(&src, Path::new("test.png")).unwrap();
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
        let imported = import_bytes(&with_exif, Path::new("rotated.jpg")).unwrap();
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
        let err = import_bytes(b"definitely not an image", Path::new("x.txt")).err().unwrap();
        assert!(err.to_string().contains("not a PNG or JPEG"), "{err}");
    }

    #[test]
    fn srgb_profiles_are_recognized() {
        assert!(looks_like_srgb(b"....desc....sRGB IEC61966-2.1"));
        assert!(!looks_like_srgb(b"....desc....Display P3"));
    }
}
