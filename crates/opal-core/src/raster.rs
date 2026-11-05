//! Sparse, unbounded bitmaps.

use crate::geom::IRect;
use crate::tile::{Pixel, TILE_SIZE, Tile, TileCoord};
use half::f16;
use std::collections::HashMap;
use std::sync::Arc;

/// A bitmap with no edges: only tiles with content exist, at any coordinates, negative
/// included. Cloning copies the tile map (pointers), not the pixels.
#[derive(Clone, Default, Debug)]
pub struct RasterImage {
    tiles: HashMap<TileCoord, Arc<Tile>>,
}

impl RasterImage {
    pub fn new() -> RasterImage {
        RasterImage::default()
    }

    /// The tile at `c`, if it exists.
    pub fn tile(&self, c: TileCoord) -> Option<&Arc<Tile>> {
        self.tiles.get(&c)
    }

    /// The tile at `c` for changing, created transparent if absent and copied first if it is
    /// shared with another image or history state.
    pub fn tile_mut(&mut self, c: TileCoord) -> &mut Tile {
        Arc::make_mut(self.tiles.entry(c).or_insert_with(|| Arc::new(Tile::transparent())))
    }

    /// Sets or removes the tile at `c`.
    pub fn set_tile(&mut self, c: TileCoord, tile: Option<Arc<Tile>>) {
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
            for y in r.y0..r.y1 {
                let row = &pixels[(y as usize) * width as usize..][..width as usize];
                for x in r.x0..r.x1 {
                    tile.set(ox + (x - r.x0) as u32, oy + (y - r.y0) as u32, row[x as usize]);
                }
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
            for y in r.y0..r.y1 {
                let ty = (y - tr.y0) as u32;
                let dst = &mut out[(y - rect.y0) as usize * w..][..w];
                for x in r.x0..r.x1 {
                    dst[(x - rect.x0) as usize] = tile.get((x - tr.x0) as u32, ty);
                }
            }
        }
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
        let px: Vec<Pixel> = (0..w * h)
            .map(|i| pixel_from_f32([(i % 7) as f32 / 8.0, 0.0, 0.0, 1.0]))
            .collect();
        let img = RasterImage::from_pixels(w, h, &px);
        assert_eq!(img.tile_count(), 2);
        let back = img.read_rect(&IRect::from_size(i64::from(w), i64::from(h)));
        assert_eq!(back, px);
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
