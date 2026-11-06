//! Conversions between the working pixel format (see `opal_core::tile`) and the
//! straight-alpha, sRGB-encoded integer pixels of ordinary image files.

use half::f16;
use opal_color::{srgb_decode, srgb_encode};
use opal_core::tile::Pixel;
use rayon::prelude::*;

fn premultiply(r: f32, g: f32, b: f32, a: f32) -> Pixel {
    if a <= 0.0 {
        return [f16::ZERO; 4];
    }
    [f16::from_f32(r * a), f16::from_f32(g * a), f16::from_f32(b * a), f16::from_f32(a)]
}

/// An 8-bit sRGB pixel with straight alpha, as PNG and JPEG store them (the conversion
/// tiles kept at 8 bits read through, so the two always agree).
pub fn from_srgb8(p: [u8; 4]) -> Pixel {
    opal_core::tile::pixel_from_srgb8(p)
}

/// A 16-bit sRGB pixel with straight alpha.
pub fn from_srgb16([r, g, b, a]: [u16; 4]) -> Pixel {
    let d = |v: u16| srgb_decode(f64::from(v) / 65535.0) as f32;
    premultiply(d(r), d(g), d(b), f32::from(a) / 65535.0)
}

/// The straight-alpha, sRGB-encoded color of a working pixel, each channel in 0–1. Colors
/// outside the sRGB gamut are clipped per channel.
pub fn to_srgb_unit(p: Pixel) -> [f32; 4] {
    let a = p[3].to_f32().clamp(0.0, 1.0);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let enc = |c: f16| srgb_encode(f64::from((c.to_f32() / a).clamp(0.0, 1.0))) as f32;
    [enc(p[0]), enc(p[1]), enc(p[2]), a]
}

/// `p` over the opaque pixel `bg`, both premultiplied: what shows where transparency is
/// flattened onto a background.
pub fn over_opaque(p: Pixel, bg: Pixel) -> Pixel {
    let a = p[3].to_f32();
    let mix = |i: usize| f16::from_f32(p[i].to_f32() + bg[i].to_f32() * (1.0 - a));
    [mix(0), mix(1), mix(2), f16::ONE]
}

pub fn to_srgb8(p: Pixel) -> [u8; 4] {
    to_srgb_unit(p).map(|v| (v * 255.0).round() as u8)
}

pub fn to_srgb16(p: Pixel) -> [u16; 4] {
    to_srgb_unit(p).map(|v| (v * 65535.0).round() as u16)
}

/// Converts a buffer of 16-bit RGBA values (row-major, straight alpha) in parallel.
pub fn pixels_from_rgba16(values: &[u16]) -> Vec<Pixel> {
    values.par_chunks_exact(4).map(|c| from_srgb16([c[0], c[1], c[2], c[3]])).collect()
}

/// Converts working pixels to 8-bit RGBA bytes in parallel.
pub fn pixels_to_rgba8(px: &[Pixel]) -> Vec<u8> {
    px.par_iter().flat_map_iter(|&p| to_srgb8(p)).collect()
}

/// Converts working pixels to 16-bit RGBA values in parallel.
pub fn pixels_to_rgba16(px: &[Pixel]) -> Vec<u16> {
    px.par_iter().flat_map_iter(|&p| to_srgb16(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_8_bit_value_round_trips() {
        for v in 0..=255u8 {
            for a in [255u8, 128] {
                let back = to_srgb8(from_srgb8([v, 255 - v, v / 2, a]));
                assert_eq!(back, [v, 255 - v, v / 2, a], "value {v}, alpha {a}");
            }
        }
    }

    #[test]
    fn sixteen_bit_values_round_trip_within_half_float_precision() {
        // Half floats keep about 11 significant bits; at the bright end of sRGB that is a
        // step of a few 16-bit units.
        for v in (0..=65535u16).step_by(97) {
            let back = to_srgb16(from_srgb16([v, v, v, 65535]));
            assert!((i32::from(back[0]) - i32::from(v)).abs() <= 40, "{v} came back as {}", back[0]);
        }
    }

    #[test]
    fn transparent_pixels_carry_no_color() {
        assert_eq!(from_srgb8([200, 100, 50, 0]), [f16::ZERO; 4]);
        assert_eq!(to_srgb8([f16::ZERO; 4]), [0, 0, 0, 0]);
    }
}
