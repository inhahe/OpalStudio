//! Sparse, unbounded bitmaps.

use crate::geom::IRect;
use crate::tile::{Pixel, TILE_SIZE, Tile, TileCoord, downsample};
use half::f16;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

/// How many times an image can be halved in its pyramid (see [`RasterImage::reduced`]).
pub const MAX_REDUCTION: u32 = 24;

/// A bitmap with no edges: only tiles with content exist, at any coordinates, negative
/// included. Cloning copies the tile map (pointers), not the pixels.
///
/// An image also keeps, built when first asked for, its content bounds and a pyramid of
/// half-size copies (for drawing it shrunk); changing a pixel discards both.
#[derive(Default)]
pub struct RasterImage {
    tiles: HashMap<TileCoord, Arc<Tile>>,
    /// `pyramid[k - 1]` is the image halved `k` times.
    pyramid: [OnceLock<Box<RasterImage>>; MAX_REDUCTION as usize],
    bounds: OnceLock<Option<IRect>>,
}

impl Clone for RasterImage {
    fn clone(&self) -> RasterImage {
        RasterImage { tiles: self.tiles.clone(), pyramid: self.pyramid.clone(), bounds: self.bounds.clone() }
    }
}

impl std::fmt::Debug for RasterImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RasterImage({} tiles)", self.tiles.len())
    }
}

impl RasterImage {
    pub fn new() -> RasterImage {
        RasterImage::default()
    }

    /// Forgets what was derived from the pixels, which are about to change.
    fn changed(&mut self) {
        self.pyramid = Default::default();
        self.bounds = OnceLock::new();
    }

    /// The tile at `c`, if it exists.
    pub fn tile(&self, c: TileCoord) -> Option<&Arc<Tile>> {
        self.tiles.get(&c)
    }

    /// The tile at `c` for changing, created transparent if absent and copied first if it is
    /// shared with another image or history state.
    pub fn tile_mut(&mut self, c: TileCoord) -> &mut Tile {
        self.changed();
        Arc::make_mut(self.tiles.entry(c).or_insert_with(|| Arc::new(Tile::transparent())))
    }

    /// Sets or removes the tile at `c`.
    pub fn set_tile(&mut self, c: TileCoord, tile: Option<Arc<Tile>>) {
        self.changed();
        match tile {
            Some(t) => {
                self.tiles.insert(c, t);
            }
            None => {
                self.tiles.remove(&c);
            }
        }
    }

