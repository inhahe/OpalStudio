//! Resampling: drawing a bitmap through an affine transform.
//!
//! [`render_tile`] draws one level-0 document tile of an image seen through a transform.
//! Everything that draws an image at another size or angle uses it (image objects,
//! Resize Image, export at a chosen size, Rasterize Layer), so all of them give the pixels
//! the canvas shows.
//!
//! - Transforms that move the pixel grid exactly onto itself (whole-pixel moves, quarter
//!   turns, flips) copy pixels without filtering, whatever the method.
//! - Otherwise each result pixel is a weighted sum of source pixels around the point its
//!   center maps to, with the method's kernel. Where the image shrinks, the sum is taken
//!   from the image's pyramid ([`RasterImage::reduced`]): the level just finer than the
//!   result, with the kernel widened by the remaining factor along each source axis, so
//!   shrinking never aliases.
//! - Weights are normalized, so a uniform area stays exactly uniform, and the image's
//!   edges fade out over the kernel's reach (pixels past them are transparent).

use crate::geom::{Affine, GridMap, IRect};
use crate::raster::{MAX_REDUCTION, RasterImage};
use crate::tile::{Pixel, PixelF32, TILE_PIXELS, TILE_SIZE, Tile, TileCoord};
use half::f16;
use half::slice::HalfFloatSliceExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How pixels are computed when an image is drawn at another size or angle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Method {
    /// Each pixel takes the source pixel its center lands in: hard pixels, for pixel art.
    Nearest,
    /// Linear interpolation between the four nearest pixels (a tent filter when shrinking).
    Bilinear,
    /// The Mitchell–Netravali family of cubic filters. B = 0, C = 0.5 is Catmull–Rom
    /// (sharp, the default); B = C = 1/3 is Mitchell's own choice (softer, less ringing).
    Bicubic { b: f32, c: f32 },
    /// A windowed sinc with this many lobes (2 to 4): the sharpest, and the most ringing.
    Lanczos { lobes: u32 },
}

/// The default cubic filter.
pub const CATMULL_ROM: Method = Method::Bicubic { b: 0.0, c: 0.5 };

impl Method {
    /// How far the kernel reaches, in source pixels at unit scale (nothing for Nearest).
    pub fn radius(&self) -> f64 {
        match self {
            Method::Nearest => 0.0,
            Method::Bilinear => 1.0,
            Method::Bicubic { .. } => 2.0,
            Method::Lanczos { lobes } => f64::from(*lobes),
        }
    }
}

/// A method and the options that go with it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sampling {
    pub method: Method,
    /// Keeps each result within the range of the four source pixels nearest the point it
    /// samples, which removes the halos sharp filters draw along hard edges.
    pub anti_ringing: bool,
}

impl Default for Sampling {
    fn default() -> Sampling {
        Sampling { method: CATMULL_ROM, anti_ringing: true }
    }
}

impl Sampling {
    /// What is used while a transform is being dragged, so the drag keeps up.
    pub const DRAFT: Sampling = Sampling { method: Method::Bilinear, anti_ringing: false };

    /// Checks that the values are in range (for reading files).
    pub fn check(&self) -> Result<(), String> {
        match self.method {
            Method::Bicubic { b, c } if !(0.0..=1.0).contains(&b) || !(0.0..=1.0).contains(&c) => {
                Err(format!("bicubic B = {b} and C = {c} must lie between 0 and 1"))
            }
            Method::Lanczos { lobes } if !(2..=4).contains(&lobes) => {
                Err(format!("Lanczos needs 2 to 4 lobes, not {lobes}"))
            }
            _ => Ok(()),
        }
    }

    /// The values as bits, for exact comparison in cache keys.
    pub fn key(&self) -> [u32; 4] {
        let (kind, p, q) = match self.method {
            Method::Nearest => (0, 0, 0),
            Method::Bilinear => (1, 0, 0),
            Method::Bicubic { b, c } => (2, b.to_bits(), c.to_bits()),
            Method::Lanczos { lobes } => (3, lobes, 0),
        };
        [kind, p, q, u32::from(self.anti_ringing)]
    }
}

/// The most a kernel is widened along one axis before a coarser pyramid level is used
/// instead, which also coarsens the other axis. Only a squash much stronger along one
/// axis than the other reaches it. Axis-aligned drawings filter each axis separately, so
/// a wide kernel costs them only along its own axis, and they allow far more.
const MAX_WIDENING: f64 = 4.0;
const MAX_WIDENING_ALIGNED: f64 = 64.0;

/// The most `inv` (document to image pixels) may widen a kernel at its pyramid level.
fn widening_cap(inv: &Affine) -> f64 {
    if inv.b == 0.0 && inv.c == 0.0 { MAX_WIDENING_ALIGNED } else { MAX_WIDENING }
}

/// Kernel values per unit of distance in the lookup table.
const PER_UNIT: f64 = 512.0;

/// A filter kernel, tabulated.
struct Kernel {
    /// Where the kernel falls to zero, in source pixels at unit scale.
    radius: f64,
    table: Vec<f32>,
}

impl Kernel {
    /// The kernel of a filtering method (not Nearest).
    fn new(method: Method) -> Kernel {
        let radius = method.radius().max(1.0);
        let f: Box<dyn Fn(f64) -> f64> = match method {
            Method::Nearest | Method::Bilinear => Box::new(|x: f64| (1.0 - x).max(0.0)),
            Method::Bicubic { b, c } => Box::new(move |x| mitchell(f64::from(b), f64::from(c), x)),
            Method::Lanczos { .. } => Box::new(move |x| if x < radius { sinc(x) * sinc(x / radius) } else { 0.0 }),
        };
        let len = (radius * PER_UNIT) as usize + 2;
        let mut table: Vec<f32> = (0..len).map(|i| f(i as f64 / PER_UNIT) as f32).collect();
        // The kernel is zero from its radius on.
        let last = table.len() - 1;
        table[last] = 0.0;
        table[last - 1] = 0.0;
        Kernel { radius, table }
    }

