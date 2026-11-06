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
fn without_a_budget_nothing_goes() {
    let _turn = TURN.lock();
    let tiles: Vec<Arc<Tile>> = (0..20).map(|n| Arc::new(tile(n))).collect();
    assert_eq!(status().on_disk, 0);
    assert_eq!(store::reclaimable(), 0);
    drop(tiles);
}
