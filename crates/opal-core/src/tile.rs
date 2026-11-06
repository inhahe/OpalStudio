//! Tiles: the unit in which pixels are stored, shared and copied.
//!
//! # Pixel format
//!
//! Each pixel is RGBA in IEEE half floats (8 bytes): **linear light**, sRGB/Rec.709
//! primaries with a D65 white, **premultiplied** by alpha, and **extended range**: colors
//! outside the sRGB gamut are stored as color values below 0 or above 1, as scRGB does, so
//! no conversion into the working space loses gamut. Alpha is in 0–1, and a pixel whose
//! alpha is 0 has all components 0.
//!
//! Half floats keep about 11 significant bits at every brightness, which beats 8-bit sRGB
//! everywhere, and GPUs sample and blend them natively.
//!
//! # Storage
//!
//! A tile holds working pixels, or, for pixels no finer than an 8-bit file's (an 8-bit
//! image just opened), the 8-bit sRGB values with straight alpha they came from: half the
//! memory. Such a tile stands for exactly the working pixels [`pixel_from_srgb8`] makes of
//! them, and reads as those everywhere; changing it makes it working pixels.

use crate::geom::IRect;
use crate::store::{Body, Slot};
use half::f16;
use half::slice::HalfFloatSliceExt;
use parking_lot::{MappedRwLockWriteGuard, RwLockReadGuard, RwLockWriteGuard};
use std::sync::{Arc, OnceLock};

/// Width and height of a tile, in pixels.
pub const TILE_SIZE: u32 = 256;
/// Pixels in a tile.
pub const TILE_PIXELS: usize = (TILE_SIZE * TILE_SIZE) as usize;

/// One stored pixel; see the module documentation for its meaning.
pub type Pixel = [f16; 4];
/// One pixel during computation: the same meaning as [`Pixel`], in single precision.
pub type PixelF32 = [f32; 4];

/// The position of a tile in a grid. At pyramid level `k` (see
/// [`crate::composite::Projection`]) a tile covers `TILE_SIZE << k` canvas pixels each way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord {
    pub x: i32,
    pub y: i32,
}

impl TileCoord {
    pub const fn new(x: i32, y: i32) -> TileCoord {
        TileCoord { x, y }
    }

    /// The level-0 tile containing canvas pixel `(x, y)`.
    pub fn containing(x: i64, y: i64) -> TileCoord {
        let t = i64::from(TILE_SIZE);
        TileCoord::new(x.div_euclid(t) as i32, y.div_euclid(t) as i32)
    }

    /// The canvas pixels this tile covers at pyramid level `level`.
    pub fn rect(self, level: u32) -> IRect {
        let size = i64::from(TILE_SIZE) << level;
        let (x, y) = (i64::from(self.x) * size, i64::from(self.y) * size);
        IRect::new(x, y, x + size, y + size)
    }

    /// The tile one pyramid level up that contains this one.
    pub fn parent(self) -> TileCoord {
        TileCoord::new(self.x >> 1, self.y >> 1)
    }

    /// The four tiles one pyramid level down that this one covers, in the order top-left,
    /// top-right, bottom-left, bottom-right.
    pub fn children(self) -> [TileCoord; 4] {
        let (x, y) = (self.x * 2, self.y * 2);
        [TileCoord::new(x, y), TileCoord::new(x + 1, y), TileCoord::new(x, y + 1), TileCoord::new(x + 1, y + 1)]
    }

    /// All tiles at pyramid level `level` that intersect `rect`.
    pub fn covering(rect: &IRect, level: u32) -> impl Iterator<Item = TileCoord> + use<> {
        let size = i64::from(TILE_SIZE) << level;
        let (tx0, ty0) = (rect.x0.div_euclid(size), rect.y0.div_euclid(size));
        let (tx1, ty1) = if rect.is_empty() {
            (tx0, ty0)
        } else {
            ((rect.x1 - 1).div_euclid(size) + 1, (rect.y1 - 1).div_euclid(size) + 1)
        };
        (ty0..ty1).flat_map(move |y| (tx0..tx1).map(move |x| TileCoord::new(x as i32, y as i32)))
    }
}

/// A square of `TILE_SIZE`² pixels. Cloning copies the pixels (512 KiB, or 256 KiB at 8
/// bits); share tiles through `Arc<Tile>` and copy them with `Arc::make_mut` only when
/// changing them. The pixels may be on disk ([`crate::store`]): reading them brings them
/// back.
pub struct Tile {
    slot: Arc<Slot>,
}

