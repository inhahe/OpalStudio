//! The resolution image files record: read from a PNG's `pHYs` chunk or a JPEG's JFIF or
//! EXIF header, and written back on export.
//!
//! The readers walk the file's headers themselves (the decoder does not report them), and
//! check every length and offset against the bytes there are: a damaged or hostile file
//! yields no resolution, never a panic.

use opal_core::units::valid_ppi;

/// Pixels per inch across and down, as a file records them: `None` if it records none, or
/// only the pixels' shape, or one out of range.
pub(crate) fn read(bytes: &[u8], format: image::ImageFormat) -> Option<[f64; 2]> {
    let ppi = match format {
        image::ImageFormat::Png => png(bytes),
        image::ImageFormat::Jpeg => jpeg(bytes),
        _ => None,
    }?;
    ppi.iter().all(|&p| valid_ppi(p)).then_some(ppi)
}

/// `n` bytes of `b` from `at`, if it has them.
fn field(b: &[u8], at: usize, n: usize) -> Option<&[u8]> {
    b.get(at..at.checked_add(n)?)
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(field(b, at, 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(field(b, at, 4)?.try_into().ok()?))
}

/// A PNG's `pHYs` chunk, which comes before the image data, in pixels per meter.
fn png(bytes: &[u8]) -> Option<[f64; 2]> {
    const PER_METER: f64 = 0.0254;
    let mut at = 8; // the signature
    loop {
        let len = be32(bytes, at)? as usize;
        let kind = field(bytes, at + 4, 4)?;
        if kind == b"IDAT" {
            return None;
        }
        let data = field(bytes, at + 8, len)?;
        if kind == b"pHYs" {
            // Unit 1 is the meter; 0 gives only the pixels' shape.
            if len != 9 || data[8] != 1 {
                return None;
            }
            let per_meter = |v: u32| tidy(f64::from(v) * PER_METER, PER_METER);
            return Some([per_meter(be32(data, 0)?), per_meter(be32(data, 4)?)]);
        }
        at = at.checked_add(12)?.checked_add(len)?;
    }
}

/// A JPEG's JFIF density (APP0) if it has a unit, else its EXIF resolution (APP1), from the
/// headers before the image data.
fn jpeg(bytes: &[u8]) -> Option<[f64; 2]> {
    if field(bytes, 0, 2)? != [0xFF, 0xD8] {
        return None;
    }
    let (mut jfif, mut exif) = (None, None);
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            break;
        }
        let code = *bytes.get(at + 1)?;
        match code {
            // Fill bytes before a marker.
            0xFF => {
                at += 1;
                continue;
            }
            // Markers without a segment.
            0x01 | 0xD0..=0xD7 => {
                at += 2;
                continue;
            }
            // The scan (the image data) or the end.
            0xDA | 0xD9 => break,
            _ => {}
        }
        let len = usize::from(be16(bytes, at + 2)?);
        let data = field(bytes, at + 4, len.checked_sub(2)?)?;
        match code {
            0xE0 if jfif.is_none() && data.starts_with(b"JFIF\0") => jfif = jfif_density(data),
            0xE1 if exif.is_none() && data.starts_with(b"Exif\0\0") => exif = exif_density(&data[6..]),
            _ => {}
        }
        at = at + 2 + len;
    }
    jfif.or(exif)
}

/// The density in a JFIF segment (from its identifier on), if it has a unit.
fn jfif_density(data: &[u8]) -> Option<[f64; 2]> {
    let per_inch = match *data.get(7)? {
        1 => 1.0,
        2 => 2.54,
        _ => return None,
    };
    let (x, y) = (be16(data, 8)?, be16(data, 10)?);
    // A density of 0 is no density: the EXIF block may still have one.
    (x > 0 && y > 0).then(|| [f64::from(x) * per_inch, f64::from(y) * per_inch])
}

