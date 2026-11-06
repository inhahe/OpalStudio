//! Tiles going to disk past a memory budget, and coming back ([`opal_core::store`]). A test
//! binary of its own, since the store is the process's: these share it, one at a time.

use opal_core::store::{self, set_budget, status};
use opal_core::tile::{TILE_PIXELS, Tile, pixel_from_f32};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::sync::Arc;

/// The memory a tile of working pixels takes.
const TILE: u64 = (TILE_PIXELS * 8) as u64;

/// One test at a time, each starting with no budget and no tiles.
static TURN: Mutex<()> = Mutex::new(());

/// A tile all of whose pixels say `n`.
fn tile(n: usize) -> Tile {
    Tile::filled(pixel_from_f32([n as f32 / 1000.0, 0.5, 0.25, 1.0]))
}

fn is(t: &Tile, n: usize) -> bool {
    t.get(17, 200) == pixel_from_f32([n as f32 / 1000.0, 0.5, 0.25, 1.0])
}

/// Runs `f` with a budget of `tiles` tiles, then lifts it (even if `f` fails).
fn with_budget(tiles: u64, f: impl FnOnce()) {
    struct Lift;
    impl Drop for Lift {
        fn drop(&mut self) {
            set_budget(None);
        }
    }
    let _turn = TURN.lock();
    store::settle();
    assert_eq!(status().resident, 0, "a test left tiles behind");
    set_budget(Some(tiles * TILE));
    let _lift = Lift;
    f();
}

#[test]
fn past_the_budget_tiles_go_to_disk_and_read_back_as_they_were() {
    with_budget(10, || {
        let tiles: Vec<Arc<Tile>> = (0..40).map(|n| Arc::new(tile(n))).collect();
        let s = status();
        assert!(s.resident <= 10 * TILE, "{} tiles in memory", s.resident / TILE);
        assert!(s.on_disk >= 30 * TILE, "{} tiles on disk", s.on_disk / TILE);
        assert_eq!(s.trouble, None);
        for (n, t) in tiles.iter().enumerate() {
            assert!(is(t, n), "tile {n} came back wrong");
        }
        assert!(status().resident <= 10 * TILE, "reading them back took memory past the budget");
        drop(tiles);
        // The store's thread may be holding one it is writing.
        store::settle();
        assert_eq!((status().resident, status().on_disk), (0, 0), "let go, they take nothing");
    });
}

#[test]
fn a_tile_brought_back_unchanged_goes_again_without_being_written() {
    with_budget(4, || {
        let tiles: Vec<Arc<Tile>> = (0..12).map(|n| Arc::new(tile(n))).collect();
        // Read back again and again, each keeps its one copy: copies written again would
        // take more than the twelve tiles' worth.
        for _ in 0..3 {
            for (n, t) in tiles.iter().enumerate() {
                assert!(is(t, n));
            }
        }
        assert!(status().on_disk <= 12 * TILE, "copies were written again: {} tiles' worth", status().on_disk / TILE);
    });
}

#[test]
fn changing_a_tile_on_disk_brings_it_back_and_lets_its_copy_go() {
    with_budget(2, || {
        let mut tiles: Vec<Arc<Tile>> = (0..8).map(|n| Arc::new(tile(n))).collect();
        let first = &mut tiles[0];
        let changed = Arc::make_mut(first);
        changed.set(17, 200, pixel_from_f32([1.0; 4]));
        assert_eq!(changed.get(17, 200), pixel_from_f32([1.0; 4]));
        assert_eq!(changed.get(0, 0), pixel_from_f32([0.0, 0.5, 0.25, 1.0]), "the rest came back");
        for (n, t) in tiles.iter().enumerate().skip(1) {
            assert!(is(t, n));
        }
        // Read again, the changed one reads as changed.
        assert_eq!(tiles[0].get(17, 200), pixel_from_f32([1.0; 4]));
        // Brought back (keeping its copy on disk), then changed: going again, it is written
        // as it is now, not left as the copy was.
        for (n, t) in tiles.iter().enumerate().skip(2) {
            assert!(is(t, n));
        }
        assert!(is(&tiles[1], 1));
        Arc::make_mut(&mut tiles[1]).set(17, 200, pixel_from_f32([0.5; 4]));
        for (n, t) in tiles.iter().enumerate().skip(2) {
            assert!(is(t, n));
        }
        assert_eq!(tiles[1].get(17, 200), pixel_from_f32([0.5; 4]), "the copy from before the change came back");
    });
}

