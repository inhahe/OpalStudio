//! Matting (design.md, *Matting*): the edge colours of a layer cut out of one background and
//! put on another set right. Defringe gives edge pixels the colour of pixels further in;
//! Remove Black Matte and Remove White Matte take back out of partly transparent pixels the
//! colour they were blended with.
//!
//! Each works on a dense buffer of working pixels (linear light, premultiplied: see
//! `tile`), row by row, `width` pixels a row; what lies past the buffer counts as
//! transparent.

use crate::tile::Pixel;
use half::f16;
use opal_color::{srgb_decode, srgb_encode};
use std::collections::VecDeque;

/// Below this alpha a pixel is transparent: nothing of it shows.
const CLEAR: f32 = 1.0 / 512.0;

/// A pixel's straight colour (linear) and its alpha.
fn straight(p: Pixel) -> ([f32; 3], f32) {
    let a = p[3].to_f32();
    if a <= CLEAR {
        return ([0.0; 3], 0.0);
    }
    ([p[0].to_f32() / a, p[1].to_f32() / a, p[2].to_f32() / a], a)
}

/// The working pixel of straight colour `c` (linear) at alpha `a`.
fn premultiplied(c: [f32; 3], a: f32) -> Pixel {
    [f16::from_f32(c[0] * a), f16::from_f32(c[1] * a), f16::from_f32(c[2] * a), f16::from_f32(a)]
}

/// Takes `matte` (0, black; 1, white: an sRGB-encoded grey) back out of each partly
/// transparent pixel of `px`, as if it had been blended over it at its alpha: in sRGB terms,
/// as images are blended where mattes come from, `true = (seen - matte × (1 − a)) / a`.
/// Opaque and transparent pixels are left as they are.
pub fn remove_matte(px: &mut [Pixel], matte: f64) {
    for p in px {
        let (c, a) = straight(*p);
        if a <= CLEAR || a >= 1.0 {
            continue;
        }
        let a64 = f64::from(a);
        let unmatted = c.map(|v| {
            let seen = srgb_encode(f64::from(v).clamp(0.0, 1.0));
            srgb_decode(((seen - matte * (1.0 - a64)) / a64).clamp(0.0, 1.0)) as f32
        });
        *p = premultiplied(unmatted, a);
    }
}

