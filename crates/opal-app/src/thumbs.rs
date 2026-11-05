//! Thumbnails: of each layer in the Layers panel, and of the image at a history state.
//!
//! A thumbnail averages a few point samples per thumbnail pixel
//! ([`opal_core::composite::point`]), which costs a millisecond or two however large the
//! image, so thumbnails are made when first shown and remade only when what they show
//! changes: a layer is replaced (its `Arc`) whenever it changes, and a history state never
//! changes.

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use opal_core::composite;
use opal_core::document::{Document, Layer};
use opal_core::geom::IRect;
use opal_core::tile::pixel_from_f32;
use opal_io::pixels::to_srgb8;
use std::collections::HashMap;
use std::sync::Arc;

struct Thumb {
    /// What it was made from, held so the addresses it is known by cannot be reused by
    /// something else while it is kept.
    _held: Vec<Arc<Layer>>,
    texture: TextureHandle,
    /// The frame it was last shown in.
    used: u64,
}

/// What identifies a thumbnail: what it shows (by address), the canvas, and its size.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    source: usize,
    canvas: IRect,
    size: u32,
}

#[derive(Default)]
pub struct Thumbnails {
    cache: HashMap<Key, Thumb>,
    frame: u64,
}

/// Frames a thumbnail may go unshown before it is dropped.
const KEEP_FRAMES: u64 = 600;

impl Thumbnails {
    /// The thumbnail of `layer`'s content within `canvas`, fitting `size` pixels.
    pub fn layer(&mut self, ctx: &egui::Context, layer: &Arc<Layer>, canvas: IRect, size: u32) -> TextureHandle {
        let key = Key { source: Arc::as_ptr(layer) as usize, canvas, size };
        let frame = self.frame;
        let entry = self.cache.entry(key).or_insert_with(|| {
            let l = layer.clone();
            let image = render(canvas, size, &|x, y| composite::layer_point(&l, x, y));
            Thumb { _held: vec![layer.clone()], texture: load(ctx, image), used: frame }
        });
        entry.used = frame;
        entry.texture.clone()
    }

    /// The thumbnail of all of `doc` within its canvas, fitting `size` pixels.
    pub fn document(&mut self, ctx: &egui::Context, doc: &Document, size: u32) -> TextureHandle {
        let canvas = doc.canvas.rect();
        // The top-level layers stand for the document: any change replaces one of them.
        let mut h = std::hash::DefaultHasher::new();
        for l in &doc.layers {
            std::hash::Hash::hash(&(Arc::as_ptr(l) as usize), &mut h);
        }
        let key = Key { source: std::hash::Hasher::finish(&h) as usize, canvas, size };
        let frame = self.frame;
        let entry = self.cache.entry(key).or_insert_with(|| {
            let layers = doc.layers.clone();
            let image = render(canvas, size, &|x, y| composite::point(&layers, x, y));
            Thumb { _held: doc.layers.clone(), texture: load(ctx, image), used: frame }
        });
        entry.used = frame;
        entry.texture.clone()
    }

    /// Drops thumbnails no longer shown; call once a frame.
    pub fn end_frame(&mut self) {
        let frame = self.frame;
        self.cache.retain(|_, t| frame - t.used < KEEP_FRAMES);
        self.frame += 1;
    }
}

fn load(ctx: &egui::Context, image: ColorImage) -> TextureHandle {
    ctx.load_texture("thumbnail", image, TextureOptions::LINEAR)
}

/// A thumbnail of `canvas` fitting `size × size`, each pixel the average of up to 6 × 6
/// samples of `at` over the canvas pixels it covers.
fn render(canvas: IRect, size: u32, at: &dyn Fn(f64, f64) -> [f32; 4]) -> ColorImage {
    let (cw, ch) = (canvas.width().max(1) as f64, canvas.height().max(1) as f64);
    let k = f64::from(size.max(1)) / cw.max(ch);
    let (w, h) = (((cw * k).round() as usize).max(1), ((ch * k).round() as usize).max(1));
    let step = 1.0 / k;
    let n = (step.ceil() as usize).clamp(1, 6);
    let mut px = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let mut acc = [0.0f32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let x = canvas.x0 as f64 + (i as f64 + (sx as f64 + 0.5) / n as f64) * step;
                    let y = canvas.y0 as f64 + (j as f64 + (sy as f64 + 0.5) / n as f64) * step;
                    let p = at(x, y);
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
            }
            let avg = acc.map(|v| v / (n * n) as f32);
            let [r, g, b, a] = to_srgb8(pixel_from_f32(avg));
            px.push(egui::Color32::from_rgba_unmultiplied(r, g, b, a));
        }
    }
    ColorImage { size: [w, h], pixels: px, source_size: egui::Vec2::new(w as f32, h as f32) }
}
