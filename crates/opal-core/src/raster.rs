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
/// half-size copies (for drawing it shrunk), which changing a pixel discards; and where one
/// tile repeats along each row (see [`alike_under`](RasterImage::alike_under)), which
/// follows each tile changed, and which a copy shares until either changes a row of it.
#[derive(Default)]
pub struct RasterImage {
    tiles: HashMap<TileCoord, Arc<Tile>>,
    /// `pyramid[k - 1]` is the image halved `k` times.
    pyramid: [OnceLock<Box<Level>>; MAX_REDUCTION as usize],
    bounds: OnceLock<Option<IRect>>,
    runs: OnceLock<Arc<Runs>>,
}

/// The tiles of an image along each tile row, as runs of one tile repeated: by row, the
/// runs `(x0, x1, tile's address)` in order (`x1` past the run's last tile), as long as
/// they go. Each row is shared with copies until changed.
#[derive(Clone, Default, Debug, PartialEq)]
struct Runs(HashMap<i32, Arc<Row>>);

/// A row's runs, `(x0, x1, tile's address)`, in order.
type Row = Vec<(i32, i32, usize)>;

/// What a row of tiles holds over a span of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Span {
    Empty,
    /// One tile throughout (its address).
    One(usize),
    /// Anything else.
    Mixed,
}

