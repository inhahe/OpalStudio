//! A selection's mask (design.md, *Selections*, *The mask*): how selected each pixel of the
//! plane is, from 0 (not at all) to 1 (fully), in 16-bit levels ([`FULL`] is 1).
//!
//! It is kept in tiles on the images' pixel grid ([`TILE_SIZE`]), so that a selection across a
//! huge canvas costs its edges, not its area:
//!
//! - tiles of the mask's level *outside* (0, or 1 once inverted, so that a mask can select the
//!   whole plane but for a hole) are not kept;
//! - tiles side by side all of one level are kept as one run along their row of tiles (within
//!   a rectangle's edges, one run a row);
//! - a tile whose levels change from column to column alone (or from row to row), as along a
//!   straight edge on the pixel grid, keeps one level a column (or a row); only the others
//!   keep one a pixel.
//!
//! Copies share their tiles until one changes, as the history's states share images'.
//!
//! Every operation limited to a selection goes through one step, [`blend`]: the pixel it would
//! make, weighed against the one there by the mask's level.

use crate::geom::{Affine, FRect, IRect};
use crate::raster::RasterImage;
use crate::tile::{PixelF32, TILE_PIXELS, TILE_SIZE, Tile, TileCoord, pixel_from_f32};
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The level of a pixel fully selected.
pub const FULL: u16 = u16::MAX;

/// Where the marching ants run by default: where the mask crosses half.
pub const HALF: u16 = FULL / 2 + 1;

/// `level` as a fraction of 1.
pub fn fraction(level: u16) -> f32 {
    f32::from(level) / f32::from(FULL)
}

/// The level nearest the fraction `f` (taken as 0 or 1 past them).
pub fn level(f: f32) -> u16 {
    (f.clamp(0.0, 1.0) * f32::from(FULL)).round() as u16
}

/// A tile's side, in pixels, as an index.
const SIDE: usize = TILE_SIZE as usize;

/// A tile of a mask: all one level; one level a column, or a row, where the levels change
/// along one way only; or one level a pixel, row by row.
#[derive(Clone, Debug, PartialEq)]
pub enum MaskTile {
    Level(u16),
    /// The same down each column: a level a column, left to right.
    Columns(Arc<[u16]>),
    /// The same along each row: a level a row, top to bottom.
    Rows(Arc<[u16]>),
    Levels(Arc<[u16]>),
}

/// Which way a tile's levels change, when they change along one way only.
#[derive(Clone, Copy)]
enum Line {
    /// From column to column.
    Columns,
    /// From row to row.
    Rows,
}

impl MaskTile {
    /// The level of the tile's pixel `(x, y)`.
    pub fn get(&self, x: u32, y: u32) -> u16 {
        match self {
            MaskTile::Level(l) => *l,
            MaskTile::Columns(l) => l[x as usize],
            MaskTile::Rows(l) => l[y as usize],
            MaskTile::Levels(l) => l[(y * TILE_SIZE + x) as usize],
        }
    }

    /// The tile `levels` (a tile's worth, row by row) make, kept as compactly as they allow.
    fn from_levels(levels: Vec<u16>) -> MaskTile {
        debug_assert_eq!(levels.len(), TILE_PIXELS);
        let first = &levels[..SIDE];
        if levels.chunks_exact(SIDE).all(|row| row == first) {
            return MaskTile::from_line(Line::Columns, first.to_vec());
        }
        if levels.chunks_exact(SIDE).all(|row| row.iter().all(|&l| l == row[0])) {
            return MaskTile::from_line(Line::Rows, levels.chunks_exact(SIDE).map(|row| row[0]).collect());
        }
        MaskTile::Levels(levels.into())
    }

    /// The tile whose levels `line` gives, one a column or one a row as `along` says: one
    /// level alone where all are alike.
    fn from_line(along: Line, line: Vec<u16>) -> MaskTile {
        debug_assert_eq!(line.len(), SIDE);
        if line.iter().all(|&l| l == line[0]) {
            return MaskTile::Level(line[0]);
        }
        match along {
            Line::Columns => MaskTile::Columns(line.into()),
            Line::Rows => MaskTile::Rows(line.into()),
        }
    }

    /// Its levels, one a column or one a row as `along` says, if it can be kept so.
    fn line(&self, along: Line) -> Option<Cow<'_, [u16]>> {
        match (self, along) {
            (MaskTile::Level(l), _) => Some(Cow::Owned(vec![*l; SIDE])),
            (MaskTile::Columns(l), Line::Columns) | (MaskTile::Rows(l), Line::Rows) => Some(Cow::Borrowed(l)),
            _ => None,
        }
    }

    /// Its levels each made `f` of themselves.
    fn map(&self, f: impl Fn(u16) -> u16) -> MaskTile {
        match self {
            MaskTile::Level(l) => MaskTile::Level(f(*l)),
            MaskTile::Columns(l) => MaskTile::from_line(Line::Columns, l.iter().map(|&v| f(v)).collect()),
            MaskTile::Rows(l) => MaskTile::from_line(Line::Rows, l.iter().map(|&v| f(v)).collect()),
            MaskTile::Levels(l) => MaskTile::from_levels(l.iter().map(|&v| f(v)).collect()),
        }
    }

    /// `a` and `b` combined pixel by pixel, as `how` says: one of them itself (its levels
    /// shared) where the other, all one level, leaves it as it is.
    fn combined(a: &MaskTile, b: &MaskTile, how: Combine) -> MaskTile {
        match (a, b) {
            (MaskTile::Level(x), MaskTile::Level(y)) => return MaskTile::Level(how.level(*x, *y)),
            (_, MaskTile::Level(y)) if how.keeps_before(*y) => return a.clone(),
            (MaskTile::Level(x), _) if how.keeps_new(*x) => return b.clone(),
            _ => {}
        }
        for along in [Line::Columns, Line::Rows] {
            if let (Some(la), Some(lb)) = (a.line(along), b.line(along)) {
                return MaskTile::from_line(along, la.iter().zip(lb.iter()).map(|(&x, &y)| how.level(x, y)).collect());
            }
        }
        let (la, lb) = (a.levels(), b.levels());
        MaskTile::from_levels(la.iter().zip(lb.iter()).map(|(&x, &y)| how.level(x, y)).collect())
    }

    /// The memory its levels take (none for a tile of one level).
    pub fn bytes(&self) -> u64 {
        match self {
            MaskTile::Level(_) => 0,
            MaskTile::Columns(l) | MaskTile::Rows(l) | MaskTile::Levels(l) => {
                (l.len() * std::mem::size_of::<u16>()) as u64
            }
        }
    }

    /// Where its levels are kept, if it keeps any (the same for its copies).
    fn address(&self) -> Option<usize> {
        match self {
            MaskTile::Level(_) => None,
            MaskTile::Columns(l) | MaskTile::Rows(l) | MaskTile::Levels(l) => Some(Arc::as_ptr(l).cast::<u16>() as usize),
        }
    }

    /// Its levels, a tile's worth, row by row.
    fn levels(&self) -> Cow<'_, [u16]> {
        match self {
            MaskTile::Levels(l) => Cow::Borrowed(l),
            _ => Cow::Owned((0..TILE_SIZE).flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y))).map(|(x, y)| self.get(x, y)).collect()),
        }
    }

    /// The smallest rectangle (from the tile's corner) holding all its pixels above 0, if any is.
    fn selected_part(&self) -> Option<IRect> {
        let span = |l: &[u16]| -> Option<(i64, i64)> {
            let first = l.iter().position(|&v| v > 0)?;
            let last = l.iter().rposition(|&v| v > 0).expect("one is above 0");
            Some((first as i64, last as i64 + 1))
        };
        let side = i64::from(TILE_SIZE);
        match self {
            MaskTile::Level(0) => None,
            MaskTile::Level(_) => Some(IRect::new(0, 0, side, side)),
            MaskTile::Columns(l) => span(l).map(|(x0, x1)| IRect::new(x0, 0, x1, side)),
            MaskTile::Rows(l) => span(l).map(|(y0, y1)| IRect::new(0, y0, side, y1)),
            MaskTile::Levels(l) => {
                let mut part: Option<IRect> = None;
                for (y, row) in l.chunks_exact(SIDE).enumerate() {
                    let Some((x0, x1)) = span(row) else { continue };
                    let y = y as i64;
                    let r = IRect::new(x0, y, x1, y + 1);
                    part = Some(part.map_or(r, |p| p.union(&r)));
                }
                part
            }
        }
    }
}