/// A tile's pixels as stored.
#[derive(Clone)]
pub(crate) enum Data {
    /// Working pixels.
    Wide(Box<[Pixel]>),
    /// 8-bit sRGB with straight alpha, standing for the working pixels
    /// [`pixel_from_srgb8`] makes of them (see the module documentation).
    Srgb8(Box<[[u8; 4]]>),
}

impl Data {
    /// All clear, as working pixels or (`srgb8`) at 8 bits.
    pub(crate) fn empty(srgb8: bool) -> Data {
        if srgb8 {
            Data::Srgb8(vec![[0; 4]; TILE_PIXELS].into_boxed_slice())
        } else {
            Data::Wide(vec![[f16::ZERO; 4]; TILE_PIXELS].into_boxed_slice())
        }
    }

    pub(crate) fn is_srgb8(&self) -> bool {
        matches!(self, Data::Srgb8(_))
    }

    /// The memory the pixels take.
    pub(crate) fn bytes(&self) -> u64 {
        self.as_bytes().len() as u64
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self {
            Data::Wide(px) => bytemuck::cast_slice(px),
            Data::Srgb8(px) => bytemuck::cast_slice(px),
        }
    }

    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8] {
        match self {
            Data::Wide(px) => bytemuck::cast_slice_mut(px),
            Data::Srgb8(px) => bytemuck::cast_slice_mut(px),
        }
    }

    fn get(&self, i: usize) -> Pixel {
        match self {
            Data::Wide(px) => px[i],
            Data::Srgb8(px) => pixel_from_srgb8(px[i]),
        }
    }

    /// Whether pixel `i` (row by row) is not fully transparent.
    fn shows(&self, i: usize) -> bool {
        match self {
            Data::Wide(px) => px[i][3] != f16::ZERO,
            Data::Srgb8(px) => px[i][3] != 0,
        }
    }

    fn read_row(&self, start: usize, out: &mut [Pixel]) {
        let end = start + out.len();
        match self {
            Data::Wide(px) => out.copy_from_slice(&px[start..end]),
            Data::Srgb8(px) => out.iter_mut().zip(&px[start..end]).for_each(|(o, &p)| *o = pixel_from_srgb8(p)),
        }
    }

    fn read_row_f32(&self, start: usize, out: &mut [PixelF32]) {
        let end = start + out.len();
        match self {
            Data::Wide(px) => {
                let src: &[f16] = bytemuck::cast_slice(&px[start..end]);
                src.convert_to_f32_slice(bytemuck::cast_slice_mut(out));
            }
            Data::Srgb8(px) => {
                out.iter_mut().zip(&px[start..end]).for_each(|(o, &p)| *o = pixel_to_f32(pixel_from_srgb8(p)))
            }
        }
    }
}

/// A tile's pixels, held for reading: none of them goes to disk meanwhile. For reading
/// many rows of a tile without taking it each time.
pub struct Reader<'a> {
    body: RwLockReadGuard<'a, Body>,
}

impl Reader<'_> {
    fn data(&self) -> &Data {
        match &*self.body {
            Body::Here { data, .. } => data,
            Body::Away { .. } => unreachable!("brought back to be read"),
        }
    }

    /// The pixel at `(x, y)` within the tile.
    pub fn get(&self, x: u32, y: u32) -> Pixel {
        self.data().get(index(x, y))
    }

    /// Copies `out.len()` pixels of row `y`, from column `x`, into `out`.
    pub fn read_row(&self, y: u32, x: u32, out: &mut [Pixel]) {
        assert!(x as usize + out.len() <= TILE_SIZE as usize, "past the row's end");
        self.data().read_row(index(x, y), out);
    }

    /// [`read_row`](Self::read_row), in single precision.
    pub fn read_row_f32(&self, y: u32, x: u32, out: &mut [PixelF32]) {
        assert!(x as usize + out.len() <= TILE_SIZE as usize, "past the row's end");
        self.data().read_row_f32(index(x, y), out);
    }

    /// The 8-bit sRGB pixels a tile keeps as such, row by row.
    pub fn srgb8(&self) -> Option<&[[u8; 4]]> {
        match self.data() {
            Data::Srgb8(px) => Some(px),
            Data::Wide(_) => None,
        }
    }

    /// The working pixels a tile keeps as such, row by row.
    pub fn wide(&self) -> Option<&[Pixel]> {
        match self.data() {
            Data::Wide(px) => Some(px),
            Data::Srgb8(_) => None,
        }
    }
}