    fn at(&self, x: f64) -> f32 {
        let t = x.abs() * PER_UNIT;
        let i = t as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let frac = (t - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * frac
    }
}

/// The Mitchell–Netravali cubic at distance `x >= 0`.
fn mitchell(b: f64, c: f64, x: f64) -> f64 {
    let (x2, x3) = (x * x, x * x * x);
    let v = if x < 1.0 {
        (12.0 - 9.0 * b - 6.0 * c) * x3 + (-18.0 + 12.0 * b + 6.0 * c) * x2 + (6.0 - 2.0 * b)
    } else if x < 2.0 {
        (-b - 6.0 * c) * x3 + (6.0 * b + 30.0 * c) * x2 + (-12.0 * b - 48.0 * c) * x + (8.0 * b + 24.0 * c)
    } else {
        0.0
    };
    v / 6.0
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let p = std::f64::consts::PI * x;
        p.sin() / p
    }
}

/// How far past its own pixels an image drawn through `transform` can reach, in its own
/// pixels: the kernel's reach, widened where the image shrinks.
pub fn reach(transform: &Affine, sampling: &Sampling) -> f64 {
    let radius = sampling.method.radius();
    let stretch = transform.inverse().map_or(1.0, |inv| {
        let [fu, fv] = inv.row_norms();
        fu.max(fv).max(1.0)
    });
    (radius + 2.0) * stretch
}

/// Draws `image` into level-0 document tile `c`, its pixels carried into the document by
/// `transform`; `None` where nothing lands.
pub fn render_tile(image: &RasterImage, transform: &Affine, sampling: &Sampling, c: TileCoord) -> Option<Tile> {
    if image.is_empty() {
        return None;
    }
    let dest = c.rect(0);
    if let Some(g) = transform.grid_map() {
        return render_exact(image, &g, &dest);
    }
    let inv = transform.inverse()?;
    match sampling.method {
        Method::Nearest => render_nearest(image, &inv, &dest),
        m => render_filtered(image, &inv, m, sampling.anti_ringing, &dest),
    }
}

/// The image pixels that `dest` (document pixels) is drawn from through a transform that
/// maps the grid onto itself, given its inverse `gi`.
fn source_rect(gi: &GridMap, dest: &IRect) -> IRect {
    let corners = [(dest.x0, dest.y0), (dest.x1 - 1, dest.y0), (dest.x0, dest.y1 - 1), (dest.x1 - 1, dest.y1 - 1)]
        .map(|(x, y)| gi.pixel(x, y));
    let (xs, ys) = (corners.map(|p| p.0), corners.map(|p| p.1));
    let (min, max) = (|v: [i64; 4]| v.into_iter().min().unwrap_or(0), |v: [i64; 4]| v.into_iter().max().unwrap_or(0));
    IRect::new(min(xs), min(ys), max(xs) + 1, max(ys) + 1)
}

/// Copies pixels for a transform that maps the grid onto itself.
fn render_exact(image: &RasterImage, g: &GridMap, dest: &IRect) -> Option<Tile> {
    let gi = g.inverse();
    let src = source_rect(&gi, dest);
    if src.intersect(&image.tile_bounds()).is_empty() {
        return None;
    }
    let buf = image.read_rect(&src);
    let tile = if (g.a, g.b, g.c, g.d) == (1, 0, 0, 1) {
        Tile::from_pixels(buf)
    } else {
        let w = src.width();
        let mut px = vec![[f16::ZERO; 4]; TILE_PIXELS];
        for (i, p) in px.iter_mut().enumerate() {
            let (x, y) = (dest.x0 + (i as i64 % i64::from(TILE_SIZE)), dest.y0 + (i as i64 / i64::from(TILE_SIZE)));
            let (u, v) = gi.pixel(x, y);
            *p = buf[((v - src.y0) * w + (u - src.x0)) as usize];
        }
        Tile::from_pixels(px)
    };
    (!tile.is_transparent()).then_some(tile)
}

/// Rows of a result tile filtered together: few enough that a band's source pixels and
/// intermediate sums stay in the processor's cache.
const BAND: usize = 32;

/// Working buffers, kept per thread so drawing a tile allocates only the tile itself.
#[derive(Default)]
struct Scratch {
    region: Vec<PixelF32>,
    across: Vec<PixelF32>,
    band: Vec<PixelF32>,
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::default());
}

/// Runs `f` with this thread's working buffers (fresh ones if they are in use).
fn with_scratch<T>(f: impl FnOnce(&mut Scratch) -> T) -> T {
    SCRATCH.with(|s| match s.try_borrow_mut() {
        Ok(mut s) => f(&mut s),
        Err(_) => f(&mut Scratch::default()),
    })
}

/// Source pixels in single precision, transparent where the image has none.
struct Region<'a> {
    rect: IRect,
    px: &'a [PixelF32],
}