/// How much of the plane a mask selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// Nothing at all.
    Nothing,
    /// Nothing outside this rectangle (the smallest that holds every pixel selected at all).
    Within(IRect),
    /// Without bound: every pixel far enough out (the mask has been inverted).
    Unbounded,
}

/// How a mask made by a step combines with the selection before it (design.md, *The
/// recipe*).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    /// It is the selection; the one before goes.
    #[default]
    Replace,
    /// Selected where either selects (the greater level).
    Add,
    /// Selected where the one before selects and it does not (the lesser of the one
    /// before's level and the opposite of its).
    Subtract,
    /// Selected where both select (the lesser level).
    Intersect,
}

impl Combine {
    /// Whether a new mask's level `new` leaves every level before as it is.
    fn keeps_before(self, new: u16) -> bool {
        match self {
            Combine::Replace => false,
            Combine::Add | Combine::Subtract => new == 0,
            Combine::Intersect => new == FULL,
        }
    }

    /// Whether the level before, `before`, leaves every new mask's level as it is.
    fn keeps_new(self, before: u16) -> bool {
        match self {
            Combine::Replace => true,
            Combine::Add => before == 0,
            Combine::Subtract => false,
            Combine::Intersect => before == FULL,
        }
    }

    /// The level a pixel gets where the selection before has `before` and the new mask `new`.
    pub fn level(self, before: u16, new: u16) -> u16 {
        match self {
            Combine::Replace => new,
            Combine::Add => before.max(new),
            Combine::Subtract => before.min(FULL - new),
            Combine::Intersect => before.min(new),
        }
    }
}

/// What a mask can be made of: how much of each pixel a shape covers.
pub trait Coverage: Sync {
    /// The pixels outside of which it covers nothing.
    fn bounds(&self) -> IRect;

    /// The distance from `(x, y)` to its edge, less than 0 inside it: never farther from the
    /// edge than `(x, y)` truly is, so that a whole area can be told inside or outside from its
    /// middle.
    fn distance(&self, x: f64, y: f64) -> f64;

    /// How much of the pixel whose middle is `(x, y)` it covers, from 0 to 1.
    fn at(&self, x: f64, y: f64) -> f32;

    /// Whether it is convex: along a row, it then covers every tile between two it covers
    /// wholly, which spares looking at them.
    fn convex(&self) -> bool {
        false
    }
}

/// Tiles side by side along a row of tiles, `x0..x1` (in tiles), all `tile`: only one tile,
/// unless it is all one level.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    x0: i32,
    x1: i32,
    tile: MaskTile,
}

/// A selection's mask: see the module's description.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mask {
    /// The rows of tiles with tiles of other levels than `outside`, by their y (in tiles): each
    /// one's runs, left to right, apart, none of the level `outside`, and no two of one level
    /// touching (so that masks alike are kept alike).
    rows: BTreeMap<i32, Vec<Run>>,
    outside: u16,
}

/// Puts `tile` at tiles `x0..x1` on the right of `row`, a row of a mask whose level outside is
/// `outside`, keeping the order [`Mask::rows`] keeps.
fn push(row: &mut Vec<Run>, x0: i32, x1: i32, tile: MaskTile, outside: u16) {
    if x0 >= x1 || tile == MaskTile::Level(outside) {
        return;
    }
    debug_assert!(x1 - x0 == 1 || matches!(tile, MaskTile::Level(_)), "a run of tiles with levels of their own");
    debug_assert!(row.last().is_none_or(|r| r.x1 <= x0), "put out of order");
    if let Some(last) = row.last_mut()
        && last.x1 == x0
        && matches!(tile, MaskTile::Level(_))
        && last.tile == tile
    {
        last.x1 = x1;
    } else {
        row.push(Run { x0, x1, tile });
    }
}