impl Clone for Tile {
    fn clone(&self) -> Tile {
        let copy = Tile::of(self.reader().data().clone());
        if let Some(content) = self.slot.content() {
            copy.slot.found_content(content);
        }
        copy
    }
}

/// Tiles are alike when their pixels are, however they are stored.
impl PartialEq for Tile {
    fn eq(&self, other: &Tile) -> bool {
        if Arc::ptr_eq(&self.slot, &other.slot) {
            return true;
        }
        let (a, b) = (self.reader(), other.reader());
        match (a.data(), b.data()) {
            (Data::Wide(x), Data::Wide(y)) => x == y,
            (Data::Srgb8(x), Data::Srgb8(y)) => x == y,
            (x, y) => (0..TILE_PIXELS).all(|i| x.get(i) == y.get(i)),
        }
    }
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = self.reader();
        let visible = (0..TILE_PIXELS).filter(|&i| r.data().shows(i)).count();
        let kind = if r.data().is_srgb8() { ", 8-bit" } else { "" };
        write!(f, "Tile({visible} non-transparent pixels{kind})")
    }
}

impl Tile {
    fn of(data: Data) -> Tile {
        Tile { slot: Slot::new(data) }
    }

    /// The pixels held for reading (brought back from disk first if they are there).
    pub fn reader(&self) -> Reader<'_> {
        Reader { body: self.slot.read() }
    }

    /// The pixels to change, as working pixels (an 8-bit tile becomes working pixels).
    fn wide_mut(&mut self) -> MappedRwLockWriteGuard<'_, [Pixel]> {
        let mut body = self.slot.write();
        let Body::Here { data, .. } = &mut *body else { unreachable!("brought back to be changed") };
        if let Data::Srgb8(px) = data {
            let wide = Data::Wide(px.iter().map(|&p| pixel_from_srgb8(p)).collect());
            self.slot.resized(data.bytes(), wide.bytes());
            *data = wide;
        }
        RwLockWriteGuard::map(body, |b| match b {
            Body::Here { data: Data::Wide(px), .. } => &mut px[..],
            _ => unreachable!("made working pixels above"),
        })
    }

    /// A fully transparent tile.
    pub fn transparent() -> Tile {
        Tile::of(Data::empty(false))
    }

    /// A tile filled with one pixel value.
    pub fn filled(p: Pixel) -> Tile {
        Tile::of(Data::Wide(vec![p; TILE_PIXELS].into_boxed_slice()))
    }

    /// A tile of `TILE_PIXELS` 8-bit sRGB pixels with straight alpha, row by row, kept as
    /// they are (see the module documentation).
    pub fn from_srgb8(px: Vec<[u8; 4]>) -> Tile {
        assert_eq!(px.len(), TILE_PIXELS);
        Tile::of(Data::Srgb8(px.into_boxed_slice()))
    }

    /// Whether the tile keeps its pixels as 8-bit sRGB (asked without bringing them back).
    pub fn is_srgb8(&self) -> bool {
        self.slot.peek().1
    }

    /// The memory the pixels take in memory (asked without bringing them back).
    pub fn bytes(&self) -> u64 {
        self.slot.peek().0
    }

    /// The pixels, row by row from the top-left, made for an 8-bit tile: prefer
    /// [`reader`](Self::reader) or [`to_f32`](Self::to_f32) where they serve.
    pub fn pixels(&self) -> Vec<Pixel> {
        let r = self.reader();
        (0..TILE_PIXELS).map(|i| r.data().get(i)).collect()
    }

    /// The pixels to change, row by row; an 8-bit tile becomes working pixels first.
    pub fn pixels_mut(&mut self) -> MappedRwLockWriteGuard<'_, [Pixel]> {
        self.wide_mut()
    }

    /// The pixel at `(x, y)` within the tile.
    pub fn get(&self, x: u32, y: u32) -> Pixel {
        self.reader().get(x, y)
    }

    pub fn set(&mut self, x: u32, y: u32, p: Pixel) {
        self.wide_mut()[index(x, y)] = p;
    }

    /// Copies `out.len()` pixels of row `y`, from column `x`, into `out`.
    pub fn read_row(&self, y: u32, x: u32, out: &mut [Pixel]) {
        self.reader().read_row(y, x, out);
    }

    /// [`read_row`](Self::read_row), in single precision.
    pub fn read_row_f32(&self, y: u32, x: u32, out: &mut [PixelF32]) {
        self.reader().read_row_f32(y, x, out);
    }

    /// Whether every pixel is fully transparent (known without bringing the pixels back from
    /// disk once asked, until they change; see [`content_rect`](Self::content_rect)).
    pub fn is_transparent(&self) -> bool {
        self.content_rect().is_none()
    }

    /// Converts the pixels to single precision, into `out` (which must hold `TILE_PIXELS`).
    pub fn to_f32(&self, out: &mut [PixelF32]) {
        assert_eq!(out.len(), TILE_PIXELS);
        self.reader().data().read_row_f32(0, out);
    }

    /// A tile made from single-precision pixels (`TILE_PIXELS` of them).
    pub fn from_f32(src: &[PixelF32]) -> Tile {
        assert_eq!(src.len(), TILE_PIXELS);
        let mut px = vec![[f16::ZERO; 4]; TILE_PIXELS];
        let dst: &mut [f16] = bytemuck::cast_slice_mut(&mut px);
        dst.convert_from_f32_slice(bytemuck::cast_slice(src));
        Tile::of(Data::Wide(px.into_boxed_slice()))
    }

    /// A tile made from `TILE_PIXELS` stored pixels, row by row.
    pub fn from_pixels(px: Vec<Pixel>) -> Tile {
        assert_eq!(px.len(), TILE_PIXELS);
        Tile::of(Data::Wide(px.into_boxed_slice()))
    }

    /// The tile with every pixel outside `keep` (`[x0, y0, x1, y1]` within it) made
    /// transparent, stored as this one is.
    pub fn kept_within(&self, keep: [u32; 4]) -> Tile {
        let [x0, y0, x1, y1] = keep;
        let inside = |i: usize| {
            let (x, y) = ((i % TILE_SIZE as usize) as u32, (i / TILE_SIZE as usize) as u32);
            (x0..x1).contains(&x) && (y0..y1).contains(&y)
        };
        let data = match self.reader().data() {
            Data::Wide(px) => {
                Data::Wide(px.iter().enumerate().map(|(i, &p)| if inside(i) { p } else { [f16::ZERO; 4] }).collect())
            }
            Data::Srgb8(px) => {
                Data::Srgb8(px.iter().enumerate().map(|(i, &p)| if inside(i) { p } else { [0; 4] }).collect())
            }
        };
        Tile::of(data)
    }

    /// The smallest rectangle (`[x0, y0, x1, y1]` within the tile) holding every pixel that
    /// is not fully transparent, or `None` if there is none. Kept once found, until the
    /// pixels change: an image's bounds, found again after any change to it, then cost
    /// nothing for its tiles on disk.
    pub fn content_rect(&self) -> Option<[u32; 4]> {
        if let Some(known) = self.slot.content() {
            return known;
        }
        let found = self.find_content();
        self.slot.found_content(found);
        found
    }

    fn find_content(&self) -> Option<[u32; 4]> {
        let r = self.reader();
        let shows = |i: usize| r.data().shows(i);
        let n = TILE_SIZE as usize;
        let opaque_row = |y: usize| (y * n..(y + 1) * n).any(shows);
        let y0 = (0..n).find(|&y| opaque_row(y))?;
        let y1 = (y0..n).rev().find(|&y| opaque_row(y)).unwrap_or(y0) + 1;
        let opaque_col = |x: usize| (y0..y1).any(|y| shows(y * n + x));
        let x0 = (0..n).find(|&x| opaque_col(x)).unwrap_or(0);
        let x1 = (x0..n).rev().find(|&x| opaque_col(x)).unwrap_or(x0) + 1;
        Some([x0 as u32, y0 as u32, x1 as u32, y1 as u32])
    }
}

