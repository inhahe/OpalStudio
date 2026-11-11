//! The selection's mask drawn over the canvas as a red overlay (design.md, *The mask*; View ›
//! Selection Overlay): red where nothing is selected, fading out as pixels are more selected,
//! so that a soft selection's falloff shows.

use eframe::egui::{self, Color32, ColorImage, Mesh, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use opal_core::mask::{Mask, MaskTile, fraction};
use opal_core::tile::{TILE_SIZE, TileCoord};
use std::collections::HashMap;

/// What the overlay keeps of a tile with levels of its own: its levels, weakly (so that, while
/// they are kept, no other tile's are kept where they are, under the same key), and, once
/// needed, its mean level and its texture.
struct Kept {
    _levels: std::sync::Weak<[u16]>,
    mean: Option<u16>,
    texture: Option<TextureHandle>,
}

/// What the overlay keeps of the tiles with levels of their own it draws, by where their
/// levels are kept (alike tiles share theirs, and so one texture); only those drawn last
/// frame are kept.
#[derive(Default)]
pub struct Textures {
    kept: HashMap<usize, Kept>,
}

/// Below this many points across, a tile with levels of its own is drawn as its mean level.
const SMALL: f32 = 12.0;

/// The overlay's red, how much a pixel selected at `level` is covered by it, `opacity` being
/// how much one not selected at all is.
fn tint(level: u16, opacity: f32) -> Color32 {
    let a = ((1.0 - fraction(level)) * opacity * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(255, 0, 0, a)
}

/// Draws `mask` over `canvas` (document pixels), shown in `rect` (screen points) through
/// `view` (`ppp` device pixels a point): red, `opacity` of it where nothing is selected, less
/// as pixels are more selected; past the canvas, where nothing is drawn, nothing. Uniform
/// stretches are drawn as rectangles; tiles with levels of their own as textures (`textures`
/// keeps them), or, too small to tell, as their mean level.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &egui::Ui,
    mask: &Mask,
    canvas: &opal_core::geom::IRect,
    rect: Rect,
    view: crate::doc::View,
    ppp: f32,
    opacity: f32,
    textures: &mut Textures,
) {
    let to_doc = |p: Pos2| view.to_canvas((p - rect.min) * ppp);
    let to_screen = |x: i64, y: i64| rect.min + view.to_screen(Vec2::new(x as f32, y as f32)) / ppp;
    let shown = rect.intersect(Rect::from_min_max(to_screen(canvas.x0, canvas.y0), to_screen(canvas.x1, canvas.y1)));
    if !shown.is_positive() {
        textures.kept.clear();
        return;
    }
    let painter = ui.painter().with_clip_rect(shown);
    let (a, b) = (to_doc(shown.min), to_doc(shown.max));
    let first = TileCoord::containing(a.x.floor() as i64, a.y.floor() as i64);
    let last = TileCoord::containing(b.x.ceil() as i64, b.y.ceil() as i64);
    let side = i64::from(TILE_SIZE);
    let across = TILE_SIZE as f32 * view.zoom / ppp;
    let fill = |x0: i32, x1: i32, y: i32, color: Color32| {
        if color.a() > 0 && x0 < x1 {
            let (y0, y1) = (i64::from(y) * side, i64::from(y + 1) * side);
            let r = Rect::from_min_max(to_screen(i64::from(x0) * side, y0), to_screen(i64::from(x1) * side, y1));
            painter.rect_filled(r, 0.0, color);
        }
    };
    let mut drawn = HashMap::new();
    for y in first.y..=last.y {
        // Along the row, the stretches between kept tiles are of the level outside.
        let mut at = first.x;
        for (x0, x1, tile) in mask.row_runs(y) {
            let (x0, x1) = (x0.max(first.x), x1.min(last.x + 1));
            if x0 >= x1 {
                continue;
            }
            fill(at, x0, y, tint(mask.outside(), opacity));
            at = x1;
            if let MaskTile::Level(l) = tile {
                fill(x0, x1, y, tint(*l, opacity));
                continue;
            }
            let key = tile.address().expect("a tile with levels of its own");
            let kept = drawn.entry(key).or_insert_with(|| {
                textures.kept.remove(&key).unwrap_or_else(|| Kept {
                    _levels: tile.levels_weakly().expect("a tile with levels of its own"),
                    mean: None,
                    texture: None,
                })
            });
            if across < SMALL {
                let mean = *kept.mean.get_or_insert_with(|| mean(tile));
                fill(x0, x1, y, tint(mean, opacity));
                continue;
            }
            let texture = kept.texture.get_or_insert_with(|| {
                ui.ctx().load_texture(
                    format!("selection-overlay-{key:x}"),
                    image_of(tile),
                    TextureOptions {
                        magnification: egui::TextureFilter::Nearest,
                        minification: egui::TextureFilter::Linear,
                        ..TextureOptions::default()
                    },
                )
            });
            let corner = TileCoord::new(x0, y).rect(0);
            let r = Rect::from_min_max(to_screen(corner.x0, corner.y0), to_screen(corner.x1, corner.y1));
            let mut mesh = Mesh::with_texture(texture.id());
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            mesh.add_rect_with_uv(r, uv, Color32::from_white_alpha((opacity * 255.0).round() as u8));
            painter.add(egui::Shape::mesh(mesh));
        }
        fill(at, last.x + 1, y, tint(mask.outside(), opacity));
    }
    textures.kept = drawn;
}

/// The mean level of `tile`.
fn mean(tile: &MaskTile) -> u16 {
    let levels: &[u16] = match tile {
        MaskTile::Level(l) => return *l,
        MaskTile::Columns(l) | MaskTile::Rows(l) | MaskTile::Levels(l) => l,
    };
    (levels.iter().map(|&l| u64::from(l)).sum::<u64>() / levels.len() as u64) as u16
}

/// `tile` as an image the overlay draws (stretched over the tile), red with no selection,
/// clear with all, before its opacity is applied: a tile of a level a column or a row, one
/// line of them.
fn image_of(tile: &MaskTile) -> ColorImage {
    let side = TILE_SIZE as usize;
    let (size, levels): ([usize; 2], &[u16]) = match tile {
        MaskTile::Columns(l) => ([side, 1], l),
        MaskTile::Rows(l) => ([1, side], l),
        MaskTile::Levels(l) => ([side, side], l),
        MaskTile::Level(_) => unreachable!("a tile of one level is drawn as a rectangle"),
    };
    let pixels = levels
        .iter()
        .map(|&l| {
            let a = ((1.0 - fraction(l)) * 255.0).round() as u8;
            Color32::from_rgba_premultiplied(a, 0, 0, a)
        })
        .collect();
    ColorImage::new(size, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_of_one_line_of_levels_is_one_line_of_texture_and_its_mean_that_line_s() {
        let line: std::sync::Arc<[u16]> = (0..256u32).map(|i| (i * 256) as u16).collect();
        let columns = MaskTile::Columns(line.clone());
        assert_eq!(image_of(&columns).size, [256, 1]);
        assert_eq!(image_of(&MaskTile::Rows(line)).size, [1, 256]);
        let mean_of_line = ((0..256u64).map(|i| i * 256).sum::<u64>() / 256) as u16;
        assert_eq!(mean(&columns), mean_of_line);
    }
}