/// The pieces two rows of tiles, `a` and `b`, part each other into, left to right, where
/// either keeps a tile: `(x0, x1, a's tile, b's tile)`, every tile of a piece alike in each
/// (`a_out` or `b_out` where one keeps none), so a piece is one tile wide unless both are all
/// one level there.
fn pieces<'a>(
    a: &'a [Run],
    b: &'a [Run],
    a_out: &'a MaskTile,
    b_out: &'a MaskTile,
) -> Vec<(i32, i32, &'a MaskTile, &'a MaskTile)> {
    let mut cuts: Vec<i32> = a.iter().chain(b).flat_map(|r| [r.x0, r.x1]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(cuts.len());
    for w in cuts.windows(2) {
        let (x0, x1) = (w[0], w[1]);
        while i < a.len() && a[i].x1 <= x0 {
            i += 1;
        }
        while j < b.len() && b[j].x1 <= x0 {
            j += 1;
        }
        let ta = a.get(i).filter(|r| r.x0 <= x0).map(|r| &r.tile);
        let tb = b.get(j).filter(|r| r.x0 <= x0).map(|r| &r.tile);
        if ta.is_some() || tb.is_some() {
            out.push((x0, x1, ta.unwrap_or(a_out), tb.unwrap_or(b_out)));
        }
    }
    out
}

/// A row of tiles of no runs.
const NO_RUNS: &[Run] = &[];

impl Mask {
    /// A mask that selects nothing.
    pub fn none() -> Mask {
        Mask::default()
    }

    /// A mask that selects the whole plane.
    pub fn all() -> Mask {
        Mask { rows: BTreeMap::new(), outside: FULL }
    }

    /// The level of every pixel no tile is kept for.
    pub fn outside(&self) -> u16 {
        self.outside
    }

    /// The mask of `rows` (each a row of tiles' runs, in the order [`Mask::rows`] keeps) and
    /// the level `outside`, alike tiles of a level a column (or a row) sharing their levels,
    /// as those along a straight edge do.
    fn shared(rows: Vec<(i32, Vec<Run>)>, outside: u16) -> Mask {
        let mut seen: std::collections::HashSet<(bool, Arc<[u16]>)> = std::collections::HashSet::new();
        let mut rows = rows;
        for (_, row) in &mut rows {
            for r in row.iter_mut() {
                let (columns, levels) = match &r.tile {
                    MaskTile::Columns(l) => (true, l),
                    MaskTile::Rows(l) => (false, l),
                    _ => continue,
                };
                match seen.get(&(columns, levels.clone())) {
                    Some((_, first)) => {
                        let first = first.clone();
                        r.tile = if columns { MaskTile::Columns(first) } else { MaskTile::Rows(first) };
                    }
                    None => {
                        seen.insert((columns, levels.clone()));
                    }
                }
            }
        }
        Mask { rows: rows.into_iter().collect(), outside }
    }

    /// The runs of row `y` of tiles (none, if it keeps no tile).
    fn row(&self, y: i32) -> &[Run] {
        self.rows.get(&y).map_or(NO_RUNS, Vec::as_slice)
    }

    /// The tile at `c`, if one is kept there.
    pub fn tile(&self, c: TileCoord) -> Option<&MaskTile> {
        let row = self.rows.get(&c.y)?;
        let i = row.partition_point(|r| r.x1 <= c.x);
        row.get(i).filter(|r| r.x0 <= c.x).map(|r| &r.tile)
    }

    /// The level of pixel `(x, y)`.
    pub fn level_at(&self, x: i64, y: i64) -> u16 {
        let c = TileCoord::containing(x, y);
        match self.tile(c) {
            None => self.outside,
            Some(t) => {
                let r = c.rect(0);
                t.get((x - r.x0) as u32, (y - r.y0) as u32)
            }
        }
    }

    /// Whether it selects nothing at all.
    pub fn is_none(&self) -> bool {
        self.outside == 0 && self.rows.is_empty()
    }

    /// The memory it takes.
    pub fn bytes(&self) -> u64 {
        let parts: HashMap<usize, u64> = self.parts().collect();
        parts.values().sum()
    }

    /// What it takes memory for, each part with where it is kept, so that what copies (and
    /// alike tiles) share can be counted once: its runs (kept where the mask is), then each
    /// tile's levels.
    pub fn parts(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        let runs: usize = self.rows.values().map(Vec::len).sum();
        let own = self.rows.len() * std::mem::size_of::<(i32, Vec<Run>)>() + runs * std::mem::size_of::<Run>();
        std::iter::once((std::ptr::from_ref(self) as usize, own as u64))
            .chain(self.rows.values().flatten().filter_map(|r| Some((r.tile.address()?, r.tile.bytes()))))
    }

    /// How much of the plane it selects.
    pub fn extent(&self) -> Extent {
        if self.outside != 0 {
            return Extent::Unbounded;
        }
        let mut within: Option<IRect> = None;
        for (&y, row) in &self.rows {
            for r in row {
                let corner = TileCoord::new(r.x0, y).rect(0);
                let part = match &r.tile {
                    MaskTile::Level(_) => IRect { x1: TileCoord::new(r.x1, y).rect(0).x0, ..corner },
                    t => match t.selected_part() {
                        Some(p) => p.translate(corner.x0, corner.y0),
                        None => continue,
                    },
                };
                within = Some(within.map_or(part, |w| w.union(&part)));
            }
        }
        within.map_or(Extent::Nothing, Extent::Within)
    }

    /// The mask of `shape`: each pixel's level, how much of it the shape covers.
    pub fn of(shape: &impl Coverage) -> Mask {
        let bounds = shape.bounds();
        if bounds.is_empty() {
            return Mask::none();
        }
        let first = TileCoord::containing(bounds.x0, bounds.y0);
        let last = TileCoord::containing(bounds.x1 - 1, bounds.y1 - 1);
        let rows: Vec<(i32, Vec<Run>)> = (first.y..=last.y)
            .into_par_iter()
            .map(|y| (y, row_of(shape, y, first.x, last.x + 1)))
            .filter(|(_, r)| !r.is_empty())
            .collect();
        Mask::shared(rows, 0)
    }

    /// The opposite: selected where it is not, and the other way round.
    pub fn inverted(&self) -> Mask {
        let rows: Vec<(i32, Vec<Run>)> = self
            .rows
            .par_iter()
            .map(|(&y, row)| {
                (y, row.iter().map(|r| Run { x0: r.x0, x1: r.x1, tile: r.tile.map(|l| FULL - l) }).collect())
            })
            .collect();
        Mask::shared(rows, FULL - self.outside)
    }

    /// `new` combined with this one, as `how` says.
    pub fn combined(&self, new: &Mask, how: Combine) -> Mask {
        if how == Combine::Replace {
            return new.clone();
        }
        let outside = how.level(self.outside, new.outside);
        let (a_out, b_out) = (MaskTile::Level(self.outside), MaskTile::Level(new.outside));
        let mut ys: Vec<i32> = self.rows.keys().chain(new.rows.keys()).copied().collect();
        ys.sort_unstable();
        ys.dedup();
        let rows: Vec<(i32, Vec<Run>)> = ys
            .into_par_iter()
            .map(|y| {
                let tiles: Vec<(i32, i32, MaskTile)> = pieces(self.row(y), new.row(y), &a_out, &b_out)
                    .into_par_iter()
                    .map(|(x0, x1, a, b)| (x0, x1, MaskTile::combined(a, b, how)))
                    .collect();
                let mut row = Vec::new();
                for (x0, x1, t) in tiles {
                    push(&mut row, x0, x1, t, outside);
                }
                (y, row)
            })
            .filter(|(_, r)| !r.is_empty())
            .collect();
        Mask::shared(rows, outside)
    }

    /// The rectangle round every tile kept, if any is.
    fn kept_area(&self) -> Option<IRect> {
        let (&y0, _) = self.rows.first_key_value()?;
        let (&y1, _) = self.rows.last_key_value()?;
        let x0 = self.rows.values().map(|r| r[0].x0).min()?;
        let x1 = self.rows.values().map(|r| r[r.len() - 1].x1).max()?;
        let (a, b) = (TileCoord::new(x0, y0).rect(0), TileCoord::new(x1, y1 + 1).rect(0));
        Some(IRect::new(a.x0, a.y0, b.x0, b.y0))
    }

    /// The mask `m` carries this one to: each pixel's level read where `m` takes it from,
    /// exactly where `m` moves the pixel grid onto itself (a quarter turn, a flip), else between
    /// the four pixels nearest. A map that flattens the plane leaves only the level outside.
    pub fn transformed(&self, m: &Affine) -> Mask {
        let Some(inverse) = m.inverse() else {
            return Mask { rows: BTreeMap::new(), outside: self.outside };
        };
        let Some(area) = self.kept_area() else { return self.clone() };
        let exact = m.grid_map().is_some();
        // Reading between pixels reaches a pixel past what the kept tiles are carried to.
        let target = IRect::covering(&m.bounds(&area.to_f())).expand(1);
        let first = TileCoord::containing(target.x0, target.y0);
        let last = TileCoord::containing(target.x1 - 1, target.y1 - 1);
        let tile = |c: TileCoord| -> MaskTile {
            let r = c.rect(0);
            // What it is read from, and the pixels round that reading between them reaches.
            let from = IRect::covering(&inverse.bounds(&r.to_f())).expand(1);
            if let Some(l) = self.uniform_in(&from) {
                return MaskTile::Level(l);
            }
            // Read pixel by pixel, the tile last read kept at hand: what it comes from may be
            // far larger than a tile (shrunk to a hundredth, a hundred times as wide).
            let mut last: Option<(TileCoord, Option<&MaskTile>)> = None;
            let mut at = |x: i64, y: i64| -> f32 {
                let c = TileCoord::containing(x, y);
                let t = match last {
                    Some((at, t)) if at == c => t,
                    _ => {
                        let t = self.tile(c);
                        last = Some((c, t));
                        t
                    }
                };
                let r = c.rect(0);
                f32::from(t.map_or(self.outside, |t| t.get((x - r.x0) as u32, (y - r.y0) as u32)))
            };
            let mut levels = vec![0u16; TILE_PIXELS];
            for (i, l) in levels.iter_mut().enumerate() {
                let (px, py) = (r.x0 + (i % SIDE) as i64, r.y0 + (i / SIDE) as i64);
                let [x, y] = inverse.apply([px as f64 + 0.5, py as f64 + 0.5]);
                *l = if exact {
                    at(x.floor() as i64, y.floor() as i64) as u16
                } else {
                    let (fx, fy) = (x - 0.5, y - 0.5);
                    let (x0, y0) = (fx.floor(), fy.floor());
                    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
                    let (x0, y0) = (x0 as i64, y0 as i64);
                    let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
                    let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
                    (top * (1.0 - ty) + bottom * ty).round() as u16
                };
            }
            MaskTile::from_levels(levels)
        };
        let rows: Vec<(i32, Vec<Run>)> = (first.y..=last.y)
            .into_par_iter()
            .map(|y| {
                let tiles: Vec<MaskTile> = (first.x..=last.x).into_par_iter().map(|x| tile(TileCoord::new(x, y))).collect();
                let mut row = Vec::new();
                for (x, t) in (first.x..).zip(tiles) {
                    push(&mut row, x, x + 1, t, self.outside);
                }
                (y, row)
            })
            .filter(|(_, r)| !r.is_empty())
            .collect();
        Mask::shared(rows, self.outside)
    }

    /// The levels of the pixels of `area`, row by row.
    pub fn levels_in(&self, area: &IRect) -> Vec<u16> {
        let (w, h) = (area.width().max(0) as usize, area.height().max(0) as usize);
        let mut out = vec![self.outside; w * h];
        for c in TileCoord::covering(area, 0) {
            let Some(t) = self.tile(c) else { continue };
            let r = c.rect(0);
            let part = r.intersect(area);
            for y in part.y0..part.y1 {
                let row = &mut out[(y - area.y0) as usize * w..][..w];
                let (x0, x1) = ((part.x0 - area.x0) as usize, (part.x1 - area.x0) as usize);
                match t {
                    MaskTile::Level(l) => row[x0..x1].fill(*l),
                    t => {
                        for x in part.x0..part.x1 {
                            row[(x - area.x0) as usize] = t.get((x - r.x0) as u32, (y - r.y0) as u32);
                        }
                    }
                }
            }
        }
        out
    }

    /// The level of every pixel of `area`, if they all have the same (told from the tiles
    /// alone: `None` where a tile with levels of its own reaches into it).
    pub fn uniform_in(&self, area: &IRect) -> Option<u16> {
        let mut level = None;
        for c in TileCoord::covering(area, 0) {
            let l = match self.tile(c) {
                None => self.outside,
                Some(MaskTile::Level(l)) => *l,
                Some(_) => return None,
            };
            if *level.get_or_insert(l) != l {
                return None;
            }
        }
        level
    }

    /// Where the mask crosses `threshold` (the marching ants): closed lines along the pixels'
    /// edges, between those at `threshold` or above and those below, in document coordinates,
    /// each by its corners. They go clockwise round what is selected (anticlockwise round a
    /// hole), and two that meet at a corner stay apart there.
    pub fn outline(&self, threshold: u16) -> Vec<Vec<[f64; 2]>> {
        contours(self, threshold)
    }
}

/// How a tile lies against a shape.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Outside,
    Inside,
    /// Crossed by its edge (or near enough to it that it may be).
    Edge,
}