/// An 8-bit sRGB pixel with straight alpha, as PNG and JPEG store them, as a working pixel:
/// decoded to linear light and premultiplied, then rounded to half floats.
pub fn pixel_from_srgb8([r, g, b, a]: [u8; 4]) -> Pixel {
    let row = &srgb8_table()[a as usize];
    [row[r as usize], row[g as usize], row[b as usize], row[256]]
}

/// For each alpha, each 8-bit sRGB value decoded and premultiplied by it as a half float,
/// then the alpha itself (all 0 for alpha 0, as a clear pixel has).
fn srgb8_table() -> &'static [[f16; 257]; 256] {
    static TABLE: OnceLock<Box<[[f16; 257]; 256]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([[f16::ZERO; 257]; 256]);
        for (a, row) in t.iter_mut().enumerate().skip(1) {
            let alpha = a as f32 / 255.0;
            for (v, e) in row.iter_mut().take(256).enumerate() {
                *e = f16::from_f32(opal_color::srgb_decode(v as f64 / 255.0) as f32 * alpha);
            }
            row[256] = f16::from_f32(alpha);
        }
        t
    })
}

/// Halves four tiles (top-left, top-right, bottom-left, bottom-right) into one, averaging
/// each 2×2 block of premultiplied linear pixels; `None` where the result is transparent.
pub fn downsample(children: [Option<&Tile>; 4]) -> Option<Tile> {
    if children.iter().all(Option::is_none) {
        return None;
    }
    let half = TILE_SIZE / 2;
    let mut out = vec![[0.0f32; 4]; TILE_PIXELS];
    let mut buf = vec![[0.0f32; 4]; TILE_PIXELS];
    for (q, child) in children.iter().enumerate() {
        let Some(child) = child else { continue };
        child.to_f32(&mut buf);
        let (qx, qy) = ((q as u32 % 2) * half, (q as u32 / 2) * half);
        for y in 0..half {
            for x in 0..half {
                let i = |dx: u32, dy: u32| ((2 * y + dy) * TILE_SIZE + 2 * x + dx) as usize;
                let (a, b, c, d) = (buf[i(0, 0)], buf[i(1, 0)], buf[i(0, 1)], buf[i(1, 1)]);
                let o = &mut out[((qy + y) * TILE_SIZE + qx + x) as usize];
                for k in 0..4 {
                    o[k] = (a[k] + b[k] + c[k] + d[k]) * 0.25;
                }
            }
        }
    }
    let t = Tile::from_f32(&out);
    (!t.is_transparent()).then_some(t)
}