/// The resolution in an EXIF block (a TIFF structure): IFD0's XResolution and
/// YResolution, in its ResolutionUnit (inches unless it says otherwise). As exact as
/// pixels per inch or per centimeter can be written, it is taken as it is.
fn exif_density(tiff: &[u8]) -> Option<[f64; 2]> {
    let big = match field(tiff, 0, 2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        field(tiff, at, 2)
            .map(|b| [b[0], b[1]])
            .map(|b| if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    };
    let u32_at = |at: usize| {
        field(tiff, at, 4)
            .map(|b| [b[0], b[1], b[2], b[3]])
            .map(|b| if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let (mut x, mut y, mut unit) = (None, None, 2);
    for i in 0..usize::from(u16_at(ifd)?) {
        let entry = ifd.checked_add(2 + i * 12)?;
        let (tag, kind) = (u16_at(entry)?, u16_at(entry + 2)?);
        // A RATIONAL (5) is two LONGs at the offset the entry holds.
        let rational = || {
            let at = u32_at(entry + 8)? as usize;
            let (n, d) = (u32_at(at)?, u32_at(at.checked_add(4)?)?);
            (d != 0).then(|| f64::from(n) / f64::from(d))
        };
        match (tag, kind) {
            (0x011A, 5) => x = rational(),
            (0x011B, 5) => y = rational(),
            // A SHORT (3) is held in the entry itself.
            (0x0128, 3) => unit = u16_at(entry + 8)?,
            _ => {}
        }
    }
    let per_inch = match unit {
        2 => 1.0,
        3 => 2.54,
        _ => return None,
    };
    let x = x?;
    Some([x * per_inch, y.unwrap_or(x) * per_inch])
}

/// The roundest resolution within `step` (how finely the file could record it) of `ppi`,
/// in pixels per inch or per centimeter, whichever is rounder (per inch if the same): what
/// the writer of a file recording another unit most likely meant, so that 2835 pixels per
/// meter reads as 72 ppi.
fn tidy(ppi: f64, step: f64) -> f64 {
    for decimals in 0..=3 {
        let scale = 10f64.powi(decimals);
        let round = |v: f64| (v * scale).round() / scale;
        // (Rounded to nothing, it is no resolution: a finer one is looked for.)
        let per_inch = round(ppi);
        if per_inch > 0.0 && (per_inch - ppi).abs() <= step {
            return per_inch;
        }
        let per_cm = round(ppi / 2.54) * 2.54;
        if per_cm > 0.0 && (per_cm - ppi).abs() <= step {
            return per_cm;
        }
    }
    ppi
}

/// `ppi` as a PNG's `pHYs` records it: pixels per meter.
pub(crate) fn png_dims(ppi: f64) -> png::PixelDimensions {
    let per_meter = (ppi / 0.0254).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    png::PixelDimensions { xppu: per_meter, yppu: per_meter, unit: png::Unit::Meter }
}

/// `ppi` as a JFIF header records it: whole pixels per inch or per centimeter, whichever
/// is nearer.
pub(crate) fn jpeg_density(ppi: f64) -> image::codecs::jpeg::PixelDensity {
    use image::codecs::jpeg::{PixelDensity, PixelDensityUnit};
    let whole = |v: f64| v.round().clamp(1.0, f64::from(u16::MAX));
    let (per_inch, per_cm) = (whole(ppi), whole(ppi / 2.54));
    let (density, unit) = if (per_inch - ppi).abs() <= (per_cm * 2.54 - ppi).abs() {
        (per_inch, PixelDensityUnit::Inches)
    } else {
        (per_cm, PixelDensityUnit::Centimeters)
    };
    PixelDensity { density: (density as u16, density as u16), unit }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_are_read_as_their_writers_meant_them() {
        // 72 ppi is 2834.6 pixels per meter, written 2835 (or 2834, truncated).
        assert_eq!(tidy(2835.0 * 0.0254, 0.0254), 72.0);
        assert_eq!(tidy(2834.0 * 0.0254, 0.0254), 72.0);
        assert_eq!(tidy(11811.0 * 0.0254, 0.0254), 300.0);
        // 118 pixels per centimeter stays that, not 299.7 ppi.
        assert!((tidy(11800.0 * 0.0254, 0.0254) / 2.54 - 118.0).abs() < 1e-9);
        // One pixel per meter is 0.0254 ppi, not none.
        assert_eq!(tidy(0.0254, 0.0254), 0.03);
    }

    #[test]
    fn jpeg_density_picks_the_nearer_unit() {
        use image::codecs::jpeg::PixelDensityUnit;
        let d = jpeg_density(300.0);
        assert_eq!((d.density, d.unit), ((300, 300), PixelDensityUnit::Inches));
        // 118 per cm is 299.72 ppi: nearer as 118 per cm than as 300 per inch.
        let d = jpeg_density(299.72);
        assert_eq!((d.density, d.unit), ((118, 118), PixelDensityUnit::Centimeters));
        assert_eq!(jpeg_density(0.01).density, (1, 1));
        assert_eq!(png_dims(72.0).xppu, 2835);
    }

    #[test]
    fn damaged_headers_give_no_resolution() {
        // Truncated anywhere, a PNG or JPEG with a resolution reads as having none or the
        // one it had, and never panics.
        let mut png = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png, 1, 1);
            e.set_color(png::ColorType::Rgb);
            e.set_pixel_dims(Some(png_dims(300.0)));
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[0, 0, 0]).unwrap();
        }
        assert_eq!(read(&png, image::ImageFormat::Png), Some([300.0, 300.0]));
        for n in 0..png.len() {
            let r = read(&png[..n], image::ImageFormat::Png);
            assert!(r.is_none() || r == Some([300.0, 300.0]));
        }
        // A chunk claiming more bytes than there are.
        let mut long = png.clone();
        long[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read(&long, image::ImageFormat::Png), None);

        let jpeg = exif_jpeg(&exif_resolution(b"II", 300, 1, 2));
        assert_eq!(read(&jpeg, image::ImageFormat::Jpeg), Some([300.0, 300.0]));
        for n in 0..jpeg.len() {
            let r = read(&jpeg[..n], image::ImageFormat::Jpeg);
            assert!(r.is_none() || r == Some([300.0, 300.0]));
        }
        // An IFD offset past the end, and a rational with no denominator.
        let mut far = exif_resolution(b"II", 300, 1, 2);
        far[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read(&exif_jpeg(&far), image::ImageFormat::Jpeg), None);
        assert_eq!(read(&exif_jpeg(&exif_resolution(b"II", 300, 0, 2)), image::ImageFormat::Jpeg), None);
    }

    #[test]
    fn exif_resolution_is_read_in_either_byte_order_and_unit() {
        let read_exif = |tiff: Vec<u8>| read(&exif_jpeg(&tiff), image::ImageFormat::Jpeg);
        assert_eq!(read_exif(exif_resolution(b"MM", 600, 2, 2)), Some([300.0, 300.0]));
        // 118 per centimeter.
        let cm = read_exif(exif_resolution(b"II", 118, 1, 3)).unwrap();
        assert!((cm[0] - 299.72).abs() < 1e-9, "{cm:?}");
        // No absolute unit.
        assert_eq!(read_exif(exif_resolution(b"II", 1, 1, 1)), None);
    }

    /// A TIFF structure in byte order `order` whose IFD0 holds XResolution `n / d` and
    /// ResolutionUnit `unit` (no YResolution: it is taken to be the same).
    fn exif_resolution(order: &[u8; 2], n: u32, d: u32, unit: u16) -> Vec<u8> {
        let big = order == b"MM";
        let w16 = |v: u16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let w32 = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut t = order.to_vec();
        t.extend_from_slice(&w16(42));
        t.extend_from_slice(&w32(8));
        t.extend_from_slice(&w16(2));
        // XResolution, RATIONAL, 1, at 38 (8 + 2 + 2 × 12 + 4).
        t.extend_from_slice(&w16(0x011A));
        t.extend_from_slice(&w16(5));
        t.extend_from_slice(&w32(1));
        t.extend_from_slice(&w32(38));
        // ResolutionUnit, SHORT, 1.
        t.extend_from_slice(&w16(0x0128));
        t.extend_from_slice(&w16(3));
        t.extend_from_slice(&w32(1));
        t.extend_from_slice(&w16(unit));
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&w32(0));
        t.extend_from_slice(&w32(n));
        t.extend_from_slice(&w32(d));
        t
    }

    /// The headers of a JPEG with an EXIF segment holding `tiff`, cut off at its scan.
    fn exif_jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend_from_slice(tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((seg.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&seg);
        out.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        out
    }
}