/// Defringe: each pixel of `px` (`width` × `height`) that shows (its alpha above nought) and
/// lies within `reach` pixels of a transparent one (counted in steps, diagonals one step;
/// what lies past the buffer is transparent) given the colour of the pixels further in, its
/// alpha kept: ring by ring outwards from those, each pixel the mean of its neighbours given
/// a colour already, at most `reach` rings (so a pixel's colour comes from no further than
/// `2 × reach + 1` pixels round it, which a tile read with that margin keeps). Content no
/// wider than `2 × reach` has nothing further in, and is left as it is, but for what lies
/// within `reach` rings of content that has (the root of a thin strand).
pub fn defringe(px: &mut [Pixel], width: usize, height: usize, reach: u32) {
    assert_eq!(px.len(), width * height, "the buffer is not width × height");
    if reach == 0 || px.is_empty() {
        return;
    }
    let shows = |p: &Pixel| p[3].to_f32() > CLEAR;
    let neighbours = |i: usize| {
        let (x, y) = ((i % width) as i64, (i / width) as i64);
        (-1..=1i64)
            .flat_map(move |dy| (-1..=1i64).map(move |dx| (dx, dy)))
            .filter(|&d| d != (0, 0))
            .map(move |(dx, dy)| (x + dx, y + dy))
    };
    let inside = |(x, y): (i64, i64)| x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height;
    let index = |(x, y): (i64, i64)| y as usize * width + x as usize;
    // Steps from the nearest transparent pixel (or the buffer's edge), up to `reach + 1`.
    let far = reach + 1;
    let mut steps = vec![far; px.len()];
    let mut queue = VecDeque::new();
    for (i, p) in px.iter().enumerate() {
        let at_edge = neighbours(i).any(|n| !inside(n));
        if !shows(p) {
            steps[i] = 0;
            queue.push_back(i);
        } else if at_edge {
            steps[i] = 1;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let next = steps[i] + 1;
        if next > reach {
            continue;
        }
        for n in neighbours(i).filter(|&n| inside(n)).map(index) {
            if steps[n] > next {
                steps[n] = next;
                queue.push_back(n);
            }
        }
    }
    // Ring by ring from the pixels further in than `reach`: the fringe's colours.
    let mut colour: Vec<Option<[f32; 3]>> =
        px.iter().zip(&steps).map(|(p, &s)| (s > reach).then(|| straight(*p).0)).collect();
    let mut ring: Vec<usize> = (0..px.len())
        .filter(|&i| colour[i].is_none() && shows(&px[i]))
        .filter(|&i| neighbours(i).filter(|&n| inside(n)).any(|n| colour[index(n)].is_some()))
        .collect();
    for _ in 0..reach {
        if ring.is_empty() {
            break;
        }
        let given: Vec<(usize, [f32; 3])> = ring
            .iter()
            .map(|&i| {
                let known: Vec<[f32; 3]> =
                    neighbours(i).filter(|&n| inside(n)).filter_map(|n| colour[index(n)]).collect();
                let k = known.len() as f32;
                let mean = known.iter().fold([0.0; 3], |s, c| [s[0] + c[0] / k, s[1] + c[1] / k, s[2] + c[2] / k]);
                (i, mean)
            })
            .collect();
        for &(i, c) in &given {
            colour[i] = Some(c);
            px[i] = premultiplied(c, straight(px[i]).1);
        }
        let mut next: Vec<usize> = given
            .iter()
            .flat_map(|&(i, _)| neighbours(i).filter(|&n| inside(n)).map(index).collect::<Vec<_>>())
            .filter(|&n| colour[n].is_none() && shows(&px[n]))
            .collect();
        next.sort_unstable();
        next.dedup();
        ring = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A working pixel of straight sRGB-encoded grey `v` at alpha `a`.
    fn grey(v: f64, a: f32) -> Pixel {
        let c = srgb_decode(v) as f32;
        premultiplied([c, c, c], a)
    }

    /// The straight sRGB-encoded grey of `p`.
    fn encoded(p: Pixel) -> f64 {
        srgb_encode(f64::from(straight(p).0[0]))
    }

    #[test]
    fn a_matte_is_taken_back_out_of_what_is_partly_transparent_only() {
        // A grey of 0.8 blended at half over black shows 0.4, over white 0.9.
        let mut px = vec![grey(0.4, 0.5), grey(0.3, 1.0), [f16::ZERO; 4]];
        remove_matte(&mut px, 0.0);
        assert!((encoded(px[0]) - 0.8).abs() < 2e-3, "{}", encoded(px[0]));
        assert!((px[0][3].to_f32() - 0.5).abs() < 1e-3, "its alpha changed");
        assert!((encoded(px[1]) - 0.3).abs() < 2e-3, "an opaque pixel changed");
        assert_eq!(px[2], [f16::ZERO; 4]);
        let mut px = vec![grey(0.9, 0.5)];
        remove_matte(&mut px, 1.0);
        assert!((encoded(px[0]) - 0.8).abs() < 2e-3, "{}", encoded(px[0]));
    }

    #[test]
    fn defringe_gives_the_edge_the_colour_further_in_and_leaves_what_is_too_thin() {
        // A 10 × 10 buffer: a 6 × 6 block (2 to 8): its outer ring red, the ring inside it
        // green, its middle blue; its corner half transparent; and transparency round it.
        let (w, h) = (10, 10);
        let (red, green, blue) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let ring = |x: usize, y: usize| (x - 2).min(y - 2).min(7 - x).min(7 - y);
        let mut px = vec![[f16::ZERO; 4]; w * h];
        for y in 2..8 {
            for x in 2..8 {
                let a = if (x, y) == (2, 2) { 0.5 } else { 1.0 };
                px[y * w + x] = premultiplied([red, green, blue][ring(x, y).min(2)], a);
            }
        }
        defringe(&mut px, w, h, 1);
        // The outer ring green as the ring inside it, which (one step further in than the
        // width) keeps its own; the middle blue still.
        for y in 2..8 {
            for x in 2..8 {
                let (c, a) = straight(px[y * w + x]);
                let want = if ring(x, y) == 2 { blue } else { green };
                assert!(c.iter().zip(want).all(|(c, w)| (c - w).abs() < 1e-3), "({x}, {y}) is {c:?}");
                assert!((a - if (x, y) == (2, 2) { 0.5 } else { 1.0 }).abs() < 1e-3, "({x}, {y})'s alpha changed");
            }
        }
        assert_eq!(px[0], [f16::ZERO; 4], "transparency given a colour");
        // Two pixels wide, with a reach of one: nothing further in, left as it is.
        let mut thin = vec![[f16::ZERO; 4]; w * h];
        for y in 2..8 {
            for x in 4..6 {
                thin[y * w + x] = premultiplied(red, 1.0);
            }
        }
        let before = thin.clone();
        defringe(&mut thin, w, h, 1);
        assert_eq!(thin, before);
    }

    #[test]
    fn a_thin_strand_from_thicker_content_is_coloured_only_near_it() {
        // A 20 × 20 blue block from (2, 2), and a red strand a pixel thick running 30 pixels
        // from its right side, on nothing.
        let (w, h) = (60, 24);
        let clear = premultiplied([0.0; 3], 0.0);
        let mut px = vec![clear; w * h];
        for y in 2..22 {
            for x in 2..22 {
                px[y * w + x] = premultiplied([0.0, 0.0, 1.0], 1.0);
            }
        }
        for x in 22..52 {
            px[12 * w + x] = premultiplied([1.0, 0.0, 0.0], 1.0);
        }
        let before = px.clone();
        defringe(&mut px, w, h, 2);
        // The block's own edge given its blue; the strand, further than two rings from it, left.
        assert_eq!(px[12 * w + 21], before[12 * w + 20]);
        for x in 22..52 {
            assert_eq!(px[12 * w + x], before[12 * w + x], "the strand recoloured at {x}");
        }
    }
}
