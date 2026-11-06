//! Times exporting and saving a large document, and what the largest canvas costs, and
//! measures the memory each takes beyond the document (how far each raises the process's
//! peak, so a step after a larger one shows only what it adds past that). Run by hand, one at a time so that each gets a process of its own:
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
    measure("Save", || opal_io::native::save(&d, &ids, &path, &opal_io::Progress::default()).unwrap());
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

/// A `side × side` canvas filled white as New Image fills it.
fn white(side: u32) -> (Document, IdGen) {
    let area = opal_core::geom::IRect::from_size(i64::from(side), i64::from(side));
    let img = RasterImage::filled(&area, pixel_from_f32([1.0; 4]));
    let mut ids = IdGen::default();
    let mut d = Document::new(side, side);
    d.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img)).unwrap();
    (d, ids)
}

/// What the largest canvas costs to make, show whole, change, merge and save.
#[test]
#[ignore]
fn huge_white_canvas() {
    use opal_core::composite::{Projection, Quality, composite_all, composite_cost};
    let side = 300_000;
    let mut built = None;
    measure("New 300,000-pixel white canvas", || built = Some(white(side)));
    let (mut d, ids) = built.unwrap();
    let whole = opal_core::geom::IRect::from_size(i64::from(side), i64::from(side));
    let mut p = Projection::new();
    p.set_budget(Some(1 << 30));
    // About what fits it in a window: level 9 (1/512).
    measure("Shown whole", || {
        p.update(&d.layers, 9, &whole, Quality::Exact);
    });
    measure("Shown whole again, unchanged", || {
        p.update(&d.layers, 9, &whole, Quality::Exact);
    });
    let mut dot = RasterImage::new();
    for y in 1000..1100 {
        for x in 1000..1100 {
            dot.set_pixel(x, y, pixel_from_f32([1.0, 0.0, 0.0, 0.5]));
        }
    }
    let mut ids2 = ids.clone();
    d.insert(Position { parent: None, index: 1 }, Layer::new_raster(ids2.layer(), ids2.object(), "dot", dot)).unwrap();
    measure("Shown whole after a layer is added", || {
        p.update(&d.layers, 9, &whole, Quality::Exact);
    });
    let mut cost = 0;
    measure("Cost of merging, found", || cost = composite_cost(&d.layers, None));
    eprintln!("  merging makes {} MB", cost >> 20);
    measure("Merged", || {
        composite_all(&d.layers);
    });
    let path = temp("huge.opal");
    measure("Saved", || opal_io::native::save(&d, &ids2, &path, &opal_io::Progress::default()).unwrap());
    eprintln!("  file: {} MB", std::fs::metadata(&path).unwrap().len() >> 20);
    let mut loaded = None;
    measure("Opened", || loaded = Some(opal_io::native::load(&path, None).unwrap()));
    std::fs::remove_file(path).unwrap();
}

/// Opening a large PNG: more than 512 MiB once decoded.
#[test]
#[ignore]
fn import_large_png() {
    // 8200 × 8200 at 16 bits a channel: 538 MB decoded. Written a row at a time.
    let side = 8200u32;
    let path = temp("large.png");
    {
        let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut encoder = png::Encoder::new(file, side, side);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().unwrap();
        let mut stream = writer.stream_writer().unwrap();
        let row = vec![0x80u8; side as usize * 8];
        for _ in 0..side {
            std::io::Write::write_all(&mut stream, &row).unwrap();
        }
        stream.finish().unwrap();
    }
    let mut opened = None;
    measure("Opened an 8200-pixel square 16-bit PNG", || opened = Some(opal_io::image_files::import(&path, None)));
    std::fs::remove_file(&path).unwrap();
    let imported = opened.unwrap().expect("opened");
    assert_eq!((imported.width, imported.height), (side, side));
}

/// Exporting a document three times larger than the memory its tiles may take: the tiles
/// go to disk and come back as the export reads them.
#[test]
#[ignore]
fn export_spilled() {
    let (one, mut ids) = large();
    let mut d = one.clone();
    for i in 1..3 {
        let mut layer = (*one.layers[0]).clone();
        layer.id = ids.layer();
        layer.opacity = 0.5;
        // Its own object, and its own tiles, so that the three take three times the memory.
        let opal_core::document::LayerContent::Objects(objects) = &mut layer.content else { unreachable!() };
        objects[0].id = ids.object();
        if let opal_core::document::ObjectKind::Pixels(p) = &mut objects[0].kind {
            Arc::make_mut(&mut p.image).set_pixel(i, 0, pixel_from_f32([1.0; 4]));
            let copied: Vec<_> = p.image.tiles().map(|(c, t)| (c, Arc::new((**t).clone()))).collect();
            let image = Arc::make_mut(&mut p.image);
            for (c, t) in copied {
                image.set_tile(c, Some(t));
            }
        }
        d.insert(Position { parent: None, index: i as usize }, layer).unwrap();
    }
    opal_core::store::set_budget(Some(200 << 20));
    let path = temp("spilled.png");
    measure("PNG of 3 × 192 MB layers, 200 MB in memory", || {
        export(&d, &path, &ExportFormat::Png { sixteen_bit: false }, None).unwrap()
    });
    let s = opal_core::store::status();
    eprintln!("  {} MB in memory, {} MB on disk", s.resident >> 20, s.on_disk >> 20);
    std::fs::remove_file(path).unwrap();
    // Saved, let go of, and opened again: its tiles go to disk as they are read.
    let saved = temp("spilled.opal");
    opal_io::native::save(&d, &ids, &saved, &opal_io::Progress::default()).unwrap();
    drop((d, one));
    opal_core::store::settle();
    let mut opened = None;
    measure("Opened, 200 MB in memory", || opened = Some(opal_io::native::load(&saved, None).unwrap()));
    let s = opal_core::store::status();
    eprintln!("  {} MB in memory, {} MB on disk", s.resident >> 20, s.on_disk >> 20);
    assert!(s.resident <= 200 << 20, "opening took memory past the budget");
    assert_eq!(opened.unwrap().doc.layers.len(), 3);
    opal_core::store::set_budget(None);
    std::fs::remove_file(saved).unwrap();
}