#[test]
fn eight_bit_tiles_go_and_come_back_eight_bit_and_copies_are_copies() {
    with_budget(2, || {
        let rgba: Vec<[u8; 4]> = (0..TILE_PIXELS).map(|i| [i as u8, (i / 256) as u8, 9, 255]).collect();
        let eight: Vec<Arc<Tile>> = (0..10).map(|_| Arc::new(Tile::from_srgb8(rgba.clone()))).collect();
        assert!(status().on_disk > 0);
        let want = Tile::from_srgb8(rgba.clone());
        for t in &eight {
            assert!(t.is_srgb8() && **t == want);
        }
        let copy = (*eight[0]).clone();
        assert!(copy == want && copy.is_srgb8());
    });
}

#[test]
fn tiles_read_in_parallel_while_others_go_and_come_all_read_right() {
    with_budget(16, || {
        let tiles: Vec<Arc<Tile>> = (0..200).map(|n| Arc::new(tile(n))).collect();
        for _ in 0..3 {
            let wrong = tiles.par_iter().enumerate().filter(|(n, t)| !is(t, *n)).count();
            assert_eq!(wrong, 0);
        }
        // Made in parallel too, past the budget all the while.
        let more: Vec<Arc<Tile>> = (200..400).into_par_iter().map(|n| Arc::new(tile(n))).collect();
        assert!(more.par_iter().enumerate().all(|(n, t)| is(t, n + 200)));
        assert!(status().resident <= 16 * TILE + 32 * TILE, "memory ran far past the budget");
    });
}

#[test]
fn changing_tiles_on_disk_keeps_memory_within_the_budget() {
    with_budget(3, || {
        let mut tiles: Vec<Arc<Tile>> = (0..12).map(|n| Arc::new(tile(n))).collect();
        assert!(status().on_disk >= 8 * TILE);
        for (n, t) in tiles.iter_mut().enumerate() {
            Arc::make_mut(t).set(0, 0, pixel_from_f32([1.0; 4]));
            assert!(status().resident <= 3 * TILE, "changing tile {n} took memory past the budget");
        }
        for (n, t) in tiles.iter().enumerate() {
            assert!(is(t, n) && t.get(0, 0) == pixel_from_f32([1.0; 4]));
        }
    });
}