    /// Every tile, in no particular order.
    pub fn tiles(&self) -> impl Iterator<Item = (TileCoord, &Arc<Tile>)> {
        self.tiles.iter().map(|(c, t)| (*c, t))
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The canvas pixels covered by existing tiles (a cheap upper bound on the content).
    pub fn tile_bounds(&self) -> IRect {
        self.tiles.keys().fold(IRect::new(0, 0, 0, 0), |acc, c| acc.union(&c.rect(0)))
    }

    /// The pixel at canvas coordinates `(x, y)`; transparent where no tile exists.
    pub fn pixel(&self, x: i64, y: i64) -> Pixel {
        let c = TileCoord::containing(x, y);
        match self.tiles.get(&c) {
            Some(t) => {
                let r = c.rect(0);
                t.get((x - r.x0) as u32, (y - r.y0) as u32)
            }
            None => [f16::ZERO; 4],
        }
    }

    /// Sets the pixel at canvas coordinates `(x, y)`. Slow; for tests and one-off edits.
    pub fn set_pixel(&mut self, x: i64, y: i64, p: Pixel) {
        let c = TileCoord::containing(x, y);
        let r = c.rect(0);
        self.tile_mut(c).set((x - r.x0) as u32, (y - r.y0) as u32, p);
    }

    /// Removes tiles that are entirely transparent.
    pub fn prune_transparent(&mut self) {
        self.tiles.retain(|_, t| !t.is_transparent());
    }

    /// Builds an image from a row-major buffer of `width × height` pixels placed with its
    /// top-left corner at the canvas origin. Tiles that come out fully transparent are
    /// left out.
    pub fn from_pixels(width: u32, height: u32, pixels: &[Pixel]) -> RasterImage {
        assert_eq!(pixels.len(), width as usize * height as usize, "buffer size");
        let mut img = RasterImage::new();
        let area = IRect::from_size(i64::from(width), i64::from(height));
        for c in TileCoord::covering(&area, 0) {
            let r = c.rect(0).intersect(&area);
            let mut tile = Tile::transparent();
            let ox = (r.x0 - c.rect(0).x0) as u32;
            let oy = (r.y0 - c.rect(0).y0) as u32;
            let n = (r.x1 - r.x0) as usize;
            for y in r.y0..r.y1 {
                let src = &pixels[y as usize * width as usize + r.x0 as usize..][..n];
                let ty = oy as usize + (y - r.y0) as usize;
                let start = ty * TILE_SIZE as usize + ox as usize;
                tile.pixels_mut()[start..start + n].copy_from_slice(src);
            }
            if !tile.is_transparent() {
                img.tiles.insert(c, Arc::new(tile));
            }
        }
        img
    }

    /// Copies the pixels of `rect` into a row-major buffer.
    pub fn read_rect(&self, rect: &IRect) -> Vec<Pixel> {
        let (w, h) = (rect.width() as usize, rect.height() as usize);
        let mut out = vec![[f16::ZERO; 4]; w * h];
        for c in TileCoord::covering(rect, 0) {
            let Some(tile) = self.tiles.get(&c) else { continue };
            let tr = c.rect(0);
            let r = tr.intersect(rect);
            let n = (r.x1 - r.x0) as usize;
            for y in r.y0..r.y1 {
                let start = (y - tr.y0) as usize * TILE_SIZE as usize + (r.x0 - tr.x0) as usize;
                let dst = (y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize;
                out[dst..dst + n].copy_from_slice(&tile.pixels()[start..start + n]);
            }
        }
        out
    }

    /// Composites `src` over this image, tile by tile (premultiplied "over"): what drawing
    /// `src` onto these pixels leaves.
    pub fn draw_over(&mut self, src: &RasterImage) {
        let mut a = vec![[0.0f32; 4]; crate::tile::TILE_PIXELS];
        let mut b = vec![[0.0f32; 4]; crate::tile::TILE_PIXELS];
        for (c, s) in src.tiles() {
            match self.tiles.get(&c) {
                None => self.set_tile(c, Some(s.clone())),
                Some(_) => {
                    s.to_f32(&mut b);
                    let dst = self.tile_mut(c);
                    dst.to_f32(&mut a);
                    for (d, s) in a.iter_mut().zip(&b) {
                        let k = 1.0 - s[3];
                        for i in 0..4 {
                            d[i] = s[i] + d[i] * k;
                        }
                    }
                    *dst = Tile::from_f32(&a);
                }
            }
        }
    }

    /// The smallest rectangle holding every pixel that is not fully transparent, or `None`
    /// for an image with no content. Computed on first use and kept until a pixel changes.
    pub fn content_bounds(&self) -> Option<IRect> {
        // Computed first and then stored, rather than inside `get_or_init`: the computation
        // runs on rayon, whose work stealing could otherwise run a job that waits on this
        // same cell on the thread initializing it.
        if let Some(b) = self.bounds.get() {
            return *b;
        }
        let b = self
            .tiles
            .par_iter()
            .filter_map(|(c, t)| {
                let [x0, y0, x1, y1] = t.content_rect()?;
                let o = c.rect(0);
                Some(IRect::new(o.x0 + i64::from(x0), o.y0 + i64::from(y0), o.x0 + i64::from(x1), o.y0 + i64::from(y1)))
            })
            .reduce_with(|a, b| a.union(&b));
        let _ = self.bounds.set(b);
        b
    }

    /// The image halved `k` times: each pixel the average of a 2^k × 2^k block, so pixel
    /// `(x, y)` covers this image's pixels from `(x·2^k, y·2^k)` on. Built on first use
    /// (each level from the one below) and kept until a pixel changes.
    pub fn reduced(&self, k: u32) -> &RasterImage {
        if k == 0 {
            return self;
        }
        let k = k.min(MAX_REDUCTION);
        let cell = &self.pyramid[k as usize - 1];
        if let Some(r) = cell.get() {
            return r;
        }
        // Not `get_or_init`, for the reason given in `content_bounds`.
        let half = self.reduced(k - 1).halved();
        let _ = cell.set(Box::new(half));
        cell.get().expect("the level was just stored")
    }

    /// Whether level `k` of the pyramid has been built.
    pub fn has_reduced(&self, k: u32) -> bool {
        k == 0 || self.pyramid.get(k as usize - 1).is_some_and(|c| c.get().is_some())
    }

    /// The image at half size, each pixel the average of a 2×2 block.
    fn halved(&self) -> RasterImage {
        let parents: HashSet<TileCoord> = self.tiles.keys().map(|c| c.parent()).collect();
        let tiles: Vec<(TileCoord, Tile)> = parents
            .into_par_iter()
            .filter_map(|p| {
                let kids = p.children().map(|k| self.tiles.get(&k).map(|t| &**t));
                downsample(kids).map(|t| (p, t))
            })
            .collect();
        let mut out = RasterImage::new();
        out.tiles = tiles.into_iter().map(|(c, t)| (c, Arc::new(t))).collect();
        out
    }
}

/// The size of one tile's worth of pixels in bytes (for memory accounting).
pub const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE) as usize * std::mem::size_of::<Pixel>();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::pixel_from_f32;

