//! Sparse, unbounded bitmaps.

use crate::geom::IRect;
use crate::tile::{Pixel, TILE_PIXELS, TILE_SIZE, Tile, TileCoord, downsample};
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
    pyramid: [OnceLock<Box<Level>>; MAX_REDUCTION as usize],
    bounds: OnceLock<Option<IRect>>,
}

/// A level of an image's pyramid, and how many tiles it holds (each once, however many
/// places share it).
struct Level {
    image: RasterImage,
    distinct: usize,
}

/// A clone has the same pixels and starts with no pyramid: images are cloned (through
/// `Arc::make_mut`) to be changed, which would discard the pyramid at once.
impl Clone for RasterImage {
    fn clone(&self) -> RasterImage {
        RasterImage { tiles: self.tiles.clone(), pyramid: Default::default(), bounds: self.bounds.clone() }
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

    /// An image of `area` all pixel `p`. Tiles alike are one tile (those `area` covers
    /// whole, and those it covers alike in part: along each edge, and the corners), so even
    /// the largest area costs a few tiles.
    pub fn filled(area: &IRect, p: Pixel) -> RasterImage {
        // The tiles made so far, by the part of the tile they fill.
        let mut made: HashMap<[i64; 4], Arc<Tile>> = HashMap::new();
        let mut img = RasterImage::new();
        for c in TileCoord::covering(area, 0) {
            let r = c.rect(0);
            let part = area.intersect(&r);
            let key = [part.x0 - r.x0, part.y0 - r.y0, part.x1 - r.x0, part.y1 - r.y0];
            let tile = made.entry(key).or_insert_with(|| {
                let mut t = Tile::transparent();
                for y in key[1]..key[3] {
                    for x in key[0]..key[2] {
                        t.set(x as u32, y as u32, p);
                    }
                }
                Arc::new(t)
            });
            img.tiles.insert(c, tile.clone());
        }
        img
    }

    /// Builds an image from a row-major buffer of `width × height` pixels placed with its
    /// top-left corner at the canvas origin. Tiles that come out fully transparent are
    /// left out.
    pub fn from_pixels(width: u32, height: u32, pixels: &[Pixel]) -> RasterImage {
        let mut img = RasterImage::new();
        img.write_rect(&IRect::from_size(i64::from(width), i64::from(height)), pixels);
        img
    }

    /// Builds an image from `width × height` 8-bit sRGB pixels with straight alpha, row by
    /// row (as 8-bit files hold them), placed with its top-left corner at the canvas origin
    /// and kept at 8 bits (see [`Tile::from_srgb8`]). Tiles all clear are left out, and
    /// those of one colour throughout (a flat background) are one tile for each colour.
    pub fn from_srgb8(width: u32, height: u32, rgba: &[[u8; 4]]) -> RasterImage {
        let (w, h) = (width as usize, height as usize);
        assert_eq!(rgba.len(), w * h, "buffer size");
        let n = TILE_SIZE as usize;
        let coords: Vec<TileCoord> = TileCoord::covering(&IRect::from_size(w as i64, h as i64), 0).collect();
        let made: Vec<(TileCoord, Tile)> = coords
            .into_par_iter()
            .filter_map(|c| {
                let r = c.rect(0);
                let mut px = vec![[0u8; 4]; TILE_PIXELS];
                for y in r.y0 as usize..(r.y1 as usize).min(h) {
                    let row = &rgba[y * w + r.x0 as usize..y * w + (r.x1 as usize).min(w)];
                    px[(y - r.y0 as usize) * n..][..row.len()].copy_from_slice(row);
                }
                px.iter().any(|p| p[3] != 0).then(|| (c, Tile::from_srgb8(px)))
            })
            .collect();
        let mut flat: HashMap<[u8; 4], Arc<Tile>> = HashMap::new();
        let mut img = RasterImage::new();
        for (c, t) in made {
            let px = t.srgb8().expect("made at 8 bits");
            let tile = if px.iter().all(|p| *p == px[0]) {
                flat.entry(px[0]).or_insert_with(|| Arc::new(t)).clone()
            } else {
                Arc::new(t)
            };
            img.tiles.insert(c, tile);
        }
        img
    }

    /// The pixels of `rect`, row by row, as 8-bit sRGB with straight alpha, if every tile
    /// there keeps them so (clear where there is none): what copying them exactly needs.
    pub fn read_rect_srgb8(&self, rect: &IRect) -> Option<Vec<[u8; 4]>> {
        let (w, h) = (rect.width() as usize, rect.height() as usize);
        let mut out = vec![[0u8; 4]; w * h];
        for c in TileCoord::covering(rect, 0) {
            let Some(tile) = self.tiles.get(&c) else { continue };
            let px = tile.srgb8()?;
            let tr = c.rect(0);
            let r = tr.intersect(rect);
            let n = (r.x1 - r.x0) as usize;
            for y in r.y0..r.y1 {
                let start = (y - tr.y0) as usize * TILE_SIZE as usize + (r.x0 - tr.x0) as usize;
                let dst = (y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize;
                out[dst..dst + n].copy_from_slice(&px[start..start + n]);
            }
        }
        Some(out)
    }

    /// Replaces the pixels of `rect` with `pixels`, row by row. Tiles that come out fully
    /// transparent are removed.
    pub fn write_rect(&mut self, rect: &IRect, pixels: &[Pixel]) {
        let w = rect.width() as usize;
        assert_eq!(pixels.len(), w * rect.height() as usize, "buffer size");
        for c in TileCoord::covering(rect, 0) {
            let tr = c.rect(0);
            let r = tr.intersect(rect);
            let n = r.width() as usize;
            let tile = self.tile_mut(c);
            for y in r.y0..r.y1 {
                let src = &pixels[(y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize..][..n];
                let start = (y - tr.y0) as usize * TILE_SIZE as usize + (r.x0 - tr.x0) as usize;
                tile.pixels_mut()[start..start + n].copy_from_slice(src);
            }
            if tile.is_transparent() {
                self.tiles.remove(&c);
            }
        }
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
                let dst = (y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize;
                tile.read_row((y - tr.y0) as u32, (r.x0 - tr.x0) as u32, &mut out[dst..dst + n]);
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
    /// `(x, y)` covers this image's pixels from `(x·2^k, y·2^k)` on. Built on first use by
    /// halving the nearest level below that is built already, and kept until a pixel
    /// changes. Only the levels asked for are kept (a quarter of the image for level 1, a
    /// sixteenth for level 2), not the ones passed through on the way.
    pub fn reduced(&self, k: u32) -> &RasterImage {
        if k == 0 {
            return self;
        }
        let k = k.min(MAX_REDUCTION);
        let cell = &self.pyramid[k as usize - 1];
        if let Some(l) = cell.get() {
            return &l.image;
        }
        let from = (1..k).rev().find(|&j| self.has_reduced(j)).unwrap_or(0);
        let mut level = self.reduced(from).halved();
        for _ in from + 1..k {
            level = level.image.halved();
        }
        // Not `get_or_init`, for the reason given in `content_bounds`.
        let _ = cell.set(Box::new(level));
        &cell.get().expect("the level was just stored").image
    }

    /// The bytes the reduced copies built so far take, each tile once however many places
    /// share it (see `halved`).
    pub fn reduced_bytes(&self) -> u64 {
        self.pyramid.iter().filter_map(|c| c.get()).map(|l| l.distinct as u64).sum::<u64>() * TILE_BYTES
    }

    /// Whether level `k` of the pyramid has been built.
    pub fn has_reduced(&self, k: u32) -> bool {
        k == 0 || self.pyramid.get(k as usize - 1).is_some_and(|c| c.get().is_some())
    }

    /// The image at half size, each pixel the average of a 2×2 block. Tiles whose four
    /// children are the same tiles (a uniform area is full of them) are made once, and
    /// shared.
    fn halved(&self) -> Level {
        let parents: HashSet<TileCoord> = self.tiles.keys().map(|c| c.parent()).collect();
        // The parents by their children (by address, in the order `children` gives them).
        let mut alike: HashMap<[usize; 4], Vec<TileCoord>> = HashMap::new();
        for p in parents {
            let kids = p.children().map(|k| self.tiles.get(&k).map_or(0, |t| Arc::as_ptr(t) as usize));
            alike.entry(kids).or_default().push(p);
        }
        let made: Vec<(Vec<TileCoord>, Tile)> = alike
            .into_par_iter()
            .filter_map(|(_, at)| {
                let kids = at[0].children().map(|k| self.tiles.get(&k).map(|t| &**t));
                downsample(kids).map(|t| (at, t))
            })
            .collect();
        let mut out = RasterImage::new();
        let distinct = made.len();
        for (at, t) in made {
            let t = Arc::new(t);
            out.tiles.extend(at.into_iter().map(|c| (c, t.clone())));
        }
        Level { image: out, distinct }
    }
}

/// The size of one tile's worth of pixels in bytes (for memory accounting).
pub const TILE_BYTES: u64 = (TILE_SIZE * TILE_SIZE) as u64 * std::mem::size_of::<Pixel>() as u64;

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
        assert!(a.has_reduced(2) && !a.has_reduced(1), "only the level asked for is kept");
        assert_eq!(a.reduced(1).pixel(1, 0), pixel_from_f32([0.5, 0.5, 0.5, 1.0]));
        assert!(a.has_reduced(1) && a.has_reduced(2) && !a.has_reduced(3));
        a.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
        assert!(!a.has_reduced(1), "a change discards the pyramid");
        // Negative coordinates halve toward negative infinity, like tile coordinates.
        let mut b = RasterImage::new();
        b.set_pixel(-1, -1, pixel_from_f32([1.0; 4]));
        assert_eq!(b.reduced(1).pixel(-1, -1), pixel_from_f32([0.25; 4]));
        // Reduced many levels at once, a uniform area stays exactly the colour it was.
        let grey = pixel_from_f32([0.7, 0.7, 0.7, 1.0]);
        let tile = Arc::new(Tile::filled(grey));
        let mut c = RasterImage::new();
        for tc in TileCoord::covering(&IRect::from_size(1024, 1024), 0) {
            c.set_tile(tc, Some(tile.clone()));
        }
        assert_eq!(c.reduced(10).pixel(0, 0), grey);
        assert!(!c.has_reduced(9), "the levels passed through are not kept");
    }

    #[test]
    fn a_filled_area_is_a_few_tiles_however_large() {
        let p = pixel_from_f32([0.2, 0.4, 0.6, 1.0]);
        // Off the grid on every side: inside, four edges and four corners.
        let area = IRect::new(-100, 30, 5000, 3000);
        let img = RasterImage::filled(&area, p);
        let distinct: HashSet<*const Tile> = img.tiles().map(|(_, t)| Arc::as_ptr(t)).collect();
        assert_eq!(distinct.len(), 9);
        assert_eq!(img.content_bounds(), Some(area));
        assert_eq!((img.pixel(-100, 30), img.pixel(4999, 2999)), (p, p));
    }

    #[test]
    fn an_eight_bit_image_keeps_its_tiles_so_and_reads_as_working_pixels() {
        use crate::tile::pixel_from_srgb8;
        let (w, h) = (600u32, 600u32);
        // Clear on the right, one colour on the left, varied between.
        let rgba: Vec<[u8; 4]> = (0..w * h)
            .map(|i| {
                let x = i % w;
                if x < 256 {
                    [10, 20, 30, 255]
                } else if x >= 512 {
                    [0; 4]
                } else {
                    [x as u8, (i / w) as u8, 77, 200]
                }
            })
            .collect();
        let img = RasterImage::from_srgb8(w, h, &rgba);
        let wide = RasterImage::from_pixels(w, h, &rgba.iter().map(|&p| pixel_from_srgb8(p)).collect::<Vec<_>>());
        assert!(img.tiles().all(|(_, t)| t.is_srgb8()));
        assert_eq!(img.tile_count(), wide.tile_count(), "the clear tiles are left out as before");
        let whole = IRect::from_size(i64::from(w), i64::from(h));
        assert!(img.read_rect(&whole) == wide.read_rect(&whole));
        // The two tiles of one colour throughout are one.
        assert!(Arc::ptr_eq(img.tile(TileCoord::new(0, 0)).unwrap(), img.tile(TileCoord::new(0, 1)).unwrap()));
        assert_eq!(img.read_rect_srgb8(&whole).unwrap()[..(w * h) as usize], rgba[..]);
        assert!(wide.read_rect_srgb8(&whole).is_none(), "working pixels are not 8-bit");
    }

    #[test]
    fn halving_makes_each_tile_alike_once() {
        let grey = Arc::new(Tile::filled(pixel_from_f32([0.7, 0.7, 0.7, 1.0])));
        let other = Arc::new(Tile::filled(pixel_from_f32([0.1, 0.2, 0.3, 1.0])));
        let mut img = RasterImage::new();
        for tc in TileCoord::covering(&IRect::from_size(2048, 2048), 0) {
            img.set_tile(tc, Some(grey.clone()));
        }
        // One tile different: the parent it falls in is its own.
        img.set_tile(TileCoord::new(5, 2), Some(other));
        let half = img.reduced(1);
        let distinct: HashSet<*const Tile> = half.tiles().map(|(_, t)| Arc::as_ptr(t)).collect();
        assert_eq!((half.tile_count(), distinct.len()), (16, 2));
        assert_eq!(img.reduced_bytes(), 2 * TILE_BYTES, "the tiles shared counted once each");
        assert_eq!(half.pixel(0, 0), pixel_from_f32([0.7, 0.7, 0.7, 1.0]));
        assert_ne!(half.pixel(5 * 128, 2 * 128), half.pixel(0, 0));
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