/// How the square of side `side` about `(cx, cy)` lies against `shape`, told from its middle
/// (every pixel's middle in it is nearer the square's middle than its corners are).
fn class(shape: &impl Coverage, cx: f64, cy: f64, side: f64) -> Class {
    let far = side * std::f64::consts::FRAC_1_SQRT_2 + 1.0;
    let d = shape.distance(cx, cy);
    if d >= far {
        Class::Outside
    } else if d <= -far {
        Class::Inside
    } else {
        Class::Edge
    }
}

/// Row `y` of tiles, those from `x0` to `x1`, of the mask of `shape`.
fn row_of(shape: &impl Coverage, y: i32, x0: i32, x1: i32) -> Vec<Run> {
    let side = f64::from(TILE_SIZE);
    let class_of = |x: i32| {
        let [cx, cy] = TileCoord::new(x, y).rect(0).to_f().center();
        class(shape, cx, cy, side)
    };
    // The tiles told apart, left to right: of a convex shape, those up to the first wholly
    // inside it and from the last, every one between those inside it too.
    let mut told: Vec<(i32, i32, Class)> = Vec::new();
    let mut x = x0;
    while x < x1 {
        let c = class_of(x);
        if c == Class::Inside && shape.convex() {
            break;
        }
        told.push((x, x + 1, c));
        x += 1;
    }
    if x < x1 {
        let mut right = Vec::new();
        let mut r = x1 - 1;
        // Tile `x` is inside: this stops there at the latest.
        loop {
            let c = class_of(r);
            if c == Class::Inside {
                break;
            }
            right.push((r, r + 1, c));
            r -= 1;
        }
        told.push((x, r + 1, Class::Inside));
        told.extend(right.into_iter().rev());
    }
    let tiles: Vec<(i32, i32, MaskTile)> = told
        .into_par_iter()
        .filter_map(|(a, b, c)| match c {
            Class::Outside => None,
            Class::Inside => Some((a, b, MaskTile::Level(FULL))),
            Class::Edge => Some((a, b, edge_tile(shape, TileCoord::new(a, y)))),
        })
        .collect();
    let mut row = Vec::new();
    for (a, b, t) in tiles {
        push(&mut row, a, b, t, 0);
    }
    row
}

/// The side of the blocks a tile its edge crosses is told apart by.
const BLOCK: u32 = 16;

/// The tile at `c` of the mask of `shape`, which its edge may cross: told apart block by
/// block, the pixels of the blocks the edge may cross one by one.
fn edge_tile(shape: &impl Coverage, c: TileCoord) -> MaskTile {
    let r = c.rect(0);
    let mut levels = vec![0u16; TILE_PIXELS];
    for by in (0..TILE_SIZE).step_by(BLOCK as usize) {
        for bx in (0..TILE_SIZE).step_by(BLOCK as usize) {
            let (x0, y0) = (r.x0 as f64 + f64::from(bx), r.y0 as f64 + f64::from(by));
            let half = f64::from(BLOCK) / 2.0;
            let fill = match class(shape, x0 + half, y0 + half, f64::from(BLOCK)) {
                Class::Outside => continue,
                Class::Inside => Some(FULL),
                Class::Edge => None,
            };
            for y in by..by + BLOCK {
                let row = &mut levels[(y * TILE_SIZE) as usize..][..SIDE];
                for x in bx..bx + BLOCK {
                    row[x as usize] =
                        fill.unwrap_or_else(|| level(shape.at(x0 + f64::from(x - bx) + 0.5, y0 + f64::from(y - by) + 0.5)));
                }
            }
        }
    }
    MaskTile::from_levels(levels)
}

/// `before` with each pixel `p` made `p × (1 − m) + changed(p) × m`, `m` the level of `mask`
/// there: the step every operation limited to a selection goes through (design.md, *The
/// mask*). `before` is a pixel object's bitmap, its pixel `(0, 0)` at `offset` in the document.
/// `changed` must leave a transparent pixel transparent (only the tiles `before` has are
/// changed); tiles the mask leaves alone, and those it covers but `changed` would not alter,
/// stay as they were, shared.
pub fn blend(
    before: &RasterImage,
    offset: [i64; 2],
    mask: &Mask,
    changed: impl Fn(PixelF32) -> PixelF32 + Sync,
) -> RasterImage {
    blend_by(before, offset, mask, fraction, Some(&changed))
}

/// `before` (a pixel object's bitmap at `offset`, as for [`blend`]) with what `mask` selects
/// erased, as much of each pixel as it selects it: what Clear does.
pub fn clear_selected(before: &RasterImage, offset: [i64; 2], mask: &Mask) -> RasterImage {
    blend_by(before, offset, mask, fraction, None)
}

/// `before` (a pixel object's bitmap at `offset`, as for [`blend`]) with only what `mask`
/// selects kept, as much of each pixel as it selects it: what copying within a selection
/// copies.
pub fn keep_selected(before: &RasterImage, offset: [i64; 2], mask: &Mask) -> RasterImage {
    blend_by(before, offset, mask, |l| 1.0 - fraction(l), None)
}

/// [`blend`], with `weight` giving `m` from each level, and `changed` making every pixel
/// transparent where it is `None`. Tiles where `m` is 0 throughout are left alone, and where
/// it is 1 throughout and `changed` is `None`, dropped, without looking at their pixels.
fn blend_by(
    before: &RasterImage,
    offset: [i64; 2],
    mask: &Mask,
    weight: impl Fn(u16) -> f32 + Sync,
    changed: Option<&(dyn Fn(PixelF32) -> PixelF32 + Sync)>,
) -> RasterImage {
    debug_assert!(changed.is_none_or(|f| f([0.0; 4]) == [0.0; 4]), "a transparent pixel changed");
    let coords: Vec<TileCoord> = before.tiles().map(|(c, _)| c).collect();
    let done: Vec<(TileCoord, Option<Arc<Tile>>)> = coords
        .into_par_iter()
        .filter_map(|c| {
            let area = c.rect(0).translate(offset[0], offset[1]);
            let uniform = mask.uniform_in(&area).map(&weight);
            match uniform {
                Some(0.0) => return None,
                Some(m) if m == 1.0 && changed.is_none() => return Some((c, None)),
                _ => {}
            }
            let weights: Option<Vec<f32>> =
                uniform.is_none().then(|| mask.levels_in(&area).into_iter().map(&weight).collect());
            if weights.as_ref().is_some_and(|w| w.iter().all(|&m| m == 0.0)) {
                return None;
            }
            let tile = before.tile(c).expect("listed");
            let mut px = vec![[0.0f32; 4]; TILE_PIXELS];
            tile.to_f32(&mut px);
            let mut same = true;
            for (i, p) in px.iter_mut().enumerate() {
                let m = weights.as_ref().map_or_else(|| uniform.expect("one weight or a weight a pixel"), |w| w[i]);
                if m == 0.0 {
                    continue;
                }
                let c = changed.map_or([0.0; 4], |f| f(*p));
                let new = [0, 1, 2, 3].map(|i| p[i] * (1.0 - m) + c[i] * m);
                if new != *p {
                    same = false;
                    *p = new;
                }
            }
            if same {
                return None;
            }
            let t = Tile::from_pixels(px.iter().map(|&p| pixel_from_f32(p)).collect());
            Some((c, (!t.is_transparent()).then(|| Arc::new(t))))
        })
        .collect();
    let mut out = before.clone();
    for (c, t) in done {
        out.set_tile(c, t);
    }
    out
}

