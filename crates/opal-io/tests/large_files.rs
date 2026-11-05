//! Times exporting and saving a large document, and measures the memory each takes beyond
//! the document. Run by hand, one at a time so that each gets a process of its own:
//! `cargo test --release -p opal-io --test large_files -- --ignored --nocapture --test-threads 1 <name>`.

use opal_core::document::{Document, IdGen, Layer, Position};
use opal_core::raster::RasterImage;
use opal_core::tile::{TILE_SIZE, Tile, TileCoord, pixel_from_f32};
use opal_io::image_files::{ExportFormat, export};
use std::sync::Arc;
use std::time::Instant;

/// A 6000 × 4000 document of one layer whose pixels all differ, built tile by tile (so that
/// building it needs no more memory than it holds).
fn large() -> (Document, IdGen) {
    let (w, h) = (6000i64, 4000i64);
    let mut img = RasterImage::new();
    for c in TileCoord::covering(&opal_core::geom::IRect::from_size(w, h), 0) {
        let r = c.rect(0);
        let mut t = Tile::transparent();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let (gx, gy) = (r.x0 + i64::from(x), r.y0 + i64::from(y));
                if gx < w && gy < h {
                    let v = ((gx * 7 + gy * 13) % 1000) as f32 / 1000.0;
                    t.set(x, y, pixel_from_f32([v, 1.0 - v, (gx % 256) as f32 / 255.0, 1.0]));
                }
            }
        }
        img.set_tile(c, Some(Arc::new(t)));
    }
    let mut ids = IdGen::default();
    let mut d = Document::new(w as u32, h as u32);
    d.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img)).unwrap();
    (d, ids)
}

/// The most memory the process has had committed so far, in bytes.
#[cfg(windows)]
fn peak_commit() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_non_paged: usize,
        quota_non_paged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
    // SAFETY: `c` is a correctly sized PROCESS_MEMORY_COUNTERS, and the handle is the
    // pseudo-handle of this process.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    assert!(ok != 0, "the memory counters could not be read");
    c.peak_pagefile
}

#[cfg(not(windows))]
fn peak_commit() -> usize {
    0
}

fn measure(what: &str, f: impl FnOnce()) {
    let before = peak_commit();
    let t = Instant::now();
    f();
    let took = t.elapsed();
    let extra = peak_commit().saturating_sub(before);
    eprintln!("{what}: {took:?}, {} MB beyond the document", extra >> 20);
}

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("opal-large-{}-{name}", std::process::id()))
}

#[test]
#[ignore]
fn export_png8() {
    let (d, _) = large();
    let path = temp("8.png");
    measure("PNG, 8 bits", || export(&d, &path, &ExportFormat::Png { sixteen_bit: false }, None).unwrap());
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore]
fn export_png16() {
    let (d, _) = large();
    let path = temp("16.png");
    measure("PNG, 16 bits", || export(&d, &path, &ExportFormat::Png { sixteen_bit: true }, None).unwrap());
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore]
fn export_jpeg() {
    let (d, _) = large();
    let path = temp("q90.jpg");
    measure("JPEG", || {
        export(&d, &path, &ExportFormat::Jpeg { quality: 90, background: [255, 255, 255] }, None).unwrap()
    });
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore]
fn export_resized() {
    let (d, _) = large();
    let path = temp("half.png");
    let size = opal_io::image_files::ExportSize {
        width: 3000,
        height: 2000,
        sampling: opal_core::resample::Sampling::default(),
    };
    measure("PNG at half size", || export(&d, &path, &ExportFormat::Png { sixteen_bit: false }, Some(&size)).unwrap());
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore]
fn save_native() {
    let (d, ids) = large();
    let path = temp("doc.opal");
    measure("Save", || opal_io::native::save(&d, &ids, &path).unwrap());
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore]
fn export_tiny() {
    let (d, _) = large();
    let path = temp("tiny.png");
    let size =
        opal_io::image_files::ExportSize { width: 20, height: 13, sampling: opal_core::resample::Sampling::default() };
    measure("PNG at 20 x 13", || export(&d, &path, &ExportFormat::Png { sixteen_bit: false }, Some(&size)).unwrap());
    std::fs::remove_file(path).unwrap();
}