impl<'a> Region<'a> {
    /// Reads `rect` of `image` into `buf`.
    fn read(image: &RasterImage, rect: IRect, buf: &'a mut Vec<PixelF32>) -> Region<'a> {
        let w = rect.width() as usize;
        buf.clear();
        buf.resize(w * rect.height() as usize, [0.0; 4]);
        for c in TileCoord::covering(&rect, 0) {
            let Some(tile) = image.tile(c) else { continue };
            let tr = c.rect(0);
            let r = tr.intersect(&rect);
            let n = r.width() as usize;
            for y in r.y0..r.y1 {
                let s = (y - tr.y0) as usize * TILE_SIZE as usize + (r.x0 - tr.x0) as usize;
                let d = (y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize;
                let src: &[f16] = bytemuck::cast_slice(&tile.pixels()[s..s + n]);
                let dst: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[d..d + n]);
                src.convert_to_f32_slice(dst);
            }
        }
        Region { rect, px: buf }
    }

    fn get(&self, x: i64, y: i64) -> PixelF32 {
        if self.rect.contains(x, y) {
            self.px[((y - self.rect.y0) * self.rect.width() + (x - self.rect.x0)) as usize]
        } else {
            [0.0; 4]
        }
    }

    /// Row `y` of the region (which must contain it).
    fn row(&self, y: i64) -> &[PixelF32] {
        let w = self.rect.width() as usize;
        let start = (y - self.rect.y0) as usize * w;
        &self.px[start..start + w]
    }
}

/// A result tile, written a band at a time.
struct Output {
    px: Vec<Pixel>,
    any: bool,
}

impl Output {
    fn new() -> Output {
        Output { px: vec![[f16::ZERO; 4]; TILE_PIXELS], any: false }
    }

    /// Stores `src` from pixel index `start` on.
    fn write(&mut self, start: usize, src: &[PixelF32]) {
        let dst = &mut self.px[start..start + src.len()];
        let d: &mut [f16] = bytemuck::cast_slice_mut(dst);
        d.convert_from_f32_slice(bytemuck::cast_slice(src));
        self.any = self.any || dst.iter().any(|p| p[3] != f16::ZERO);
    }

    fn finish(self) -> Option<Tile> {
        self.any.then(|| Tile::from_pixels(self.px))
    }
}

/// Each result pixel takes the source pixel its center lands in.
fn render_nearest(image: &RasterImage, inv: &Affine, dest: &IRect) -> Option<Tile> {
    let need = IRect::covering(&inv.bounds(&dest.to_f())).expand(1).intersect(&image.tile_bounds());
    if need.is_empty() {
        return None;
    }
    // A strongly shrunk image needs a few scattered pixels from a large area: read those
    // alone rather than the whole area.
    let gather = need.width().saturating_mul(need.height()) <= 4 * TILE_PIXELS as i64;
    let buf = if gather { image.read_rect(&need) } else { Vec::new() };
    let mut px = vec![[f16::ZERO; 4]; TILE_PIXELS];
    for (i, p) in px.iter_mut().enumerate() {
        let x = dest.x0 + (i as i64 % i64::from(TILE_SIZE));
        let y = dest.y0 + (i as i64 / i64::from(TILE_SIZE));
        let [u, v] = inv.apply([x as f64 + 0.5, y as f64 + 0.5]);
        let (u, v) = (u.floor() as i64, v.floor() as i64);
        *p = if !gather {
            image.pixel(u, v)
        } else if need.contains(u, v) {
            buf[((v - need.y0) * need.width() + (u - need.x0)) as usize]
        } else {
            [f16::ZERO; 4]
        };
    }
    let tile = Tile::from_pixels(px);
    (!tile.is_transparent()).then_some(tile)
}

/// The weights of one result pixel along one source axis, normalized to sum to 1 (all 0
/// if the kernel gave nothing).
struct Taps {
    /// The first source pixel weighed.
    first: i64,
    weights: Vec<f32>,
}

impl Taps {
    /// The taps for sampling at `u` (source pixels, centers at `i + 0.5`) with the kernel
    /// widened by `widen`, reusing `weights`' allocation.
    fn new(kernel: &Kernel, u: f64, widen: f64, weights: Vec<f32>) -> Taps {
        let r = kernel.radius * widen;
        let first = (u - 0.5 - r).ceil() as i64;
        let last = (u - 0.5 + r).floor() as i64;
        let mut weights = weights;
        weights.clear();
        let mut sum = 0.0;
        for i in first..=last {
            let w = kernel.at((i as f64 + 0.5 - u) / widen);
            weights.push(w);
            sum += w;
        }
        let scale = if sum > 0.0 { 1.0 / sum } else { 0.0 };
        weights.iter_mut().for_each(|w| *w *= scale);
        Taps { first, weights }
    }

    fn end(&self) -> i64 {
        self.first + self.weights.len() as i64
    }

