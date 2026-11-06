//! Once tiles can no longer be written to disk ([`opal_core::store`]), those with a copy
//! there still go. A test binary of its own: stopping is for the rest of the process.

use opal_core::store::{self, reclaimable, set_budget, status};
use opal_core::tile::{TILE_PIXELS, Tile, pixel_from_f32};
use std::sync::Arc;

const TILE: u64 = (TILE_PIXELS * 8) as u64;

fn tile(n: usize) -> Tile {
    Tile::filled(pixel_from_f32([n as f32 / 1000.0, 0.5, 0.25, 1.0]))
}

#[test]
fn after_writing_stops_tiles_with_a_copy_on_disk_still_go() {
    set_budget(Some(4 * TILE));
    let tiles: Vec<Arc<Tile>> = (0..20).map(|n| Arc::new(tile(n))).collect();
    store::settle();
    assert!(status().on_disk >= 16 * TILE);
    store::stop_writing_for_tests();
    // Read back, a tile keeps its copy on disk, so it could go again.
    let reading = tiles[0].reader();
    assert!(reclaimable() >= TILE, "a tile with a copy could still go");
    drop(reading);
    // Read back one after another, each goes again as the next comes.
    for (n, t) in tiles.iter().enumerate() {
        assert_eq!(t.get(1, 1), tile(n).get(1, 1));
        assert!(status().resident <= 6 * TILE, "{} tiles in memory", status().resident / TILE);
    }
    // New tiles cannot go; memory takes them past the budget.
    let more: Vec<Arc<Tile>> = (0..8).map(|n| Arc::new(tile(n + 100))).collect();
    assert!(status().resident >= 8 * TILE);
    drop((tiles, more));
    set_budget(None);
}