/// What [`Mask::outline`] gives: the edges between a pixel in (at `threshold` or above) and a
/// neighbour out, joined into closed lines. Only the edges of tiles that are not alike, and
/// those within a tile with levels of its own, are looked at; the edges along a whole side
/// of tiles of one level are taken as one.
fn contours(mask: &Mask, threshold: u16) -> Vec<Vec<[f64; 2]>> {
    let inn = |l: u16| l >= threshold;
    let outside = MaskTile::Level(mask.outside);
    let side = i64::from(TILE_SIZE);
    // Each kept row of tiles, and the line between it and the rows above and below it.
    let mut ys: Vec<i32> = mask.rows.keys().flat_map(|&y| [y, y + 1]).collect();
    ys.sort_unstable();
    ys.dedup();
    let edges: Vec<Step> = ys
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut out = Vec::new();
            let top = i64::from(y) * side;
            // Across the line between rows y − 1 and y.
            for (x0, x1, above, below) in pieces(mask.row(y - 1), mask.row(y), &outside, &outside) {
                let left = i64::from(x0) * side;
                match (above, below) {
                    (MaskTile::Level(a), MaskTile::Level(b)) => {
                        if inn(*a) != inn(*b) {
                            out.push(horizontal(left, i64::from(x1) * side, top, inn(*b)));
                        }
                    }
                    _ => {
                        for i in 0..TILE_SIZE {
                            let (a, b) = (inn(above.get(i, TILE_SIZE - 1)), inn(below.get(i, 0)));
                            if a != b {
                                let x = left + i64::from(i);
                                out.push(horizontal(x, x + 1, top, b));
                            }
                        }
                    }
                }
            }
            // Within row y: between tiles side by side where they are not alike (where a run
            // begins or ends), and within the tiles with levels of their own.
            let row = mask.row(y);
            let mut cuts: Vec<i32> = row.iter().flat_map(|r| [r.x0, r.x1]).collect();
            cuts.dedup();
            let tile_at = |x: i32| mask.tile(TileCoord::new(x, y)).unwrap_or(&outside);
            for x in cuts {
                let (l, r) = (tile_at(x - 1), tile_at(x));
                let at = i64::from(x) * side;
                match (l, r) {
                    (MaskTile::Level(a), MaskTile::Level(b)) => {
                        if inn(*a) != inn(*b) {
                            out.push(vertical(at, top, top + side, inn(*b)));
                        }
                    }
                    _ => {
                        for i in 0..TILE_SIZE {
                            let (a, b) = (inn(l.get(TILE_SIZE - 1, i)), inn(r.get(0, i)));
                            if a != b {
                                let y = top + i64::from(i);
                                out.push(vertical(at, y, y + 1, b));
                            }
                        }
                    }
                }
            }
            for r in row {
                let left = i64::from(r.x0) * side;
                match &r.tile {
                    MaskTile::Level(_) => {}
                    MaskTile::Columns(l) => {
                        for (i, w) in l.windows(2).enumerate() {
                            if inn(w[0]) != inn(w[1]) {
                                out.push(vertical(left + i as i64 + 1, top, top + side, inn(w[1])));
                            }
                        }
                    }
                    MaskTile::Rows(l) => {
                        for (i, w) in l.windows(2).enumerate() {
                            if inn(w[0]) != inn(w[1]) {
                                out.push(horizontal(left, left + side, top + i as i64 + 1, inn(w[1])));
                            }
                        }
                    }
                    MaskTile::Levels(l) => {
                        for py in 0..SIDE {
                            for px in 0..SIDE {
                                let here = inn(l[py * SIDE + px]);
                                let (x, y) = (left + px as i64, top + py as i64);
                                if px > 0 && inn(l[py * SIDE + px - 1]) != here {
                                    out.push(vertical(x, y, y + 1, here));
                                }
                                if py > 0 && inn(l[(py - 1) * SIDE + px]) != here {
                                    out.push(horizontal(x, x + 1, y, here));
                                }
                            }
                        }
                    }
                }
            }
            out
        })
        .collect();
    join(edges)
}

/// The edge along the line `x` from `y0` down to `y1`, between pixels in and out, clockwise
/// round those in (`right_in`: those on its right are in, y downward).
fn vertical(x: i64, y0: i64, y1: i64, right_in: bool) -> Step {
    if right_in { Step { from: (x, y1), to: (x, y0) } } else { Step { from: (x, y0), to: (x, y1) } }
}

/// The edge along the line `y` from `x0` to `x1`, between pixels in and out, clockwise round
/// those in (`below_in`: those below it are in).
fn horizontal(x0: i64, x1: i64, y: i64, below_in: bool) -> Step {
    if below_in { Step { from: (x0, y), to: (x1, y) } } else { Step { from: (x1, y), to: (x0, y) } }
}

/// A straight piece of edge between pixels, from one pixel corner to another, clockwise round
/// what is in. No other piece begins or ends between its ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    from: (i64, i64),
    to: (i64, i64),
}

impl Step {
    /// Which way it goes: one of the four, a step of 1.
    fn direction(self) -> (i64, i64) {
        ((self.to.0 - self.from.0).signum(), (self.to.1 - self.from.1).signum())
    }
}

/// Edges joined end to end into closed lines, each made of its corners alone. Where two
/// lines touch at a corner (pixels in meeting only diagonally), each turns right there, so
/// that it keeps to its own pixels.
fn join(steps: Vec<Step>) -> Vec<Vec<[f64; 2]>> {
    let mut from: HashMap<(i64, i64), Vec<usize>> = HashMap::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        from.entry(s.from).or_default().push(i);
    }
    let mut used = vec![false; steps.len()];
    let mut lines = Vec::new();
    for start in 0..steps.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut line = vec![steps[start].from];
        let mut at = steps[start];
        loop {
            line.push(at.to);
            let (dx, dy) = at.direction();
            // Right, ahead, then left (in screen coordinates, y downward).
            let order = [(-dy, dx), (dx, dy), (dy, -dx)];
            let next = order.iter().find_map(|&d| {
                from.get(&at.to)?.iter().copied().find(|&j| steps[j].direction() == d && (!used[j] || j == start))
            });
            let Some(j) = next else { break };
            if j == start {
                break;
            }
            used[j] = true;
            at = steps[j];
        }
        // Closed: its last point is its first again.
        if line.len() > 2 && line.first() == line.last() {
            line.pop();
        }
        lines.push(corners(line.into_iter().map(|(x, y)| [x as f64, y as f64]).collect()));
    }
    lines
}

/// A closed line made of its corners alone: points between two others in line with them go.
fn corners(line: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    let n = line.len();
    if n < 3 {
        return line;
    }
    let straight = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        cross.abs() < 1e-9
    };
    let kept: Vec<[f64; 2]> =
        (0..n).filter(|&i| !straight(line[(i + n - 1) % n], line[i], line[(i + 1) % n])).map(|i| line[i]).collect();
    if kept.len() < 3 { line } else { kept }
}

/// A rectangle or an ellipse, turned about its middle, as a selection's shape.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Its middle, in document pixels.
    pub center: [f64; 2],
    /// Its width and height (an ellipse's diameters), in pixels.
    pub size: [f64; 2],
    /// How far it is turned clockwise, in degrees.
    pub angle: f64,
    /// Whether its edge is smoothed (each pixel as selected as it is covered), or sharp
    /// (selected or not by its middle).
    pub smooth: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
}

