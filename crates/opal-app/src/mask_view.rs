//! The selection's mask drawn over the canvas as a red overlay (design.md, *The mask*; View ›
//! Selection Overlay): red where nothing is selected, fading out as pixels are more selected,
//! so that a soft selection's falloff shows.

use eframe::egui::{self, Color32, ColorImage, Mesh, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use opal_core::mask::{Mask, MaskTile, fraction};
use opal_core::tile::{TILE_SIZE, TileCoord};
use std::collections::HashMap;

/// What the overlay keeps of a tile with levels of its own: its mean level, and, once drawn
/// large enough to need one, its texture.
struct Kept {
    mean: u16,
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
            let levels = match tile {
                MaskTile::Level(l) => {
                    fill(x0, x1, y, tint(*l, opacity));
                    continue;
                }
                MaskTile::Columns(l) | MaskTile::Rows(l) | MaskTile::Levels(l) => l,
            };
            let key = std::sync::Arc::as_ptr(levels).cast::<u16>() as usize;
            let kept = drawn.entry(key).or_insert_with(|| {
                textures.kept.remove(&key).unwrap_or_else(|| Kept { mean: mean(tile), texture: None })
            });
            if across < SMALL {
                fill(x0, x1, y, tint(kept.mean, opacity));
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
    let side = TILE_SIZE as usize;
    let sum: u64 = (0..TILE_SIZE).flat_map(|y| (0..TILE_SIZE).map(move |x| u64::from(tile.get(x, y)))).sum();
    (sum / (side * side) as u64) as u16
}

/// `tile` as an image the overlay draws, red with no selection, clear with all, before its
/// opacity is applied.
fn image_of(tile: &MaskTile) -> ColorImage {
    let side = TILE_SIZE as usize;
    let pixels = (0..TILE_SIZE)
        .flat_map(|y| (0..TILE_SIZE).map(move |x| tile.get(x, y)))
        .map(|l| {
            let a = ((1.0 - fraction(l)) * 255.0).round() as u8;
            Color32::from_rgba_premultiplied(a, 0, 0, a)
        })
        .collect();
    ColorImage::new([side, side], pixels)
}