/// The spans `[x0, x1)` where rows `a` and `b` hold different tiles, joined where they meet.
fn row_differences(a: &Row, b: &Row) -> Vec<(i32, i32)> {
    // Between two neighbouring ends of runs, each row holds one tile throughout, or none.
    let mut cuts: Vec<i32> = a.iter().chain(b).flat_map(|&(x0, x1, _)| [x0, x1]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<(i32, i32)> = Vec::new();
    for w in cuts.windows(2) {
        if Runs::at(a, w[0]) != Runs::at(b, w[0]) {
            match out.last_mut() {
                Some(last) if last.1 == w[0] => last.1 = w[1],
                _ => out.push((w[0], w[1])),
            }
        }
    }
    out
}

impl Runs {
    fn of(tiles: &HashMap<TileCoord, Arc<Tile>>) -> Runs {
        let mut at: Vec<(i32, i32, usize)> = tiles.iter().map(|(c, t)| (c.y, c.x, Arc::as_ptr(t) as usize)).collect();
        at.sort_unstable();
        let mut rows: HashMap<i32, Row> = HashMap::new();
        for (y, x, p) in at {
            let row = rows.entry(y).or_default();
            match row.last_mut() {
                Some(last) if last.1 == x && last.2 == p => last.1 = x + 1,
                _ => row.push((x, x + 1, p)),
            }
        }
        Runs(rows.into_iter().map(|(y, row)| (y, Arc::new(row))).collect())
    }

    /// The tile at `c` is now the one at address `p` (`None`: none): the runs of its row
    /// are made again around it, split and joined as they now go.
    fn set(&mut self, c: TileCoord, p: Option<usize>) {
        let x = c.x;
        let row = Arc::make_mut(self.0.entry(c.y).or_default());
        // The runs holding `x`, or ending or starting next to it.
        let (lo, hi) = (row.partition_point(|r| r.1 < x), row.partition_point(|r| r.0 <= x + 1));
        let mut pieces: Vec<(i32, i32, usize)> = Vec::with_capacity(hi - lo + 1);
        for &(a, b, q) in &row[lo..hi] {
            if a < b.min(x) {
                pieces.push((a, b.min(x), q));
            }
            if b > a.max(x + 1) {
                pieces.push((a.max(x + 1), b, q));
            }
        }
        pieces.extend(p.map(|p| (x, x + 1, p)));
        pieces.sort_unstable();
        let mut joined: Vec<(i32, i32, usize)> = Vec::with_capacity(pieces.len());
        for r in pieces {
            match joined.last_mut() {
                Some(last) if last.1 == r.0 && last.2 == r.2 => last.1 = r.1,
                _ => joined.push(r),
            }
        }
        row.splice(lo..hi, joined);
        if row.is_empty() {
            self.0.remove(&c.y);
        }
    }

    /// The tile `row` holds at `x` (its address), if any.
    fn at(row: &[(i32, i32, usize)], x: i32) -> Option<usize> {
        row.get(row.partition_point(|r| r.1 <= x)).filter(|r| r.0 <= x).map(|r| r.2)
    }

    /// What `row` holds over `x0..x1`.
    fn span(row: &[(i32, i32, usize)], x0: i32, x1: i32) -> Span {
        if x1 <= x0 {
            return Span::Empty;
        }
        // The first run ending past `x0`: it holds `x0`, or lies after it.
        match row.get(row.partition_point(|r| r.1 <= x0)) {
            Some(&(rx0, rx1, p)) if rx0 <= x0 => {
                if rx1 >= x1 {
                    Span::One(p)
                } else {
                    Span::Mixed
                }
            }
            Some(r) if r.0 < x1 => Span::Mixed,
            _ => Span::Empty,
        }
    }

    /// Whether `f` holds of what every row of the rectangle `[x0, y0, x1, y1]` (tile
    /// coordinates) holds, a row without tiles holding nothing; asked while it holds. Given
    /// more rows than the image has, it visits the image's rows, and one empty row stands
    /// for the others.
    fn all_rows(&self, [x0, y0, x1, y1]: [i32; 4], mut f: impl FnMut(Span) -> bool) -> bool {
        if i64::from(y1) - i64::from(y0) <= self.0.len() as i64 {
            (y0..y1).all(|y| f(self.0.get(&y).map_or(Span::Empty, |row| Self::span(row, x0, x1))))
        } else {
            f(Span::Empty)
                && self.0.iter().filter(|(y, _)| (y0..y1).contains(*y)).all(|(_, row)| f(Self::span(row, x0, x1)))
        }
    }
}

/// A pixel as a tile of either kind stores it, for building images of it (see
/// `RasterImage::tiled`).
trait TilePixel: Copy + Send + Sync {
    /// The clear pixel.
    const CLEAR: Self;
    type Bits: std::hash::Hash + Eq;
    /// Its bits as stored: alike exactly when the pixels are, down to the sign of a zero.
    fn bits(&self) -> Self::Bits;
    /// Whether it is not fully transparent.
    fn shows(&self) -> bool;
    /// A tile of `TILE_PIXELS` of them, row by row.
    fn tile(px: Vec<Self>) -> Tile;
}

impl TilePixel for Pixel {
    const CLEAR: Pixel = [f16::ZERO; 4];
    type Bits = [u16; 4];
    fn bits(&self) -> [u16; 4] {
        self.map(f16::to_bits)
    }
    fn shows(&self) -> bool {
        crate::tile::shows(self)
    }
    fn tile(px: Vec<Pixel>) -> Tile {
        Tile::from_pixels(px)
    }
}

impl TilePixel for [u8; 4] {
    const CLEAR: [u8; 4] = [0; 4];
    type Bits = [u8; 4];
    fn bits(&self) -> [u8; 4] {
        *self
    }
    fn shows(&self) -> bool {
        crate::tile::shows_srgb8(self)
    }
    fn tile(px: Vec<[u8; 4]>) -> Tile {
        Tile::from_srgb8(px)
    }
}

/// Up to how many tiles a question about a rectangle of them looks at each tile, rather than
/// at the runs (which are built for it, at the cost of sorting every tile of the image).
const LOOKED_AT_ONE_BY_ONE: i64 = 64;

/// The tiles any pixel of `area` lies in, as `[x0, y0, x1, y1]` (tile coordinates, the
/// ends past the last).
fn tiles_under(area: &IRect) -> [i32; 4] {
    if area.is_empty() {
        return [0; 4];
    }
    let t = i64::from(TILE_SIZE);
    let at = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    [
        at(area.x0.div_euclid(t)),
        at(area.y0.div_euclid(t)),
        at((area.x1 - 1).div_euclid(t) + 1),
        at((area.y1 - 1).div_euclid(t) + 1),
    ]
}

/// How many tiles `[x0, y0, x1, y1]` holds.
fn cells([x0, y0, x1, y1]: [i32; 4]) -> i64 {
    (i64::from(x1) - i64::from(x0)).max(0) * (i64::from(y1) - i64::from(y0)).max(0)
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
        RasterImage {
            tiles: self.tiles.clone(),
            pyramid: Default::default(),
            bounds: self.bounds.clone(),
            runs: self.runs.clone(),
        }
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

    /// Forgets what was derived from the pixels, which are about to change. The runs of
    /// tiles are kept, and told of each tile placed ([`placed`](Self::placed)).
    fn changed(&mut self) {
        self.pyramid = Default::default();
        self.bounds = OnceLock::new();
    }

    /// The tile map holds the tile at address `p` at `c` (or, `None`, none): the runs follow,
    /// if built.
    fn placed(&mut self, c: TileCoord, p: Option<usize>) {
        if let Some(runs) = self.runs.get_mut() {
            Arc::make_mut(runs).set(c, p);
        }
    }

    /// Whether the tiles under `area` (pixels) are alike: one tile at every place, or none
    /// at any. A large area is asked of the runs of tiles along each row (built on first
    /// use), at a lookup a row however wide.
    pub fn alike_under(&self, area: &IRect) -> bool {
        let r = tiles_under(area);
        if cells(r) <= LOOKED_AT_ONE_BY_ONE {
            return self.alike_one_by_one(r);
        }
        let mut first = None;
        self.runs().all_rows(r, |s| s != Span::Mixed && *first.get_or_insert(s) == s)
    }

    /// The tile under every place of `area` (pixels), `Some(None)` if none is under any, or
    /// `None` if they are not alike (see [`alike_under`](Self::alike_under)).
    pub fn alike_tile_under(&self, area: &IRect) -> Option<Option<&Arc<Tile>>> {
        if !self.alike_under(area) {
            return None;
        }
        let [x0, y0, x1, y1] = tiles_under(area);
        Some(if x1 > x0 && y1 > y0 { self.tiles.get(&TileCoord::new(x0, y0)) } else { None })
    }

    /// Whether any tile lies under `area` (pixels). A large area is asked of the runs, as
    /// for [`alike_under`](Self::alike_under).
    pub fn any_under(&self, area: &IRect) -> bool {
        let r = tiles_under(area);
        if cells(r) <= LOOKED_AT_ONE_BY_ONE {
            return self.any_one_by_one(r);
        }
        !self.runs().all_rows(r, |s| s == Span::Empty)
    }

    fn runs(&self) -> &Runs {
        self.runs.get_or_init(|| Arc::new(Runs::of(&self.tiles)))
    }

    /// Where the image has tiles: by tile row, the spans `[x0, x1)` of tile columns along
    /// which they run unbroken, as `(row, x0, x1)`. Asked of the runs, at a step a run.
    pub fn spans(&self) -> Vec<(i32, i32, i32)> {
        let mut out: Vec<(i32, i32, i32)> = Vec::new();
        for (&y, row) in &self.runs().0 {
            for &(x0, x1, _) in row.iter() {
                match out.last_mut() {
                    Some(last) if last.0 == y && last.2 == x0 => last.2 = x1,
                    _ => out.push((y, x0, x1)),
                }
            }
        }
        out
    }

    /// Where `self` and `other` hold different tiles (by identity: an edit replaces the
    /// tiles it changes and keeps the others): by tile row, the spans `[x0, x1)` of tile
    /// columns. Asked of the two images' runs, whose rows an edit left alone are shared
    /// between an image and its edited copy, so the cost follows the rows that changed.
    pub fn differences(&self, other: &RasterImage) -> HashMap<i32, Vec<(i32, i32)>> {
        let (a, b) = (self.runs(), other.runs());
        let mut out = HashMap::new();
        let none = Row::new();
        let mut differ = |y: i32, ra: &Row, rb: &Row| {
            let spans = row_differences(ra, rb);
            if !spans.is_empty() {
                out.insert(y, spans);
            }
        };
        for (&y, ra) in &a.0 {
            match b.0.get(&y) {
                Some(rb) if Arc::ptr_eq(ra, rb) => {}
                rb => differ(y, ra, rb.map_or(&none, |r| r)),
            }
        }
        for (&y, rb) in &b.0 {
            if !a.0.contains_key(&y) {
                differ(y, &none, rb);
            }
        }
        out
    }

    fn alike_one_by_one(&self, [x0, y0, x1, y1]: [i32; 4]) -> bool {
        let mut first = None;
        (y0..y1).all(|y| {
            (x0..x1).all(|x| {
                let here = self.tiles.get(&TileCoord::new(x, y)).map(|t| Arc::as_ptr(t) as usize);
                *first.get_or_insert(here) == here
            })
        })
    }

    fn any_one_by_one(&self, [x0, y0, x1, y1]: [i32; 4]) -> bool {
        (y0..y1).any(|y| (x0..x1).any(|x| self.tiles.contains_key(&TileCoord::new(x, y))))
    }

    /// The tile at `c`, if it exists.
    pub fn tile(&self, c: TileCoord) -> Option<&Arc<Tile>> {
        self.tiles.get(&c)
    }

    /// The tile at `c` for changing, created transparent if absent and copied first if it is
    /// shared with another image or history state.
    pub fn tile_mut(&mut self, c: TileCoord) -> &mut Tile {
        self.changed();
        // This image's own first, so that the runs know where it is; changed after, it is
        // still one tile of its own there.
        let tile = self.tiles.entry(c).or_insert_with(|| Arc::new(Tile::transparent()));
        Arc::make_mut(tile);
        if let Some(runs) = self.runs.get_mut() {
            Arc::make_mut(runs).set(c, Some(Arc::as_ptr(tile) as usize));
        }
        Arc::get_mut(tile).expect("this image's own")
    }

    /// Sets or removes the tile at `c`.
    pub fn set_tile(&mut self, c: TileCoord, tile: Option<Arc<Tile>>) {
        self.changed();
        let p = tile.as_ref().map(|t| Arc::as_ptr(t) as usize);
        match tile {
            Some(t) => {
                self.tiles.insert(c, t);
            }
            None => {
                self.tiles.remove(&c);
            }
        }
        self.placed(c, p);
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
        self.runs = OnceLock::new();
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
                {
                    let mut px = t.pixels_mut();
                    for y in key[1]..key[3] {
                        let row = y as usize * TILE_SIZE as usize;
                        px[row + key[0] as usize..row + key[2] as usize].fill(p);
                    }
                }
                Arc::new(t)
            });
            img.tiles.insert(c, tile.clone());
        }
        img
    }

    /// Builds an image from a row-major buffer of `width × height` pixels placed with its
    /// top-left corner at the canvas origin, as [`from_rows`](Self::from_rows) does.
    pub fn from_pixels(width: u32, height: u32, pixels: &[Pixel]) -> RasterImage {
        let w = width as usize;
        assert_eq!(pixels.len(), w * height as usize, "buffer size");
        RasterImage::from_rows(width, height, |y, x0, out| out.copy_from_slice(&pixels[y * w + x0..][..out.len()]))
    }

    /// Builds an image of `width × height` pixels placed with its top-left corner at the
    /// canvas origin, whose rows `row` gives: row `y` from column `x0`, into `out` (as many
    /// as it holds), for each tile in parallel, so that no copy of the whole is needed.
    /// Tiles all clear are left out, and those of one pixel value throughout (a flat
    /// background) are one tile for each value.
    pub fn from_rows(width: u32, height: u32, row: impl Fn(usize, usize, &mut [Pixel]) + Sync) -> RasterImage {
        RasterImage::tiled(width, height, row)
    }

    /// Builds an image from `width × height` 8-bit sRGB pixels with straight alpha, row by
    /// row (as 8-bit files hold them), placed with its top-left corner at the canvas origin
    /// and kept at 8 bits (see [`Tile::from_srgb8`]), as [`from_srgb8_rows`] does.
    ///
    /// [`from_srgb8_rows`]: Self::from_srgb8_rows
    pub fn from_srgb8(width: u32, height: u32, rgba: &[[u8; 4]]) -> RasterImage {
        let w = width as usize;
        assert_eq!(rgba.len(), w * height as usize, "buffer size");
        RasterImage::from_srgb8_rows(width, height, |y, x0, out| out.copy_from_slice(&rgba[y * w + x0..][..out.len()]))
    }

    /// [`from_rows`](Self::from_rows) for 8-bit sRGB pixels with straight alpha, kept at 8
    /// bits.
    pub fn from_srgb8_rows(width: u32, height: u32, row: impl Fn(usize, usize, &mut [[u8; 4]]) + Sync) -> RasterImage {
        RasterImage::tiled(width, height, row)
    }

    /// The image `row` gives (see [`from_rows`](Self::from_rows)), of tiles of pixels `P`
    /// (clear past the image's edges).
    fn tiled<P: TilePixel>(width: u32, height: u32, row: impl Fn(usize, usize, &mut [P]) + Sync) -> RasterImage {
        let (w, h) = (width as usize, height as usize);
        let n = TILE_SIZE as usize;
        let coords: Vec<TileCoord> = TileCoord::covering(&IRect::from_size(w as i64, h as i64), 0).collect();
        // A tile of one value is told by its pixels before it is made: made, past the
        // memory budget, it could be on disk by the time it was looked at.
        enum Made<P> {
            Flat(P),
            Own(Tile),
        }
        let made: Vec<(TileCoord, Made<P>)> = coords
            .into_par_iter()
            .filter_map(|c| {
                let r = c.rect(0);
                let (x0, x1) = (r.x0 as usize, (r.x1 as usize).min(w));
                let mut px = vec![P::CLEAR; TILE_PIXELS];
                for y in r.y0 as usize..(r.y1 as usize).min(h) {
                    row(y, x0, &mut px[(y - r.y0 as usize) * n..][..x1 - x0]);
                }
                if !px.iter().any(|p| p.shows()) {
                    return None;
                }
                let first = px[0].bits();
                let flat = px.iter().all(|p| p.bits() == first).then_some(px[0]);
                Some((c, flat.map_or_else(|| Made::Own(P::tile(px)), Made::Flat)))
            })
            .collect();
        let mut flat: HashMap<P::Bits, Arc<Tile>> = HashMap::new();
        let mut img = RasterImage::new();
        for (c, made) in made {
            let tile = match made {
                Made::Flat(p) => {
                    flat.entry(p.bits()).or_insert_with(|| Arc::new(P::tile(vec![p; TILE_PIXELS]))).clone()
                }
                Made::Own(t) => Arc::new(t),
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
            let reader = tile.reader();
            let px = reader.srgb8()?;
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
            {
                // Taken once a tile: each taking locks it, and may make room for it.
                let mut px = tile.pixels_mut();
                for y in r.y0..r.y1 {
                    let src = &pixels[(y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize..][..n];
                    let start = (y - tr.y0) as usize * TILE_SIZE as usize + (r.x0 - tr.x0) as usize;
                    px[start..start + n].copy_from_slice(src);
                }
            }
            if tile.is_transparent() {
                self.set_tile(c, None);
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
            let reader = tile.reader();
            for y in r.y0..r.y1 {
                let dst = (y - rect.y0) as usize * w + (r.x0 - rect.x0) as usize;
                reader.read_row((y - tr.y0) as u32, (r.x0 - tr.x0) as u32, &mut out[dst..dst + n]);
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
    fn the_runs_of_tiles_say_what_looking_at_each_tile_says() {
        let white = Arc::new(Tile::filled(pixel_from_f32([1.0; 4])));
        let blue = Arc::new(Tile::filled(pixel_from_f32([0.0, 0.0, 1.0, 1.0])));
        // Rows of one tile, of another, of both, with gaps, and missing; the white tile also
        // where blue is at the end of a row (runs of one tile, apart).
        let mut img = RasterImage::new();
        for (y, row) in
            ["wwwwwwww", "wwwwwwww", "bbbbbbbb", "", "wwwbwwww", "  www   ", "wwwwwwww", "www  www", "bbbbwwww"]
                .iter()
                .enumerate()
        {
            for (x, t) in row.chars().enumerate() {
                let tile = match t {
                    'w' => white.clone(),
                    'b' => blue.clone(),
                    _ => continue,
                };
                img.set_tile(TileCoord::new(x as i32, y as i32), Some(tile));
            }
        }
        let runs = Runs::of(&img.tiles);
        for y0 in -2..11 {
            for y1 in y0..12 {
                for x0 in -2..10 {
                    for x1 in x0..11 {
                        let r = [x0, y0, x1, y1];
                        let mut first = None;
                        let alike = runs.all_rows(r, |s| s != Span::Mixed && *first.get_or_insert(s) == s);
                        assert_eq!(alike, img.alike_one_by_one(r), "alike over {r:?}");
                        let any = !runs.all_rows(r, |s| s == Span::Empty);
                        assert_eq!(any, img.any_one_by_one(r), "any over {r:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_image_built_from_pixels_shares_flat_tiles_leaves_clear_ones_out_and_keeps_every_bit() {
        // 3 × 2 tiles: the first column clear, the last flat, the middle each pixel its own,
        // with zeros of both signs; and one flat tile alike but for a zero's sign.
        let (w, h) = (256 * 3, 256 * 2);
        let flat = pixel_from_f32([0.5, 0.0, 0.25, 1.0]);
        let mut minus = flat;
        minus[1] = f16::NEG_ZERO;
        let px: Vec<Pixel> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match x / 256 {
                    0 => [f16::ZERO; 4],
                    1 => [
                        f16::from_f32(x as f32 / 1000.0),
                        if y % 2 == 0 { f16::NEG_ZERO } else { f16::ZERO },
                        f16::ONE,
                        f16::ONE,
                    ],
                    _ if y >= 256 && x == 600 && y == 300 => minus,
                    _ => flat,
                }
            })
            .collect();
        let img = RasterImage::from_pixels(w as u32, h as u32, &px);
        let mut want = RasterImage::new();
        let all = IRect::from_size(w as i64, h as i64);
        want.write_rect(&all, &px);
        let bits = |p: &[Pixel]| p.iter().map(|p| p.map(f16::to_bits)).collect::<Vec<_>>();
        assert_eq!(bits(&img.read_rect(&all)), bits(&px), "every bit, both zeros kept");
        let coords = |i: &RasterImage| i.tiles().map(|(c, _)| c).collect::<HashSet<TileCoord>>();
        assert_eq!(coords(&img), coords(&want), "the clear tiles left out, and only those");
        let (top, bottom) = (img.tile(TileCoord::new(2, 0)).unwrap(), img.tile(TileCoord::new(2, 1)).unwrap());
        assert!(!Arc::ptr_eq(top, bottom), "a tile alike but for a zero's sign was taken as flat");
        let (w2, h2) = (256 * 2, 256);
        let two = RasterImage::from_pixels(w2 as u32, h2 as u32, &vec![flat; w2 * h2]);
        let (a, b) = (two.tile(TileCoord::new(0, 0)).unwrap(), two.tile(TileCoord::new(1, 0)).unwrap());
        assert!(Arc::ptr_eq(a, b), "flat tiles of one value are one tile");
    }

    #[test]
    fn the_runs_follow_each_tile_changed_as_if_made_again() {
        let white = Arc::new(Tile::filled(pixel_from_f32([1.0; 4])));
        let blue = Arc::new(Tile::filled(pixel_from_f32([0.0, 0.0, 1.0, 1.0])));
        let mut img = RasterImage::filled(&IRect::from_size(256 * 12, 256 * 3), pixel_from_f32([1.0; 4]));
        img.set_tile(TileCoord::new(20, 1), Some(white.clone()));
        let _ = img.runs();
        // Placed, removed, changed and put back, across a row, at its ends and past them.
        let steps: Vec<(i32, i32, u8)> = vec![
            (5, 0, 1),
            (6, 0, 1),
            (4, 0, 1),
            (5, 0, 0),
            (0, 0, 2),
            (11, 0, 2),
            (12, 0, 0),
            (13, 0, 0),
            (-1, 1, 0),
            (0, 1, 3),
            (5, 1, 3),
            (19, 1, 0),
            (21, 1, 1),
            (20, 1, 2),
            (3, 7, 1),
            (3, 7, 2),
            (4, 7, 1),
            (6, 2, 3),
            (6, 2, 0),
            (7, 2, 2),
            (8, 2, 2),
            (9, 2, 2),
            (8, 2, 1),
        ];
        let shared = img.clone();
        for (x, y, what) in steps {
            let c = TileCoord::new(x, y);
            match what {
                0 => img.set_tile(c, Some(white.clone())),
                1 => img.set_tile(c, Some(blue.clone())),
                2 => img.set_tile(c, None),
                _ => img.tile_mut(c).set(1, 1, pixel_from_f32([0.5; 4])),
            }
            assert_eq!(**img.runs.get().expect("kept"), Runs::of(&img.tiles), "after {what} at {c:?}");
        }
        assert_eq!(**shared.runs.get().expect("shared"), Runs::of(&shared.tiles), "a copy's runs changed");
        img.set_tile(TileCoord::new(2, 2), Some(Arc::new(Tile::transparent())));
        img.prune_transparent();
        assert!(img.tile(TileCoord::new(2, 2)).is_none());
        assert_eq!(*img.runs(), Runs::of(&img.tiles));
    }

    #[test]
    fn two_images_differ_where_they_hold_different_tiles() {
        let blue = Arc::new(Tile::filled(pixel_from_f32([0.0, 0.0, 1.0, 1.0])));
        let old = RasterImage::filled(&IRect::from_size(256 * 12, 256 * 3), pixel_from_f32([1.0; 4]));
        let _ = old.runs();
        // An edited copy, as a layer is edited: a tile changed, two placed side by side, one
        // removed, one placed far off.
        let mut new = old.clone();
        new.tile_mut(TileCoord::new(3, 0)).set(1, 1, pixel_from_f32([0.5; 4]));
        new.set_tile(TileCoord::new(5, 1), Some(blue.clone()));
        new.set_tile(TileCoord::new(6, 1), Some(blue.clone()));
        new.set_tile(TileCoord::new(11, 2), None);
        new.set_tile(TileCoord::new(-4, 9), Some(blue));
        let want = HashMap::from([(0, vec![(3, 4)]), (1, vec![(5, 7)]), (2, vec![(11, 12)]), (9, vec![(-4, -3)])]);
        assert_eq!(new.differences(&old), want);
        assert_eq!(old.differences(&new), want);
        // With runs made afresh from the tiles, sharing no row with the other's: the same.
        let fresh = RasterImage { tiles: new.tiles.clone(), ..RasterImage::new() };
        assert_eq!(fresh.differences(&old), want);
        assert!(old.differences(&old.clone()).is_empty());
    }

    #[test]
    fn an_area_is_alike_when_one_tile_or_none_is_under_every_place() {
        let white = pixel_from_f32([1.0; 4]);
        let mut img = RasterImage::filled(&IRect::from_size(256 * 20, 256 * 12), white);
        let tiles = |x0: i64, y0: i64, x1: i64, y1: i64| IRect::new(x0 * 256, y0 * 256, x1 * 256, y1 * 256);
        // Small areas are looked at tile by tile, large ones asked of the runs: both kinds.
        for (area, alike, any) in [
            (tiles(0, 0, 20, 12), true, true),
            (tiles(3, 2, 7, 5), true, true),
            (IRect::new(300, 300, 301, 301), true, true),
            (tiles(30, 0, 60, 12), true, false),
            (tiles(18, 0, 22, 2), false, true),
            (tiles(-12, -12, 0, 0), true, false),
            (tiles(-12, -12, 1, 1), false, true),
            (IRect::new(-1, 0, 1, 1), false, true),
        ] {
            assert_eq!((img.alike_under(&area), img.any_under(&area)), (alike, any), "{area:?}");
        }
        // A tile of its own in the middle.
        img.set_pixel(256 * 10 + 3, 256 * 6 + 3, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        assert!(!img.alike_under(&tiles(0, 0, 20, 12)), "the runs were not built again");
        assert!(img.alike_under(&tiles(0, 0, 10, 12)) && img.alike_under(&tiles(11, 0, 20, 12)));
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