/// The size and angle (in degrees) of a rectangle or an ellipse `size` wide and high turned
/// by `angle`, as those of the same shape turned by an eighth or less either way: a quarter
/// turn more or less is its sides swapped. Within a billionth of a degree of a quarter turn
/// counts as one, so that a shape on the grid turned by a quarter is on it again.
fn upright(size: [f64; 2], angle: f64) -> ([f64; 2], f64) {
    let quarters = (angle / 90.0).round();
    let rest = angle - quarters * 90.0;
    let size = if (quarters as i64).rem_euclid(2) == 1 { [size[1], size[0]] } else { size };
    (size, if rest.abs() < 1e-9 { 0.0 } else { rest })
}

impl Shape {
    /// The rectangle `r` (document pixels), upright; smoothed if `smooth`.
    pub fn rectangle(r: &FRect, smooth: bool) -> Shape {
        Shape { kind: ShapeKind::Rectangle, center: r.center(), size: [r.width(), r.height()], angle: 0.0, smooth }
    }

    /// The shape `m` carries this one to, if it is one: an ellipse always is, a rectangle
    /// only while `m` keeps its sides at right angles (not once a turned one is stretched
    /// along another way). It is turned by an eighth or less (see [`upright`]).
    pub fn transformed(&self, m: &Affine) -> Option<Shape> {
        let (s, c) = self.angle.to_radians().sin_cos();
        let [w, h] = self.size.map(|v| v.abs() / 2.0);
        let linear = |[x, y]: [f64; 2]| [m.a * x + m.c * y, m.b * x + m.d * y];
        // Its half-width and half-height, as the arrows from its middle along its sides.
        let (u, v) = (linear([c * w, s * w]), linear([-s * h, c * h]));
        let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
        let (size, angle) = if (u[0] * v[0] + u[1] * v[1]).abs() <= 1e-9 * lu * lv {
            ([2.0 * lu, 2.0 * lv], u[1].atan2(u[0]).to_degrees())
        } else if self.kind == ShapeKind::Ellipse {
            // The ellipse [u v] carries the unit circle to: its semi-axes are the square roots
            // of the eigenvalues of [u v][u v]ᵀ, the longer along the first eigenvector.
            let (p, q, r) = (u[0] * u[0] + v[0] * v[0], u[0] * u[1] + v[0] * v[1], u[1] * u[1] + v[1] * v[1]);
            let (mid, apart) = ((p + r) / 2.0, ((p - r) / 2.0).hypot(q));
            let (long, short) = ((mid + apart).sqrt(), (mid - apart).max(0.0).sqrt());
            ([2.0 * long, 2.0 * short], (0.5 * (2.0 * q).atan2(p - r)).to_degrees())
        } else {
            return None;
        };
        let (size, angle) = upright(size, angle);
        Some(Shape { center: m.apply(self.center), size, angle, ..*self })
    }

    /// `(x, y)` in the shape's own frame: from its middle, along its sides.
    fn local(&self, x: f64, y: f64) -> [f64; 2] {
        let (s, c) = self.angle.to_radians().sin_cos();
        let (dx, dy) = (x - self.center[0], y - self.center[1]);
        [dx * c + dy * s, -dx * s + dy * c]
    }

    /// Of an ellipse: whether `(x, y)` is inside it, if it is surely more than `margin` from its
    /// edge. A point on the ellipse `s` times as large (about the same middle) is at least
    /// `|s - 1|` times the shorter semi-axis `b` from its edge: the ellipse holds the disc of
    /// radius `b` about its middle, so, being convex, the disc of radius `(1 - s)·b` about each
    /// point of the one `s` times as large (and the other way round where `s` is above 1).
    fn ellipse_far(&self, x: f64, y: f64, margin: f64) -> Option<bool> {
        let [lx, ly] = self.local(x, y);
        let [a, b] = self.size.map(|v| v.abs() / 2.0);
        if a.min(b) <= 0.0 {
            return None;
        }
        let s = ((lx / a).powi(2) + (ly / b).powi(2)).sqrt();
        ((s - 1.0).abs() * a.min(b) > margin).then_some(s < 1.0)
    }
}

impl Coverage for Shape {
    fn bounds(&self) -> IRect {
        let [w, h] = self.size.map(|v| v.abs() / 2.0);
        let (s, c) = self.angle.to_radians().sin_cos();
        let (ex, ey) = (w * c.abs() + h * s.abs(), w * s.abs() + h * c.abs());
        let [x, y] = self.center;
        IRect::covering(&FRect::new(x - ex, y - ey, x + ex, y + ey)).expand(1)
    }

    fn distance(&self, x: f64, y: f64) -> f64 {
        let [lx, ly] = self.local(x, y);
        let [a, b] = self.size.map(|v| v.abs() / 2.0);
        match self.kind {
            ShapeKind::Rectangle => {
                let (dx, dy) = (lx.abs() - a, ly.abs() - b);
                dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0)
            }
            ShapeKind::Ellipse => ellipse_distance(lx, ly, a, b),
        }
    }

    fn at(&self, x: f64, y: f64) -> f32 {
        // Most of an ellipse's pixels are told without the distance's bisection.
        if self.kind == ShapeKind::Ellipse
            && let Some(inside) = self.ellipse_far(x, y, if self.smooth { 0.5 } else { 0.0 })
        {
            return f32::from(u8::from(inside));
        }
        let d = self.distance(x, y);
        if self.smooth { (0.5 - d).clamp(0.0, 1.0) as f32 } else { f32::from(u8::from(d <= 0.0)) }
    }

    fn convex(&self) -> bool {
        true
    }
}