    /// The part of the taps within source pixels `lo..hi`: where it starts, counted from
    /// `lo`, and its weights.
    fn clipped(&self, lo: i64, hi: i64) -> (usize, &[f32]) {
        let (a, b) = (self.first.max(lo), self.end().min(hi));
        if b <= a {
            return (0, &[]);
        }
        ((a - lo) as usize, &self.weights[(a - self.first) as usize..(b - self.first) as usize])
    }
}

/// `acc += w × p`, channel by channel.
#[inline(always)]
fn add(acc: &mut PixelF32, w: f32, p: PixelF32) {
    for ch in 0..4 {
        acc[ch] += w * p[ch];
    }
}

/// Filters for a transform that does not map the grid onto itself.
fn render_filtered(
    image: &RasterImage,
    inv: &Affine,
    method: Method,
    anti_ringing: bool,
    dest: &IRect,
) -> Option<Tile> {
    let kernel = Kernel::new(method);
    // How many source pixels one result pixel spans along each source axis.
    let [fu, fv] = inv.row_norms();
    let level = pyramid_level(inv);
    let scale = 0.5f64.powi(level as i32);
    // Built before the working buffers are taken: building it runs on rayon, which may run
    // another tile on this thread meanwhile.
    let src = image.reduced(level);
    let bounds = src.tile_bounds();
    let inv = inv.then(Affine::scale(scale, scale));
    // A squash so extreme that the coarsest level still leaves the kernel wider than the cap
    // (one axis shrunk millions of times more than the other) is drawn with the kernel
    // capped there, aliasing a little, rather than with billions of taps.
    let cap = widening_cap(&inv);
    let (wu, wv) = ((fu * scale).clamp(1.0, cap), (fv * scale).clamp(1.0, cap));
    let margin = (kernel.radius * wu.max(wv)).ceil() as i64 + 2;
    let reach = |r: &IRect| IRect::covering(&inv.bounds(&r.to_f())).expand(margin).intersect(&bounds);
    if reach(dest).is_empty() {
        return None;
    }
    let n = TILE_SIZE as usize;
    let center = |i: usize, origin: i64| (origin + i as i64) as f64 + 0.5;
    let axis_aligned = inv.b == 0.0 && inv.c == 0.0;
    // Axis-aligned, every result column shares its horizontal weights and every row its
    // vertical ones, so rows are filtered first, then columns.
    let us: Vec<f64> = (0..n).map(|x| inv.a * center(x, dest.x0) + inv.e).collect();
    let cols: Vec<Taps> =
        if axis_aligned { us.iter().map(|&u| Taps::new(&kernel, u, wu, Vec::new())).collect() } else { Vec::new() };

    with_scratch(|s| {
        let Scratch { region: region_buf, across, band } = s;
        let mut out = Output::new();
        let (mut wx, mut wy) = (Vec::new(), Vec::new());
        for y0 in (0..n).step_by(BAND) {
            let rows = IRect::new(dest.x0, dest.y0 + y0 as i64, dest.x1, dest.y0 + (y0 + BAND) as i64);
            let need = reach(&rows);
            if need.is_empty() {
                continue;
            }
            let region = Region::read(src, need, region_buf);
            let r = region.rect;
            band.clear();
            band.resize(BAND * n, [0.0; 4]);
            if axis_aligned {
                let vs: Vec<f64> = (0..BAND).map(|y| inv.d * center(y0 + y, dest.y0) + inv.f).collect();
                let row_taps: Vec<Taps> = vs.iter().map(|&v| Taps::new(&kernel, v, wv, Vec::new())).collect();
                let j0 = row_taps.iter().map(|t| t.first).min().unwrap_or(r.y0).max(r.y0);
                let j1 = row_taps.iter().map(Taps::end).max().unwrap_or(r.y0).min(r.y1);
                if j0 >= j1 {
                    continue;
                }
                // `across[j - j0][x]`: source row j filtered horizontally for column x.
                let clipped: Vec<(usize, &[f32])> = cols.iter().map(|t| t.clipped(r.x0, r.x1)).collect();
                across.clear();
                across.resize((j1 - j0) as usize * n, [0.0; 4]);
                for j in j0..j1 {
                    let row = region.row(j);
                    let dst = &mut across[(j - j0) as usize * n..][..n];
                    for (d, (start, ws)) in dst.iter_mut().zip(&clipped) {
                        let mut a = [0.0f32; 4];
                        for (k, &w) in ws.iter().enumerate() {
                            add(&mut a, w, row[start + k]);
                        }
                        *d = a;
                    }
                }
                // Each result row is then a weighted sum of whole filtered rows.
                for (y, t) in row_taps.iter().enumerate() {
                    let acc = &mut band[y * n..][..n];
                    let (start, ws) = t.clipped(j0, j1);
                    for (k, &w) in ws.iter().enumerate() {
                        let src = &across[(start + k) * n..][..n];
                        for (a, p) in acc.iter_mut().zip(src) {
                            add(a, w, *p);
                        }
                    }
                    for (x, a) in acc.iter_mut().enumerate() {
                        *a = finish(*a, anti_ringing, &region, us[x], vs[y]);
                    }
                }
            } else if method == Method::Bilinear && wu == 1.0 && wv == 1.0 {
                // Plain bilinear interpolation between the four nearest pixels: what the
                // general sum below computes in this case, without the kernel table.
                for y in 0..BAND {
                    for x in 0..n {
                        let [u, v] = inv.apply([center(x, dest.x0), center(y0 + y, dest.y0)]);
                        let (fu, fv) = (u - 0.5, v - 0.5);
                        let (i, j) = (fu.floor(), fv.floor());
                        let (tx, ty) = ((fu - i) as f32, (fv - j) as f32);
                        let (i, j) = (i as i64, j as i64);
                        let mut acc = [0.0f32; 4];
                        add(&mut acc, (1.0 - tx) * (1.0 - ty), region.get(i, j));
                        add(&mut acc, tx * (1.0 - ty), region.get(i + 1, j));
                        add(&mut acc, (1.0 - tx) * ty, region.get(i, j + 1));
                        add(&mut acc, tx * ty, region.get(i + 1, j + 1));
                        band[y * n + x] = finish(acc, anti_ringing, &region, u, v);
                    }
                }
            } else {
                for y in 0..BAND {
                    for x in 0..n {
                        let [u, v] = inv.apply([center(x, dest.x0), center(y0 + y, dest.y0)]);
                        let tu = Taps::new(&kernel, u, wu, std::mem::take(&mut wx));
                        let tv = Taps::new(&kernel, v, wv, std::mem::take(&mut wy));
                        let (cs, cw) = tu.clipped(r.x0, r.x1);
                        let (rs, rw) = tv.clipped(r.y0, r.y1);
                        let mut acc = [0.0f32; 4];
                        for (k, &w) in rw.iter().enumerate() {
                            let row = &region.row(r.y0 + (rs + k) as i64)[cs..];
                            let mut h = [0.0f32; 4];
                            for (i, &wi) in cw.iter().enumerate() {
                                add(&mut h, wi, row[i]);
                            }
                            add(&mut acc, w, h);
                        }
                        band[y * n + x] = finish(acc, anti_ringing, &region, u, v);
                        wx = tu.weights;
                        wy = tv.weights;
                    }
                }
            }
            out.write(y0 * n, band);
        }
        out.finish()
    })
}

/// Keeps a filtered sample a valid pixel: alpha within 0–1, and nothing where alpha is 0.
/// With anti-ringing, each channel also stays within the range of the four source pixels
/// nearest `(u, v)`.
fn finish(mut p: PixelF32, anti_ringing: bool, region: &Region, u: f64, v: f64) -> PixelF32 {
    if anti_ringing {
        let (i, j) = ((u - 0.5).floor() as i64, (v - 0.5).floor() as i64);
        let near = [region.get(i, j), region.get(i + 1, j), region.get(i, j + 1), region.get(i + 1, j + 1)];
        for ch in 0..4 {
            let lo = near.iter().map(|q| q[ch]).fold(f32::INFINITY, f32::min);
            let hi = near.iter().map(|q| q[ch]).fold(f32::NEG_INFINITY, f32::max);
            // Not `clamp`, which panics when the bounds cross (as they do if the source
            // pixels are not numbers).
            p[ch] = p[ch].max(lo).min(hi);
        }
    }
    // Constant bounds: this clamp cannot panic, and a NaN alpha falls to transparent below.
    p[3] = p[3].clamp(0.0, 1.0);
    if p[3] > 0.0 { p } else { [0.0; 4] }
}

/// Draws `image` through `transform` into a new image covering `area` of the document (all
/// of the result when `area` is `None`), tile by tile in parallel (see [`render_plan`]).
pub fn render_image(image: &RasterImage, transform: &Affine, sampling: &Sampling, area: Option<&IRect>) -> RasterImage {
    use rayon::prelude::*;
    let plan = render_plan(image, transform, sampling, area);
    // Build the pyramid levels the tiles need before drawing them in parallel, so the tiles
    // do not each build them.
    prepare(image, transform, sampling);
    let made: Vec<(Vec<TileCoord>, Tile)> =
        plan.into_par_iter().filter_map(|at| render_tile(image, transform, sampling, at[0]).map(|t| (at, t))).collect();
    let mut out = RasterImage::new();
    for (at, t) in made {
        let t = Arc::new(t);
        for c in at {
            out.set_tile(c, Some(t.clone()));
        }
    }
    out
}

/// The memory, in bytes, drawing `image` through `transform` into `area` would take
/// ([`render_image`]): a tile for each it draws, at most.
pub fn render_cost(image: &RasterImage, transform: &Affine, sampling: &Sampling, area: Option<&IRect>) -> u64 {
    render_plan(image, transform, sampling, area).len() as u64 * crate::raster::TILE_BYTES
}

/// The tiles drawing `image` through `transform` into `area` (everywhere when `None`)
/// draws, each set of tiles alike once: only the tiles some stored tile of `image` reaches,
/// and through a transform that maps the grid onto itself (a move, quarter turn or flip),
/// where tiles drawn from the same stored tiles come out alike, each such set together.
fn render_plan(
    image: &RasterImage,
    transform: &Affine,
    sampling: &Sampling,
    area: Option<&IRect>,
) -> Vec<Vec<TileCoord>> {
    use std::collections::{HashMap, HashSet};
    let Some(bounds) = footprint(image, transform, sampling) else { return Vec::new() };
    let bounds = area.map_or(bounds, |a| bounds.intersect(a));
    if bounds.is_empty() {
        return Vec::new();
    }
    let r = reach(transform, sampling);
    // The stored tiles that may reach the tiles over `bounds` (drawn whole): those under
    // them, mapped back and grown by the filter's reach, when they are fewer than all (a
    // band of a larger drawing).
    let all = image.tile_count();
    let under: Option<Vec<TileCoord>> = transform.inverse().and_then(|inv| {
        let t = i64::from(TILE_SIZE);
        let snap = |a: i64| a.div_euclid(t) * t;
        let whole = IRect::new(snap(bounds.x0), snap(bounds.y0), snap(bounds.x1 - 1) + t, snap(bounds.y1 - 1) + t);
        let back = IRect::covering(&inv.bounds(&whole.to_f()).expand(r));
        let near: Vec<TileCoord> = TileCoord::covering(&back, 0).take(all + 1).collect();
        (near.len() <= all).then(|| near.into_iter().filter(|c| image.tile(*c).is_some()).collect())
    });
    let sources: Box<dyn Iterator<Item = TileCoord>> = match under {
        Some(near) => Box::new(near.into_iter()),
        None => Box::new(image.tiles().map(|(c, _)| c)),
    };
    let mut reached: HashSet<TileCoord> = HashSet::new();
    for c in sources {
        let t = c.rect(0);
        let to = IRect::covering(&transform.bounds(&t.to_f().expand(r)));
        reached.extend(TileCoord::covering(&to.intersect(&bounds), 0));
    }
    let Some(g) = transform.grid_map() else { return reached.into_iter().map(|c| vec![c]).collect() };
    // Each tile is drawn from the same place within the stored tiles under it (the map's
    // offset modulo a tile), so the tiles under it, in order, say what it shows.
    let gi = g.inverse();
    let mut alike: HashMap<Vec<usize>, Vec<TileCoord>> = HashMap::new();
    for c in reached {
        let under = TileCoord::covering(&source_rect(&gi, &c.rect(0)), 0);
        let key: Vec<usize> = under.map(|s| image.tile(s).map_or(0, |t| Arc::as_ptr(t) as usize)).collect();
        // With no stored tile under it, nothing is drawn there.
        if key.iter().any(|&k| k != 0) {
            alike.entry(key).or_default().push(c);
        }
    }
    alike.into_values().collect()
}

/// The document pixels `image` can reach when drawn through `transform`.
pub fn footprint(image: &RasterImage, transform: &Affine, sampling: &Sampling) -> Option<IRect> {
    let content = image.content_bounds()?;
    let r = reach(transform, sampling);
    Some(IRect::covering(&transform.bounds(&content.to_f().expand(r))))
}

/// The pyramid level a filtered drawing through `inv` (document to image pixels) reads:
/// the one just finer than the result along the less shrunk axis, or coarser if the other
/// axis would need a kernel widened past its cap (`widening_cap`).
fn pyramid_level(inv: &Affine) -> u32 {
    let [fu, fv] = inv.row_norms();
    let (su, sv) = (fu.max(1.0), fv.max(1.0));
    let cap = widening_cap(inv);
    let mut level = su.min(sv).log2().floor().max(0.0) as u32;
    while level < MAX_REDUCTION && su.max(sv) / f64::from(1u32 << level) > cap {
        level += 1;
    }
    level.min(MAX_REDUCTION)
}

/// Builds the pyramid level drawing `image` through `transform` will read, so tiles drawn
/// in parallel afterwards find it ready instead of each building it.
pub fn prepare(image: &RasterImage, transform: &Affine, sampling: &Sampling) {
    if matches!(sampling.method, Method::Nearest) || transform.grid_map().is_some() {
        return;
    }
    if let Some(inv) = transform.inverse() {
        image.reduced(pyramid_level(&inv));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::{pixel_from_f32, pixel_to_f32};

    fn solid(w: u32, h: u32, p: PixelF32) -> RasterImage {
        RasterImage::from_pixels(w, h, &vec![pixel_from_f32(p); (w * h) as usize])
    }

    fn render(image: &RasterImage, m: &Affine, s: &Sampling) -> RasterImage {
        render_image(image, m, s, None)
    }

    /// `image` drawn through `m` one tile at a time over all its footprint, as a map.
    fn every_tile(image: &RasterImage, m: &Affine, s: &Sampling) -> std::collections::HashMap<TileCoord, Tile> {
        let Some(bounds) = footprint(image, m, s) else { return Default::default() };
        TileCoord::covering(&bounds, 0).filter_map(|c| render_tile(image, m, s, c).map(|t| (c, t))).collect()
    }

    #[test]
    fn drawing_only_the_tiles_reached_and_alike_ones_once_gives_what_drawing_every_tile_gives() {
        let grey = Arc::new(Tile::filled(pixel_from_f32([0.5, 0.5, 0.5, 1.0])));
        let mut mixed = RasterImage::filled(&IRect::new(0, 0, 2000, 1500), pixel_from_f32([0.2, 0.4, 0.6, 1.0]));
        mixed.set_tile(TileCoord::new(2, 2), Some(grey.clone()));
        mixed.set_pixel(1999, 0, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        // Two marks far apart: the box between them is empty.
        let mut sparse = RasterImage::new();
        sparse.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
        sparse.set_pixel(9000, 7000, pixel_from_f32([1.0; 4]));
        let flip = Affine::scale(-1.0, 1.0).then(Affine::translate(2048.0, 0.0));
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2).then(Affine::translate(1537.0, 13.0));
        let cases = [
            (&mixed, flip, "a flip"),
            (&mixed, quarter, "a quarter turn off the grid"),
            (&sparse, Affine::rotate(0.3), "a sparse image turned"),
            (&mixed, Affine::scale(0.3, 0.45), "shrunk"),
        ];
        for (image, m, what) in cases {
            let s = Sampling::default();
            let planned = render_image(image, &m, &s, None);
            let every = every_tile(image, &m, &s);
            assert_eq!(planned.tile_count(), every.len(), "{what}");
            for (c, t) in &every {
                assert!(planned.tile(*c).is_some_and(|p| **p == *t), "{what}: tile {c:?}");
            }
        }
    }

    #[test]
    fn drawing_a_band_gives_the_pixels_of_drawing_it_all_there() {
        let mut img = RasterImage::filled(&IRect::new(0, 0, 3000, 2000), pixel_from_f32([0.2, 0.4, 0.6, 1.0]));
        img.set_pixel(1234, 777, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        let s = Sampling::default();
        for m in
            [Affine::scale(0.37, 0.41), Affine::rotate(0.3).then(Affine::scale(1.3, 1.3)), Affine::scale(-1.0, 1.0)]
        {
            let all = render_image(&img, &m, &s, None);
            let Some(bounds) = footprint(&img, &m, &s) else { panic!() };
            // A band of a tile's rows across it, off the band's own grid.
            let band = IRect::new(bounds.x0, bounds.y0 + 300, bounds.x1, bounds.y0 + 556);
            let part = render_image(&img, &m, &s, Some(&band));
            assert!(part.read_rect(&band) == all.read_rect(&band), "{m:?}");
        }
        // A band reached only by the filter, from the stored tiles just past it: an image
        // shrunk to a tenth, the band the rows above its content, which its blur touches
        // (without anti-ringing, which would keep them as clear as the pixels nearest).
        let s = Sampling { anti_ringing: false, ..Sampling::default() };
        // More stored tiles (400) than lie under the band, so only those are looked at.
        let big = RasterImage::filled(&IRect::from_size(5120, 5120), pixel_from_f32([1.0; 4]));
        let tenth = Affine::scale(0.1, 0.1);
        let Some(bounds) = footprint(&big, &tenth, &s) else { panic!() };
        let above = IRect::new(bounds.x0, bounds.y0, bounds.x1, 0);
        assert!(!above.is_empty());
        let all = render_image(&big, &tenth, &s, None).read_rect(&above);
        assert!(all.iter().any(|p| p[3].to_f32() > 0.0), "the blur does not reach the band");
        assert!(render_image(&big, &tenth, &s, Some(&above)).read_rect(&above) == all);
    }

    #[test]
    fn a_flip_of_a_uniform_image_is_a_few_tiles_and_a_sparse_one_costs_what_it_reaches() {
        let s = Sampling::default();
        let white = RasterImage::filled(&IRect::from_size(4096, 4096), pixel_from_f32([1.0; 4]));
        let flip = Affine::scale(-1.0, 1.0).then(Affine::translate(4096.0, 0.0));
        let flipped = render_image(&white, &flip, &s, None);
        let distinct: std::collections::HashSet<*const Tile> = flipped.tiles().map(|(_, t)| Arc::as_ptr(t)).collect();
        assert_eq!((flipped.tile_count(), distinct.len()), (256, 1));
        assert_eq!(render_cost(&white, &flip, &s, None), crate::raster::TILE_BYTES);
        // Two marks far apart, turned: the tiles around each, not the box between them.
        let mut sparse = RasterImage::new();
        sparse.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
        sparse.set_pixel(20_000, 20_000, pixel_from_f32([1.0; 4]));
        let cost = render_cost(&sparse, &Affine::rotate(0.3), &s, None);
        assert!(cost <= 16 * crate::raster::TILE_BYTES, "{} tiles", cost / crate::raster::TILE_BYTES);
    }

    fn methods() -> [Sampling; 5] {
        [
            Sampling { method: Method::Nearest, anti_ringing: false },
            Sampling { method: Method::Bilinear, anti_ringing: false },
            Sampling { method: CATMULL_ROM, anti_ringing: false },
            Sampling { method: Method::Bicubic { b: 1.0 / 3.0, c: 1.0 / 3.0 }, anti_ringing: true },
            Sampling { method: Method::Lanczos { lobes: 3 }, anti_ringing: false },
        ]
    }

    #[test]
    fn grid_maps_copy_pixels_exactly_whatever_the_method() {
        let px: Vec<Pixel> = (0..300 * 7).map(|i| pixel_from_f32([(i % 13) as f32 / 13.0, 0.0, 0.0, 1.0])).collect();
        let img = RasterImage::from_pixels(300, 7, &px);
        // A move by whole pixels, a flip and a quarter turn, each checked against the
        // pixel it should have copied.
        let turn = Affine { a: 0.0, b: 1.0, c: -1.0, d: 0.0, e: 7.0, f: 0.0 };
        for m in [Affine::translate(-37.0, 300.0), Affine::scale(-1.0, 1.0).then(Affine::translate(300.0, 0.0)), turn] {
            let g = m.grid_map().unwrap();
            for s in methods() {
                let out = render(&img, &m, &s);
                for (x, y) in [(0, 0), (299, 6), (123, 3)] {
                    let (u, v) = g.pixel(x, y);
                    assert_eq!(out.pixel(u, v), img.pixel(x, y), "{m:?} {s:?} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn a_uniform_area_stays_exactly_uniform_inside() {
        let c = [0.2, 0.4, 0.6, 1.0];
        let img = solid(100, 80, c);
        let m = Affine::rotate(0.3).then(Affine::scale(2.37, 1.6)).then(Affine::translate(50.5, -20.25));
        for s in methods() {
            let out = render(&img, &m, &s);
            // The image of the center pixel is far from every edge.
            let [x, y] = m.apply([50.0, 40.0]);
            let p = pixel_to_f32(out.pixel(x as i64, y as i64));
            for ch in 0..4 {
                assert!((p[ch] - c[ch]).abs() < 2e-3, "{s:?}: {p:?}");
            }
        }
    }

    #[test]
    fn edges_fade_and_nothing_lands_outside_the_footprint() {
        let img = solid(10, 10, [1.0, 1.0, 1.0, 1.0]);
        let m = Affine::scale(10.0, 10.0).then(Affine::translate(0.5, 0.5));
        let s = Sampling::default();
        let out = render(&img, &m, &s);
        let fp = footprint(&img, &m, &s).unwrap();
        let bounds = out.content_bounds().unwrap();
        assert_eq!(bounds.intersect(&fp), bounds, "everything drawn lies within the footprint");
        let edge = pixel_to_f32(out.pixel(0, 50))[3];
        assert!(edge > 0.0 && edge < 1.0, "the half-covered edge column is partly transparent: {edge}");
    }

    #[test]
    fn the_separable_and_general_paths_agree() {
        let px: Vec<Pixel> =
            (0..64 * 64).map(|i| pixel_from_f32([((i * 7) % 64) as f32 / 64.0, 0.5, 0.25, 1.0])).collect();
        let img = RasterImage::from_pixels(64, 64, &px);
        // Shrinking along one axis, and (for the direct bilinear path) enlarging along both.
        for aligned in [
            Affine::scale(3.3, 0.61).then(Affine::translate(4.25, 9.5)),
            Affine::scale(2.5, 1.25).then(Affine::translate(-3.0, 0.5)),
        ] {
            // The same map with a rotation too small to change any pixel, which forces the
            // general path. (Nearest has one path only; a point exactly on a pixel's edge
            // could fall either way.)
            let nudged = Affine { b: 1e-13, ..aligned };
            for s in methods().into_iter().filter(|s| s.method != Method::Nearest) {
                let a = render(&img, &aligned, &s);
                let b = render(&img, &nudged, &s);
                assert_eq!(a.tile_count(), b.tile_count());
                for (c, t) in a.tiles() {
                    let u = b.tile(c).expect("the same tiles");
                    for (p, q) in t.pixels().iter().zip(u.pixels()) {
                        let (p, q) = (pixel_to_f32(*p), pixel_to_f32(*q));
                        for ch in 0..4 {
                            assert!((p[ch] - q[ch]).abs() < 2e-3, "{s:?}: {p:?} vs {q:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shrinking_does_not_alias() {
        // One-pixel stripes, shrunk eightfold, average to gray with a filter; nearest
        // picks one stripe.
        let px: Vec<Pixel> = (0..256 * 256)
            .map(|i| {
                let v = ((i % 256) % 2) as f32;
                pixel_from_f32([v, v, v, 1.0])
            })
            .collect();
        let img = RasterImage::from_pixels(256, 256, &px);
        let m = Affine::scale(0.125, 0.125);
        for s in methods() {
            let out = render(&img, &m, &s);
            let p = pixel_to_f32(out.pixel(16, 16))[0];
            if s.method == Method::Nearest {
                assert!(p == 0.0 || p == 1.0);
            } else {
                assert!((p - 0.5).abs() < 0.02, "{s:?}: {p}");
            }
        }
    }

    #[test]
    fn anti_ringing_removes_overshoot_at_hard_edges() {
        // Black then white, enlarged: Catmull-Rom overshoots on both sides of the edge.
        let px: Vec<Pixel> = (0..16 * 4)
            .map(|i| if i % 16 < 8 { pixel_from_f32([0.0, 0.0, 0.0, 1.0]) } else { pixel_from_f32([1.0; 4]) })
            .collect();
        let img = RasterImage::from_pixels(16, 4, &px);
        let m = Affine::scale(9.7, 9.7);
        let range = |s: Sampling| {
            let out = render(&img, &m, &s);
            (0..155)
                .map(|x| pixel_to_f32(out.pixel(x, 20))[0])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
        };
        let (lo, hi) = range(Sampling { method: CATMULL_ROM, anti_ringing: false });
        assert!(lo < -0.01 && hi > 1.01, "rings without it: {lo} {hi}");
        let (lo, hi) = range(Sampling { method: CATMULL_ROM, anti_ringing: true });
        assert!(lo >= 0.0 && hi <= 1.0, "stays in range with it: {lo} {hi}");
    }

    #[test]
    fn pixels_that_are_not_numbers_do_not_crash_anti_ringing() {
        let mut img = RasterImage::new();
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [half::f16::NAN; 4]);
            }
        }
        render(&img, &Affine::scale(3.3, 3.3), &Sampling::default());
    }

    #[test]
    fn an_extreme_squash_draws_quickly() {
        // Stretched a million times one way and crushed far more the other: the kernel is
        // capped instead of growing to billions of taps.
        let img = solid(64, 64, [1.0; 4]);
        let squash = Affine { a: 1e6, b: 0.0, c: 0.0, d: 2e-18, e: 0.0, f: 0.0 };
        let t = std::time::Instant::now();
        render_tile(&img, &squash, &Sampling::default(), TileCoord::new(0, 0));
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn squashing_one_axis_keeps_the_other_axis_sharp() {
        // One-pixel columns, squashed fivefold vertically only: the columns survive.
        let px: Vec<Pixel> = (0..64 * 64)
            .map(|i| {
                let v = ((i % 64) % 2) as f32;
                pixel_from_f32([v, v, v, 1.0])
            })
            .collect();
        let img = RasterImage::from_pixels(64, 64, &px);
        let out = render(&img, &Affine::scale(1.0, 0.2), &Sampling::default());
        let (a, b) = (pixel_to_f32(out.pixel(30, 6))[0], pixel_to_f32(out.pixel(31, 6))[0]);
        assert!((a - b).abs() > 0.9, "the columns blurred together: {a} {b}");
    }

    #[test]
    fn sampling_values_are_checked() {
        assert!(Sampling::default().check().is_ok());
        assert!(Sampling { method: Method::Lanczos { lobes: 9 }, anti_ringing: false }.check().is_err());
        assert!(Sampling { method: Method::Bicubic { b: 2.0, c: 0.0 }, anti_ringing: false }.check().is_err());
        assert!(Sampling { method: Method::Bicubic { b: f32::NAN, c: 0.0 }, anti_ringing: false }.check().is_err());
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use crate::tile::pixel_from_f32;

    /// Times drawing a 24-megapixel image (run with `--release --ignored --nocapture`).
    #[test]
    #[ignore]
    fn time_a_large_render() {
        let (w, h) = (6000u32, 4000u32);
        let px: Vec<_> = (0..w * h).map(|i| pixel_from_f32([(i % 251) as f32 / 251.0, 0.5, 0.25, 1.0])).collect();
        let img = RasterImage::from_pixels(w, h, &px);
        let m = Affine::scale(1.07, 1.07).then(Affine::translate(3.5, 2.5));
        for s in [Sampling::DRAFT, Sampling::default()] {
            let t = std::time::Instant::now();
            for _ in 0..50 {
                std::hint::black_box(render_tile(&img, &m, &s, TileCoord::new(3, 3)));
            }
            eprintln!("one tile, one thread, {:?}: {:?}", s.method, t.elapsed() / 50);
        }
        for (name, m) in [
            ("scaled 1.07", Affine::scale(1.07, 1.07).then(Affine::translate(3.5, 2.5))),
            ("rotated 0.2, scaled 1.07", Affine::rotate(0.2).then(Affine::scale(1.07, 1.07))),
            ("shrunk 0.3", Affine::scale(0.3, 0.3)),
        ] {
            for s in [Sampling::DRAFT, Sampling::default()] {
                let t = std::time::Instant::now();
                let out = render_image(&img, &m, &s, None);
                eprintln!("{name:28} {:?}: {:?} ({} tiles)", s.method, t.elapsed(), out.tile_count());
            }
        }
    }
}