    #[test]
    fn writing_copies_a_shared_tile_and_leaves_the_original() {
        let mut a = RasterImage::new();
        a.set_pixel(10, 10, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        let b = a.clone();
        a.set_pixel(10, 10, pixel_from_f32([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(b.pixel(10, 10), pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(a.pixel(10, 10), pixel_from_f32([0.0, 1.0, 0.0, 1.0]));
    }

    #[test]
    fn untouched_tiles_stay_shared() {
        let mut a = RasterImage::new();
        a.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
        a.set_pixel(1000, 0, pixel_from_f32([1.0; 4]));
        let b = a.clone();
        a.set_pixel(1, 0, pixel_from_f32([0.5; 4]));
        let far = TileCoord::containing(1000, 0);
        assert!(Arc::ptr_eq(a.tile(far).unwrap(), b.tile(far).unwrap()));
        let near = TileCoord::containing(0, 0);
        assert!(!Arc::ptr_eq(a.tile(near).unwrap(), b.tile(near).unwrap()));
    }

    #[test]
    fn negative_coordinates_work() {
        let mut a = RasterImage::new();
        a.set_pixel(-1, -300, pixel_from_f32([0.25, 0.5, 0.75, 1.0]));
        assert_eq!(a.pixel(-1, -300), pixel_from_f32([0.25, 0.5, 0.75, 1.0]));
        assert_eq!(a.pixel(-2, -300), [f16::ZERO; 4]);
        assert_eq!(a.tile_bounds(), IRect::new(-256, -512, 0, -256));
    }

    #[test]
    fn from_pixels_and_read_rect_round_trip() {
        let (w, h) = (300u32, 5u32);
        let px: Vec<Pixel> = (0..w * h).map(|i| pixel_from_f32([(i % 7) as f32 / 8.0, 0.0, 0.0, 1.0])).collect();
        let img = RasterImage::from_pixels(w, h, &px);
        assert_eq!(img.tile_count(), 2);
        let back = img.read_rect(&IRect::from_size(i64::from(w), i64::from(h)));
        assert_eq!(back, px);
    }

    #[test]
    fn content_bounds_are_tight_and_follow_changes() {
        let mut a = RasterImage::new();
        assert_eq!(a.content_bounds(), None);
        a.set_pixel(-3, 10, pixel_from_f32([0.0, 0.0, 0.0, 1.0]));
        a.set_pixel(300, 2, pixel_from_f32([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(a.content_bounds(), Some(IRect::new(-3, 2, 301, 11)));
        a.set_pixel(300, 2, [f16::ZERO; 4]);
        assert_eq!(a.content_bounds(), Some(IRect::new(-3, 10, -2, 11)), "a change discards the stored bounds");
    }

    #[test]
    fn the_pyramid_averages_blocks_and_is_rebuilt_after_a_change() {
        // A 4×4 block of 1s and 0s alternating by column: halved twice, one pixel of 0.5.
        let mut a = RasterImage::new();
        for y in 0..4 {
            for x in 0..4 {
                let v = (x % 2) as f32;
                a.set_pixel(x, y, pixel_from_f32([v, v, v, 1.0]));
            }
        }
        let p = a.reduced(2).pixel(0, 0);
        assert_eq!(p, pixel_from_f32([0.5, 0.5, 0.5, 1.0]));
        assert!(a.has_reduced(1) && a.has_reduced(2) && !a.has_reduced(3));
        a.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
        assert!(!a.has_reduced(1), "a change discards the pyramid");
        // Negative coordinates halve toward negative infinity, like tile coordinates.
        let mut b = RasterImage::new();
        b.set_pixel(-1, -1, pixel_from_f32([1.0; 4]));
        assert_eq!(b.reduced(1).pixel(-1, -1), pixel_from_f32([0.25; 4]));
    }

    #[test]
    fn transparent_tiles_are_not_stored() {
        let px = vec![[f16::ZERO; 4]; 4];
        assert!(RasterImage::from_pixels(2, 2, &px).is_empty());
        let mut a = RasterImage::new();
        a.set_pixel(5, 5, [f16::ZERO; 4]);
        assert_eq!(a.tile_count(), 1);
        a.prune_transparent();
        assert!(a.is_empty());
    }
}