/// The signed distance from `(x, y)` to the ellipse about the origin with semi-axes `a` along
/// x and `b` along y (less than 0 inside): exact, by bisection (Eberly, *Distance from a Point
/// to an Ellipse*).
fn ellipse_distance(x: f64, y: f64, a: f64, b: f64) -> f64 {
    // In the first quadrant, the longer axis first.
    let (mut p0, mut p1, mut e0, mut e1) = (x.abs(), y.abs(), a, b);
    if e0 < e1 {
        (p0, p1, e0, e1) = (p1, p0, e1, e0);
    }
    if e1 <= 0.0 {
        // Flat: a segment along the longer axis.
        return if p0 <= e0 { p1 } else { (p0 - e0).hypot(p1) };
    }
    let inside = (p0 / e0).powi(2) + (p1 / e1).powi(2) < 1.0;
    let d = if p1 > 0.0 {
        if p0 > 0.0 {
            let (z0, z1) = (p0 / e0, p1 / e1);
            let g = z0 * z0 + z1 * z1 - 1.0;
            if g == 0.0 {
                0.0
            } else {
                let r0 = (e0 / e1).powi(2);
                let n0 = r0 * z0;
                let (mut s0, mut s1) = (z1 - 1.0, if g < 0.0 { 0.0 } else { n0.hypot(z1) - 1.0 });
                let mut s = 0.0;
                for _ in 0..200 {
                    s = (s0 + s1) / 2.0;
                    if s == s0 || s == s1 {
                        break;
                    }
                    let g = (n0 / (s + r0)).powi(2) + (z1 / (s + 1.0)).powi(2) - 1.0;
                    if g > 0.0 {
                        s0 = s;
                    } else if g < 0.0 {
                        s1 = s;
                    } else {
                        break;
                    }
                }
                let (x0, x1) = (r0 * p0 / (s + r0), p1 / (s + 1.0));
                (x0 - p0).hypot(x1 - p1)
            }
        } else {
            (p1 - e1).abs()
        }
    } else {
        let (numer, denom) = (e0 * p0, e0 * e0 - e1 * e1);
        if numer < denom {
            let xde0 = numer / denom;
            let (x0, x1) = (e0 * xde0, e1 * (1.0 - xde0 * xde0).max(0.0).sqrt());
            (x0 - p0).hypot(x1)
        } else {
            (p0 - e0).abs()
        }
    };
    if inside { -d } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Shape {
        Shape {
            kind: ShapeKind::Rectangle,
            center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
            size: [x1 - x0, y1 - y0],
            angle: 0.0,
            smooth: true,
        }
    }

    #[test]
    fn a_rectangle_on_the_pixel_grid_is_sharp_and_kept_by_its_edges() {
        // 1000 × 600 at (10, 20): its edges' tiles kept, the tiles within it one level each.
        let m = Mask::of(&rect(10.0, 20.0, 1010.0, 620.0));
        assert_eq!(m.level_at(10, 20), FULL);
        assert_eq!(m.level_at(1009, 619), FULL);
        assert_eq!(m.level_at(9, 20), 0);
        assert_eq!(m.level_at(1010, 300), 0);
        assert_eq!(m.extent(), Extent::Within(IRect::new(10, 20, 1010, 620)));
        // Tiles 0..=3 across, 0..=2 down: those wholly inside it one level, those along its
        // sides a level a column or a row, and only its corners' a level a pixel.
        let at = |x, y| m.tile(TileCoord::new(x, y)).cloned();
        assert_eq!(at(1, 1), Some(MaskTile::Level(FULL)));
        assert!(matches!(at(0, 1), Some(MaskTile::Columns(_))) && matches!(at(3, 1), Some(MaskTile::Columns(_))));
        assert!(matches!(at(1, 0), Some(MaskTile::Rows(_))) && matches!(at(2, 2), Some(MaskTile::Rows(_))));
        assert!([(0, 0), (3, 0), (0, 2), (3, 2)].iter().all(|&(x, y)| matches!(at(x, y), Some(MaskTile::Levels(_)))));
        let corners = 4 * TILE_PIXELS as u64 * 2;
        assert!(m.bytes() < corners + 8000, "{} bytes", m.bytes());
    }

    #[test]
    fn a_rectangle_as_large_as_a_canvas_may_be_costs_its_edges() {
        // 300,000 pixels a side (`dialogs::MAX_SIDE`): 1172 rows of 1172 tiles.
        let side = 300_000.0;
        let m = Mask::of(&Shape { smooth: false, ..rect(0.0, 0.0, side, side) });
        assert_eq!(m.rows.len(), 1172);
        // Within it, a run a row; along its right side, which ends within a column of tiles,
        // and its bottom, a tile at a time.
        assert!(m.rows.values().rev().skip(1).all(|r| r.len() == 2), "a run and a tile a row");
        assert_eq!(m.row(1171).len(), 1172);
        // Alike, the tiles along each side share their levels: one a side, and the corner's.
        let arrays: std::collections::HashSet<usize> = m.parts().skip(1).map(|(a, _)| a).collect();
        assert!(arrays.len() <= 3, "{} arrays of levels", arrays.len());
        assert!(m.bytes() < 2_000_000, "{} bytes", m.bytes());
        assert_eq!(m.extent(), Extent::Within(IRect::new(0, 0, 300_000, 300_000)));
        assert_eq!(m.level_at(150_000, 150_000), FULL);
        let mut lines = m.outline(HALF);
        assert_eq!(lines.len(), 1);
        lines[0].sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(lines[0], [[0.0, 0.0], [0.0, side], [side, 0.0], [side, side]]);
        // Inverted, taken away from and added to, it keeps to its edges.
        let hole = m.inverted();
        assert_eq!(hole.outline(HALF).len(), 1);
        let inner = Mask::of(&rect(1000.0, 1000.0, side - 1000.0, side - 1000.0));
        let frame = m.combined(&inner, Combine::Subtract);
        assert_eq!(frame.outline(HALF).len(), 2, "round the frame, and round its hole");
        // About 7000 tiles along its four edges and its hole's, each a level a column or a row.
        assert!(frame.bytes() < 8_000_000, "{} bytes", frame.bytes());
        assert_eq!(frame.combined(&inner, Combine::Add), m);
    }

    #[test]
    fn combining_with_what_leaves_a_tile_as_it_is_shares_that_tile() {
        let e = Mask::of(&Shape {
            kind: ShapeKind::Ellipse,
            center: [500.5, 500.5],
            size: [600.0, 400.0],
            angle: 20.0,
            smooth: true,
        });
        let far = Mask::of(&rect(5000.0, 0.0, 5100.0, 100.0));
        let arrays = |m: &Mask| m.parts().skip(1).map(|(a, _)| a).collect::<std::collections::HashSet<usize>>();
        assert!(!arrays(&e).is_empty());
        for (how, with) in [(Combine::Add, &far), (Combine::Subtract, &far), (Combine::Intersect, &Mask::all())] {
            assert!(arrays(&e).is_subset(&arrays(&e.combined(with, how))), "{how:?} copied the ellipse's tiles");
        }
        // Added to nothing there, the new mask's tiles are shared too.
        assert!(arrays(&far).is_subset(&arrays(&Mask::none().combined(&far, Combine::Add))));
    }

    #[test]
    fn a_mask_shrunk_a_hundredfold_is_read_only_where_its_pixels_come_from() {
        // A turned square 30,000 pixels a side, no longer one once stretched: carried pixel by
        // pixel, each read from the four it lies between.
        let m = Mask::of(&Shape { angle: 10.0, ..rect(0.0, 0.0, 30_000.0, 30_000.0) });
        let small = m.transformed(&Affine::scale(0.01, 0.0067));
        assert_eq!(small.level_at(150, 100), FULL);
        assert_eq!([small.level_at(400, 400), small.level_at(-50, 100)], [0, 0]);
        let Extent::Within(r) = small.extent() else { panic!("{:?}", small.extent()) };
        // The turned square's corners reach 15,000·(cos 10° + sin 10°) from its middle.
        let reach = 15_000.0 * (10f64.to_radians().cos() + 10f64.to_radians().sin());
        assert!((r.x1 as f64 - (15_000.0 + reach) * 0.01).abs() < 2.0, "{r:?}");
    }

    #[test]
    fn masks_alike_are_kept_alike_however_they_were_made() {
        let whole = Mask::of(&rect(0.0, 0.0, 1024.0, 512.0));
        let halves = Mask::of(&rect(0.0, 0.0, 512.0, 512.0)).combined(&Mask::of(&rect(512.0, 0.0, 1024.0, 512.0)), Combine::Add);
        assert_eq!(halves, whole);
        assert_eq!(whole.rows.values().map(Vec::len).collect::<Vec<_>>(), [1, 1], "one run a row");
        // Taken away again, nothing, kept as nothing.
        assert!(halves.combined(&whole, Combine::Subtract).is_none());
    }

    #[test]
    fn an_ellipse_s_pixels_told_without_the_bisection_are_covered_as_much_as_with_it() {
        for (size, angle, smooth) in [([40.0, 40.0], 0.0, true), ([300.0, 41.0], 30.0, true), ([90.0, 13.0], -70.0, false)]
        {
            let e = Shape { kind: ShapeKind::Ellipse, center: [3.25, -7.5], size, angle, smooth };
            for y in -160..160 {
                for x in -160..160 {
                    let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let d = e.distance(px, py);
                    let exact = if smooth { (0.5 - d).clamp(0.0, 1.0) as f32 } else { f32::from(u8::from(d <= 0.0)) };
                    assert_eq!(level(e.at(px, py)), level(exact), "({x}, {y}) of {size:?} at {angle}");
                }
            }
        }
    }

    #[test]
    fn an_edge_between_pixels_is_shared_by_them() {
        // From 10.5 to 20: pixel 10 half selected.
        let m = Mask::of(&rect(10.5, 0.0, 20.0, 5.0));
        assert_eq!(m.level_at(10, 2), level(0.5));
        assert_eq!(m.level_at(11, 2), FULL);
        // Sharp, the pixel whose middle is on the edge is in.
        let sharp = Mask::of(&Shape { smooth: false, ..rect(10.5, 0.0, 20.0, 5.0) });
        assert_eq!(sharp.level_at(10, 2), FULL);
        assert_eq!(sharp.level_at(9, 2), 0);
    }

    #[test]
    fn an_ellipse_is_as_far_from_its_edge_as_it_says() {
        for (x, y, a, b) in [(3.0, 4.0, 5.0, 5.0), (0.0, 0.0, 10.0, 2.0), (12.0, 0.0, 10.0, 2.0), (1.0, 7.0, 3.0, 9.0)]
        {
            let d = ellipse_distance(x, y, a, b);
            // Against the nearest of many points on it.
            let nearest = (0..20000)
                .map(|i| {
                    let t = i as f64 / 20000.0 * std::f64::consts::TAU;
                    (a * t.cos() - x).hypot(b * t.sin() - y)
                })
                .fold(f64::INFINITY, f64::min);
            assert!((d.abs() - nearest).abs() < 1e-3, "({x}, {y}) on ({a}, {b}): {d} against {nearest}");
            assert_eq!(d < 0.0, (x / a).powi(2) + (y / b).powi(2) < 1.0);
        }
        // A circle of radius 100 about pixel (0, 0)'s middle: the middle fully, pixel 100,
        // whose middle is on the edge, half.
        let c = Shape { kind: ShapeKind::Ellipse, center: [0.5, 0.5], size: [200.0, 200.0], angle: 0.0, smooth: true };
        let m = Mask::of(&c);
        assert_eq!(m.level_at(0, 0), FULL);
        assert_eq!(m.level_at(99, 0), FULL);
        assert_eq!(m.level_at(150, 0), 0);
        let edge = fraction(m.level_at(100, 0));
        assert!((edge - 0.5).abs() < 0.01, "{edge}");
    }

    #[test]
    fn a_turned_rectangle_covers_its_turned_corners() {
        // A square of 100 turned 45°: its corners on the axes, 70.7 from its middle.
        let s = Shape { angle: 45.0, ..rect(-50.0, -50.0, 50.0, 50.0) };
        let m = Mask::of(&s);
        assert_eq!(m.level_at(0, 0), FULL);
        assert_eq!(m.level_at(65, 0), FULL);
        assert_eq!(m.level_at(75, 0), 0);
        assert_eq!(m.level_at(45, 45), 0, "the square's corner was not turned away");
    }

    #[test]
    fn masks_combine_add_subtract_and_intersect() {
        let a = Mask::of(&rect(0.0, 0.0, 100.0, 100.0));
        let b = Mask::of(&rect(50.0, 0.0, 150.0, 100.0));
        let at = |m: &Mask| [m.level_at(25, 50), m.level_at(75, 50), m.level_at(125, 50)];
        assert_eq!(at(&a.combined(&b, Combine::Add)), [FULL, FULL, FULL]);
        assert_eq!(at(&a.combined(&b, Combine::Subtract)), [FULL, 0, 0]);
        assert_eq!(at(&a.combined(&b, Combine::Intersect)), [0, FULL, 0]);
        assert_eq!(at(&a.combined(&b, Combine::Replace)), [0, FULL, FULL]);
        // Subtracting all of it leaves nothing, kept as no tiles.
        assert!(a.combined(&a, Combine::Subtract).is_none());
    }

    #[test]
    fn an_inverted_mask_selects_the_plane_but_for_a_hole() {
        let a = Mask::of(&rect(0.0, 0.0, 100.0, 100.0)).inverted();
        assert_eq!(a.level_at(50, 50), 0);
        assert_eq!(a.level_at(-1_000_000, 7), FULL);
        assert_eq!(a.extent(), Extent::Unbounded);
        // Inverted again, as it was.
        assert_eq!(a.inverted(), Mask::of(&rect(0.0, 0.0, 100.0, 100.0)));
        assert!(Mask::all().inverted().is_none());
    }

    #[test]
    fn the_outline_of_a_rectangle_is_its_four_corners() {
        let m = Mask::of(&rect(10.0, 20.0, 600.0, 300.0));
        let lines = m.outline(HALF);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let mut corners = lines[0].clone();
        corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(corners, [[10.0, 20.0], [10.0, 300.0], [600.0, 20.0], [600.0, 300.0]]);
    }

    #[test]
    fn the_outline_of_a_hole_in_the_plane_and_of_two_shapes_are_closed_lines() {
        // A hole: one line, round it.
        let hole = Mask::of(&rect(0.0, 0.0, 300.0, 300.0)).inverted();
        let lines = hole.outline(HALF);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 4);
        // Two apart: two lines; an ellipse's, round it, each point on its edge.
        let e = Shape {
            kind: ShapeKind::Ellipse,
            center: [1000.0, 1000.0],
            size: [400.0, 200.0],
            angle: 30.0,
            smooth: true,
        };
        let two = Mask::of(&rect(0.0, 0.0, 100.0, 100.0)).combined(&Mask::of(&e), Combine::Add);
        let lines = two.outline(HALF);
        assert_eq!(lines.len(), 2, "{}", lines.len());
        let round = lines.iter().find(|l| l.len() > 4).expect("the ellipse's");
        // Along the pixels' edges: each corner within a pixel's diagonal of the true edge.
        for p in round {
            assert!(e.distance(p[0], p[1]).abs() < 1.0, "{p:?} is {} from the edge", e.distance(p[0], p[1]));
        }
    }

    #[test]
    fn pixels_meeting_only_at_a_corner_have_lines_of_their_own() {
        let a = Mask::of(&rect(0.0, 0.0, 1.0, 1.0));
        let b = Mask::of(&rect(1.0, 1.0, 2.0, 2.0));
        let mut lines = a.combined(&b, Combine::Add).outline(HALF);
        lines.iter_mut().for_each(|l| l.sort_by(|a, b| a.partial_cmp(b).unwrap()));
        lines.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            lines,
            [
                vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]],
                vec![[1.0, 1.0], [1.0, 2.0], [2.0, 1.0], [2.0, 2.0]]
            ]
        );
    }

    #[test]
    fn blending_through_a_mask_changes_what_it_selects_as_much_as_it_selects_it() {
        let red = crate::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let image = RasterImage::filled(&IRect::new(0, 0, 600, 100), red);
        // The left half selected, pixel 299 half; the image 1000 to the right of the origin.
        let mask = Mask::of(&rect(1000.0, 0.0, 1299.5, 100.0));
        let cleared = clear_selected(&image, [1000, 0], &mask);
        assert_eq!(blend(&image, [1000, 0], &mask, |_| [0.0; 4]).pixel(299, 50), cleared.pixel(299, 50));
        assert_eq!(cleared.pixel(10, 50), crate::tile::pixel_from_f32([0.0; 4]));
        assert_eq!(cleared.pixel(400, 50), red);
        let half = crate::tile::pixel_to_f32(cleared.pixel(299, 50));
        assert!((half[3] - 0.5).abs() < 0.01, "{half:?}");
        // The tiles it leaves alone are the same tiles.
        let shared = |a: &RasterImage, b: &RasterImage, c| Arc::ptr_eq(a.tile(c).unwrap(), b.tile(c).unwrap());
        assert!(shared(&image, &cleared, TileCoord::new(2, 0)));
        // Wholly cleared tiles go.
        assert!(cleared.tile(TileCoord::new(0, 0)).is_none());
    }

    #[test]
    fn keeping_what_is_selected_keeps_as_much_of_each_pixel_as_is_selected() {
        let red = crate::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let image = RasterImage::filled(&IRect::new(0, 0, 600, 100), red);
        let mask = Mask::of(&rect(0.0, 0.0, 299.5, 100.0));
        let kept = keep_selected(&image, [0, 0], &mask);
        assert_eq!(kept.pixel(10, 50), red);
        assert_eq!(kept.pixel(400, 50), crate::tile::pixel_from_f32([0.0; 4]));
        let half = crate::tile::pixel_to_f32(kept.pixel(299, 50));
        assert!((half[3] - 0.5).abs() < 0.01, "{half:?}");
        // Wholly selected tiles are the same tiles; wholly unselected ones go.
        assert!(Arc::ptr_eq(image.tile(TileCoord::new(0, 0)).unwrap(), kept.tile(TileCoord::new(0, 0)).unwrap()));
        assert!(kept.tile(TileCoord::new(2, 0)).is_none());
    }
}