/// Waits up to a few seconds for `done`.
fn eventually(done: impl Fn() -> bool) -> bool {
    let start = std::time::Instant::now();
    while !done() {
        if start.elapsed() > std::time::Duration::from_secs(5) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    true
}

#[test]
fn a_lower_budget_is_kept_by_the_store_s_own_thread_down_to_nine_tenths() {
    with_budget(1000, || {
        let tiles: Vec<Arc<Tile>> = (0..40).map(|n| Arc::new(tile(n))).collect();
        assert_eq!(status().on_disk, 0);
        set_budget(Some(20 * TILE));
        assert!(eventually(|| status().resident <= 18 * TILE), "{} tiles still in memory", status().resident / TILE);
        assert!(status().on_disk >= 22 * TILE);
        for (n, t) in tiles.iter().enumerate() {
            assert!(is(t, n));
        }
    });
}

#[test]
fn what_a_tile_on_disk_holds_is_known_without_bringing_it_back() {
    with_budget(2, || {
        let dot = |n: usize| {
            let mut t = Tile::transparent();
            t.set(n as u32, 3, pixel_from_f32([1.0; 4]));
            Arc::new(t)
        };
        let tiles: Vec<Arc<Tile>> = (0..10).map(dot).collect();
        let empty: Vec<Arc<Tile>> = (0..4).map(|_| Arc::new(Tile::transparent())).collect();
        // Found once (bringing each back), then kept.
        for (n, t) in tiles.iter().enumerate() {
            assert_eq!(t.content_rect(), Some([n as u32, 3, n as u32 + 1, 4]));
        }
        assert!(empty.iter().all(|t| t.is_transparent()));
        store::settle();
        let before = status().read_back;
        for (n, t) in tiles.iter().enumerate() {
            assert_eq!(t.content_rect(), Some([n as u32, 3, n as u32 + 1, 4]));
            assert!(!t.is_transparent());
        }
        assert!(empty.iter().all(|t| t.is_transparent() && t.content_rect().is_none()));
        assert_eq!(status().read_back, before, "asking brought tiles back");
        // Changed, it is found again.
        let mut changed = (*tiles[0]).clone();
        assert_eq!(changed.content_rect(), Some([0, 3, 1, 4]));
        changed.set(200, 100, pixel_from_f32([1.0; 4]));
        assert_eq!(changed.content_rect(), Some([0, 3, 201, 101]));
    });
}

#[test]
fn an_eight_bit_image_made_past_the_budget_reads_no_tile_back() {
    with_budget(4, || {
        let (w, h) = (256 * 8, 256 * 4);
        // Half its tiles one colour, the rest each of their own.
        let rgba: Vec<[u8; 4]> =
            (0..w * h).map(|i| if i % w < w / 2 { [9, 8, 7, 255] } else { [(i % 251) as u8, 4, 3, 255] }).collect();
        let before = status().read_back;
        let img = opal_core::raster::RasterImage::from_srgb8(w as u32, h as u32, &rgba);
        assert_eq!(status().read_back, before, "tiles were read back while being made");
        assert_eq!(img.tile_count(), 32);
        assert_eq!(img.read_rect_srgb8(&opal_core::geom::IRect::from_size(w as i64, h as i64)), Some(rgba));
    });
}

#[test]
fn a_cap_held_keeps_tiles_within_it_until_let_go() {
    with_budget(100, || {
        let mut tiles: Vec<Arc<Tile>> = (0..20).map(|n| Arc::new(tile(n))).collect();
        assert_eq!(status().on_disk, 0);
        let hold = store::hold(8 * TILE, 3);
        assert!(status().resident <= 8 * TILE, "holding the cap took {} tiles' worth away", status().resident / TILE);
        // Another, wider, held meanwhile: the lower applies; lowered, it applies.
        let mut wider = store::hold(50 * TILE, 0);
        assert!(status().resident <= 8 * TILE);
        assert_eq!(store::set_aside(), 3, "what work set aside to take in one go");
        wider.lower(5 * TILE, 4);
        assert_eq!(store::set_aside(), 7);
        assert!(status().resident <= 5 * TILE, "a cap lowered took {} tiles' worth away", status().resident / TILE);
        tiles.extend((20..30).map(|n| Arc::new(tile(n))));
        assert!(status().resident <= 5 * TILE, "tiles made under the cap went past it");
        drop((hold, wider));
        assert_eq!(store::set_aside(), 0, "let go of, the caps set nothing aside");
        store::settle();
        let on_disk = status().on_disk;
        tiles.extend((30..40).map(|n| Arc::new(tile(n))));
        assert_eq!(status().on_disk, on_disk, "the cap let go of, tiles made stayed within the budget");
        for (n, t) in tiles.iter().enumerate() {
            assert!(is(t, n));
        }
    });
}

#[test]
fn a_cap_held_takes_away_only_what_is_past_it_leaving_the_budget_to_the_store_s_thread() {
    with_budget(1000, || {
        let tiles: Vec<Arc<Tile>> = (0..40).map(|n| Arc::new(tile(n))).collect();
        // The store's thread is kept from its turn while the budget is lowered and a cap
        // above what tiles take is held: the cap takes nothing away itself.
        let turn = store::hold_queue_for_tests();
        set_budget(Some(10 * TILE));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(store::hold(100 * TILE, 0)).unwrap());
        let wide = rx.recv_timeout(std::time::Duration::from_secs(2));
        let on_disk = status().on_disk;
        drop(turn);
        let wide = wide.expect("a cap above what tiles take waited to take them down to the budget");
        assert_eq!(on_disk, 0, "a cap above what tiles take took them down to the budget");
        assert!(eventually(|| status().resident <= 10 * TILE));
        drop((wide, tiles));
    });
}

#[test]
fn without_a_budget_nothing_goes() {
    let _turn = TURN.lock();
    store::settle();
    let tiles: Vec<Arc<Tile>> = (0..20).map(|n| Arc::new(tile(n))).collect();
    let held = store::hold(TILE, 0);
    assert_eq!(status().on_disk, 0, "without a budget a cap does nothing either");
    assert_eq!(store::reclaimable(), 0);
    drop((held, tiles));
}