fn index(x: u32, y: u32) -> usize {
    debug_assert!(x < TILE_SIZE && y < TILE_SIZE, "pixel ({x}, {y}) outside a tile");
    (y * TILE_SIZE + x) as usize
}

/// Converts a single-precision pixel to storage precision.
pub fn pixel_from_f32(p: PixelF32) -> Pixel {
    p.map(f16::from_f32)
}

/// Converts a stored pixel to single precision.
pub fn pixel_to_f32(p: Pixel) -> PixelF32 {
    p.map(f16::to_f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_floor_toward_negative_infinity() {
        assert_eq!(TileCoord::containing(0, 0), TileCoord::new(0, 0));
        assert_eq!(TileCoord::containing(255, 256), TileCoord::new(0, 1));
        assert_eq!(TileCoord::containing(-1, -256), TileCoord::new(-1, -1));
        assert_eq!(TileCoord::containing(-257, 0), TileCoord::new(-2, 0));
        assert_eq!(TileCoord::new(-3, 5).parent(), TileCoord::new(-2, 2));
        for c in TileCoord::new(-3, 5).children() {
            assert_eq!(c.parent(), TileCoord::new(-3, 5));
        }
    }

    #[test]
    fn covering_lists_exactly_the_intersecting_tiles() {
        let r = IRect::new(-10, 0, 300, 256);
        let got: Vec<_> = TileCoord::covering(&r, 0).collect();
        assert_eq!(got, vec![TileCoord::new(-1, 0), TileCoord::new(0, 0), TileCoord::new(1, 0)]);
        assert_eq!(TileCoord::covering(&IRect::new(0, 0, 0, 0), 0).count(), 0);
        let big = IRect::new(0, 0, 1024, 1024);
        assert_eq!(TileCoord::covering(&big, 2).collect::<Vec<_>>(), vec![TileCoord::new(0, 0)]);
    }

    #[test]
    fn f32_round_trip_is_exact_for_half_values() {
        let mut t = Tile::transparent();
        t.set(3, 4, pixel_from_f32([0.25, -0.5, 1.5, 0.75]));
        let mut buf = vec![[0.0f32; 4]; TILE_PIXELS];
        t.to_f32(&mut buf);
        assert_eq!(buf[4 * TILE_SIZE as usize + 3], [0.25, -0.5, 1.5, 0.75]);
        assert_eq!(Tile::from_f32(&buf), t);
        assert!(!t.is_transparent());
        assert!(Tile::transparent().is_transparent());
    }

    #[test]
    fn content_rect_is_tight() {
        assert_eq!(Tile::transparent().content_rect(), None);
        assert_eq!(Tile::filled(pixel_from_f32([1.0; 4])).content_rect(), Some([0, 0, TILE_SIZE, TILE_SIZE]));
        let mut t = Tile::transparent();
        t.set(3, 200, pixel_from_f32([0.0, 0.0, 0.0, 0.5]));
        t.set(17, 9, pixel_from_f32([0.0, 0.0, 0.0, 0.5]));
        assert_eq!(t.content_rect(), Some([3, 9, 18, 201]));
    }

    /// What an 8-bit sRGB pixel with straight alpha means, worked out afresh: decoded,
    /// premultiplied, rounded to half floats.
    fn by_formula([r, g, b, a]: [u8; 4]) -> Pixel {
        if a == 0 {
            return [f16::ZERO; 4];
        }
        let alpha = f32::from(a) / 255.0;
        let c = |v: u8| f16::from_f32(opal_color::srgb_decode(f64::from(v) / 255.0) as f32 * alpha);
        [c(r), c(g), c(b), f16::from_f32(alpha)]
    }

    #[test]
    fn eight_bit_pixels_read_as_the_formula_says_for_every_value_and_alpha() {
        for a in 0..=255u8 {
            for v in 0..=255u8 {
                let p = [v, 255 - v, v / 2, a];
                assert_eq!(pixel_from_srgb8(p), by_formula(p), "{p:?}");
            }
        }
    }

    /// An 8-bit tile of every kind of pixel, and the same tile as working pixels.
    fn both_ways() -> (Tile, Tile) {
        let px: Vec<[u8; 4]> =
            (0..TILE_PIXELS).map(|i| [i as u8, (i / 256) as u8, (i * 7) as u8, (i / 3) as u8]).collect();
        let wide = Tile::from_pixels(px.iter().map(|&p| by_formula(p)).collect());
        (Tile::from_srgb8(px), wide)
    }

    #[test]
    fn an_eight_bit_tile_reads_as_its_working_pixels_every_way() {
        let (eight, wide) = both_ways();
        assert!(eight.is_srgb8() && !wide.is_srgb8());
        assert_eq!(eight.bytes() * 2, wide.bytes());
        assert!(eight == wide, "alike, however stored");
        assert!(*eight.pixels() == *wide.pixels());
        assert_eq!(eight.get(200, 100), wide.get(200, 100));
        let (mut a, mut b) = (vec![[f16::ZERO; 4]; 100], vec![[f16::ZERO; 4]; 100]);
        eight.read_row(37, 150, &mut a);
        wide.read_row(37, 150, &mut b);
        assert_eq!(a, b);
        let (mut a, mut b) = (vec![[0.0f32; 4]; 100], vec![[0.0f32; 4]; 100]);
        eight.read_row_f32(37, 150, &mut a);
        wide.read_row_f32(37, 150, &mut b);
        assert_eq!(a, b);
        let (mut a, mut b) = (vec![[0.0f32; 4]; TILE_PIXELS], vec![[0.0f32; 4]; TILE_PIXELS]);
        eight.to_f32(&mut a);
        wide.to_f32(&mut b);
        assert_eq!(a, b);
        assert_eq!(eight.content_rect(), wide.content_rect());
        assert_eq!(eight.is_transparent(), wide.is_transparent());
    }

    #[test]
    fn changing_an_eight_bit_tile_makes_it_working_pixels_and_cropping_one_keeps_it() {
        let (eight, wide) = both_ways();
        let kept = eight.kept_within([10, 20, 30, 40]);
        assert!(kept.is_srgb8(), "cropping is a copy, not a change");
        assert!(kept == wide.kept_within([10, 20, 30, 40]));
        assert_eq!(kept.content_rect(), wide.kept_within([10, 20, 30, 40]).content_rect());
        let mut changed = eight.clone();
        changed.set(5, 5, pixel_from_f32([1.0; 4]));
        assert!(!changed.is_srgb8());
        let mut want = wide.clone();
        want.set(5, 5, pixel_from_f32([1.0; 4]));
        assert!(changed == want);
    }
}
