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

use crate::geom::IRect;
use half::f16;
use half::slice::HalfFloatSliceExt;

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
        [
            TileCoord::new(x, y),
            TileCoord::new(x + 1, y),
            TileCoord::new(x, y + 1),
            TileCoord::new(x + 1, y + 1),
        ]
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

/// A square of `TILE_SIZE`² pixels. Cloning copies the pixels (512 KiB); share tiles
/// through `Arc<Tile>` and copy them with `Arc::make_mut` only when changing them.
#[derive(Clone, PartialEq)]
pub struct Tile {
    px: Box<[Pixel]>,
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let visible = self.px.iter().filter(|p| p[3] != f16::ZERO).count();
        write!(f, "Tile({visible} non-transparent pixels)")
    }
}

impl Tile {
    /// A fully transparent tile.
    pub fn transparent() -> Tile {
        Tile { px: vec![[f16::ZERO; 4]; TILE_PIXELS].into_boxed_slice() }
    }

    /// A tile filled with one pixel value.
    pub fn filled(p: Pixel) -> Tile {
        Tile { px: vec![p; TILE_PIXELS].into_boxed_slice() }
    }

    /// The pixels, row by row from the top-left.
    pub fn pixels(&self) -> &[Pixel] {
        &self.px
    }

    pub fn pixels_mut(&mut self) -> &mut [Pixel] {
        &mut self.px
    }

    /// The pixel at `(x, y)` within the tile.
    pub fn get(&self, x: u32, y: u32) -> Pixel {
        self.px[index(x, y)]
    }

    pub fn set(&mut self, x: u32, y: u32, p: Pixel) {
        self.px[index(x, y)] = p;
    }

    /// Whether every pixel is fully transparent.
    pub fn is_transparent(&self) -> bool {
        self.px.iter().all(|p| p[3] == f16::ZERO)
    }

    /// Converts the pixels to single precision, into `out` (which must hold `TILE_PIXELS`).
    pub fn to_f32(&self, out: &mut [PixelF32]) {
        assert_eq!(out.len(), TILE_PIXELS);
        let src: &[f16] = bytemuck::cast_slice(&self.px);
        let dst: &mut [f32] = bytemuck::cast_slice_mut(out);
        src.convert_to_f32_slice(dst);
    }

    /// A tile made from single-precision pixels (`TILE_PIXELS` of them).
    pub fn from_f32(src: &[PixelF32]) -> Tile {
        assert_eq!(src.len(), TILE_PIXELS);
        let mut t = Tile::transparent();
        let src: &[f32] = bytemuck::cast_slice(src);
        let dst: &mut [f16] = bytemuck::cast_slice_mut(&mut t.px);
        dst.convert_from_f32_slice(src);
        t
    }
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
}
