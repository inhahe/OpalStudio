//! Comparing images (design.md, *Comparing*): their panes tiled or put back in tabs, a
//! reference image pinned beside them, and views matched to the active one's.

use crate::app::Pane;
use crate::doc::{DocId, View};
use eframe::egui::Vec2;
use egui_tiles::{Container, Tile, TileId, Tiles, Tree};

/// `to`'s view made to show what `from` shows: the same zoom, and in the middle of its
/// viewport the point of its image that `from` shows in the middle of its own, both counted
/// from their canvases' corners (`from_corner`, `to_corner`: where each canvas's top-left
/// is, in canvas pixels).
pub fn matched(from: &View, from_corner: Vec2, to: &View, to_corner: Vec2) -> View {
    let middle = from.to_canvas(from.viewport * 0.5) - from_corner;
    let mut v = *to;
    v.zoom = from.zoom;
    v.offset = to.viewport * 0.5 - (middle + to_corner) * v.zoom;
    v.placed = true;
    v.snap();
    v
}

/// Whether `pane` is one the images' region holds: an image's, or the start page.
fn is_image(pane: &Pane) -> bool {
    matches!(pane, Pane::Document(_) | Pane::Welcome)
}

/// The tiles from `id` up to the root.
fn ancestry(tiles: &Tiles<Pane>, id: TileId) -> Vec<TileId> {
    let mut out = vec![id];
    while let Some(p) = tiles.parent_of(*out.last().expect("one at least")) {
        out.push(p);
    }
    out
}

/// Whether every pane within tile `id` is an image's.
fn only_images(tiles: &Tiles<Pane>, id: TileId) -> bool {
    match tiles.get(id) {
        Some(Tile::Pane(p)) => is_image(p),
        Some(Tile::Container(c)) => c.children().all(|&child| only_images(tiles, child)),
        None => true,
    }
}

/// The images' panes, in `order` (the start page last, if it is there).
fn image_panes(tiles: &Tiles<Pane>, order: &[DocId]) -> Vec<TileId> {
    let mut panes: Vec<TileId> = order.iter().filter_map(|&d| tiles.find_pane(&Pane::Document(d))).collect();
    panes.extend(tiles.find_pane(&Pane::Welcome));
    panes
}

/// The container holding all of `panes` and nothing but images: the smallest one holding
/// them all, if it holds nothing else. None if the images are spread among the panels.
fn region_of(tiles: &Tiles<Pane>, panes: &[TileId]) -> Option<TileId> {
    let first = ancestry(tiles, *panes.first()?);
    // The first container above the first pane that holds every other.
    let common = first.into_iter().skip(1).find(|&c| panes.iter().all(|&p| ancestry(tiles, p).contains(&c)))?;
    only_images(tiles, common).then_some(common)
}

/// Puts `new` where `old` is in the tree, and removes `old` with the containers within it
/// (their panes moved out already).
fn put_in_place(tree: &mut Tree<Pane>, old: TileId, new: TileId) {
    replace(tree, old, new);
    tree.remove_recursively(old);
}

/// Puts `new` where `old` is in the tree (a linear container giving it the share it gave
/// `old`), `old` kept, to be put elsewhere.
fn replace(tree: &mut Tree<Pane>, old: TileId, new: TileId) {
    match tree.tiles.parent_of(old) {
        Some(parent) => {
            if let Some(Tile::Container(c)) = tree.tiles.get_mut(parent) {
                if let Container::Linear(l) = c {
                    l.shares.replace_with(old, new);
                }
                let found = c.replace_child(old, new);
                debug_assert!(found.is_some(), "{old:?} not a child of its parent");
            }
        }
        None => tree.root = Some(new),
    }
}

/// Takes `pane` out of the container it is in.
fn detach(tiles: &mut Tiles<Pane>, pane: TileId) {
    if let Some(parent) = tiles.parent_of(pane)
        && let Some(Tile::Container(c)) = tiles.get_mut(parent)
    {
        c.remove_child(pane);
    }
}

/// How many columns a grid of images of the shapes `aspects` (width over height) is best given
/// in `room`: the one in which they show most, each fitted in a cell.
pub fn columns(room: Vec2, aspects: &[f32]) -> usize {
    let n = aspects.len().max(1);
    let shown = |cols: usize| {
        let cell = Vec2::new(room.x / cols as f32, room.y / n.div_ceil(cols) as f32);
        let area = |a: f32| if cell.x / cell.y > a { cell.y * cell.y * a } else { cell.x * cell.x / a };
        aspects.iter().map(|&a| area(a.max(1e-6))).sum::<f32>()
    };
    (2..=n).fold((1, shown(1)), |best, c| if shown(c) > best.1 { (c, shown(c)) } else { best }).0
}

/// Arranges the images' panes where they are: each in a pane of its own, in a grid (`tiled`)
/// of as many columns as shows them best, or all in one set of tabs, `active`'s in front.
/// `images` are the images open, in order, each with its shape (width over height). Returns
/// whether it could: not if the images are spread among the panels, which only the starting
/// layout puts back.
pub fn arrange(tree: &mut Tree<Pane>, images: &[(DocId, f32)], active: Option<DocId>, tiled: bool) -> bool {
    let order: Vec<DocId> = images.iter().map(|i| i.0).collect();
    let panes = image_panes(&tree.tiles, &order);
    let Some(region) = region_of(&tree.tiles, &panes) else { return false };
    let room = tree.tiles.rect(region);
    let aspects: Vec<f32> = panes
        .iter()
        .map(|&p| match tree.tiles.get_pane(&p) {
            Some(Pane::Document(d)) => images.iter().find(|i| i.0 == *d).map_or(1.0, |i| i.1),
            _ => 1.0,
        })
        .collect();
    for &p in &panes {
        detach(&mut tree.tiles, p);
    }
    let new = if tiled && panes.len() > 1 {
        let tabs: Vec<TileId> = panes.iter().map(|&p| tree.tiles.insert_tab_tile(vec![p])).collect();
        let grid = tree.tiles.insert_grid_tile(tabs);
        // (Not yet drawn, it is left to lay itself out by its own shape.)
        if let Some(room) = room
            && let Some(Tile::Container(Container::Grid(g))) = tree.tiles.get_mut(grid)
        {
            g.layout = egui_tiles::GridLayout::Columns(columns(room.size(), &aspects));
        }
        grid
    } else {
        let tabs = tree.tiles.insert_tab_tile(panes.clone());
        let front = active.and_then(|a| tree.tiles.find_pane(&Pane::Document(a)));
        if let (Some(front), Some(Tile::Container(Container::Tabs(t)))) = (front, tree.tiles.get_mut(tabs)) {
            t.set_active(front);
        }
        tabs
    };
    put_in_place(tree, region, new);
    true
}

/// Pins image `doc`'s pane beside the other images: in a set of tabs of its own, at the
/// right of their region, a third of its width. Returns whether it could (as `arrange`).
pub fn pin_beside(tree: &mut Tree<Pane>, order: &[DocId], doc: DocId) -> bool {
    let Some(pane) = tree.tiles.find_pane(&Pane::Document(doc)) else { return false };
    let others: Vec<TileId> = image_panes(&tree.tiles, order).into_iter().filter(|&p| p != pane).collect();
    // (Nothing else to stand beside, or the others spread among the panels: left where it is.)
    let Some(region) = region_of(&tree.tiles, &others) else { return false };
    detach(&mut tree.tiles, pane);
    // Its own tabs, beside a copy of the region's place.
    let pinned = tree.tiles.insert_tab_tile(vec![pane]);
    let placeholder = tree.tiles.insert_tab_tile(Vec::new());
    replace(tree, region, placeholder);
    let mut row = egui_tiles::Linear::new(egui_tiles::LinearDir::Horizontal, vec![region, pinned]);
    row.shares.set_share(region, 2.0);
    row.shares.set_share(pinned, 1.0);
    let row = tree.tiles.insert_container(row);
    put_in_place(tree, placeholder, row);
    true
}

impl crate::state::AppState {
    /// Asks for an image file and opens it as a reference (see
    /// [`open_reference`](Self::open_reference)).
    pub(crate) fn open_reference_dialog(&mut self, ctx: &eframe::egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Open Reference")
            .add_filter("Images and Opal Studio files", &["png", "jpg", "jpeg", "opal"])
            .pick_file()
        else {
            return;
        };
        self.open_reference(path, ctx);
    }

    /// Opens the file at `path` (read in the background, as Open reads) as an image pinned
    /// beside the others, in a pane of its own; the image active now stays so.
    pub(crate) fn open_reference(&mut self, path: std::path::PathBuf, ctx: &eframe::egui::Context) {
        let editing = self.active_doc;
        let then: crate::opening::Then = Box::new(move |st: &mut crate::state::AppState, read| {
            for (path, file) in read {
                let id = st.open_read(&path, file)?.0;
                st.pin.push((id, editing));
            }
            Ok(serde_json::json!({}))
        });
        self.read_then(vec![path], None, None, then, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The starting layout's shape: the images' tabs, and a panel beside them.
    fn tree(docs: &[u64]) -> Tree<Pane> {
        let mut tiles = Tiles::default();
        let panes: Vec<TileId> = docs.iter().map(|&d| tiles.insert_pane(Pane::Document(DocId(d)))).collect();
        let images = tiles.insert_tab_tile(panes);
        let layers = tiles.insert_pane(Pane::Layers);
        let side = tiles.insert_tab_tile(vec![layers]);
        let mut root = egui_tiles::Linear::new(egui_tiles::LinearDir::Horizontal, vec![images, side]);
        root.shares.set_share(images, 0.8);
        root.shares.set_share(side, 0.2);
        let root = tiles.insert_container(root);
        Tree::new(eframe::egui::Id::unique("test"), root, tiles)
    }

    /// `order`'s images, each square.
    fn square(order: &[DocId]) -> Vec<(DocId, f32)> {
        order.iter().map(|&d| (d, 1.0)).collect()
    }

    fn parent_kind(tree: &Tree<Pane>, doc: u64) -> Option<&'static str> {
        let pane = tree.tiles.find_pane(&Pane::Document(DocId(doc)))?;
        let tabs = tree.tiles.parent_of(pane)?;
        let above = tree.tiles.parent_of(tabs)?;
        Some(match tree.tiles.get(above)? {
            Tile::Container(Container::Grid(_)) => "grid",
            Tile::Container(Container::Linear(_)) => "linear",
            Tile::Container(Container::Tabs(_)) => "tabs",
            Tile::Pane(_) => "pane",
        })
    }

    #[test]
    fn images_are_tiled_and_put_back_in_tabs_where_they_were() {
        let order = [DocId(1), DocId(2), DocId(3)];
        let mut t = tree(&[1, 2, 3]);
        assert!(arrange(&mut t, &square(&order), Some(DocId(2)), true));
        // Each in tabs of its own, in a grid, which stands where the tabs were.
        for d in 1..=3 {
            assert_eq!(parent_kind(&t, d), Some("grid"), "image {d}");
        }
        assert!(t.tiles.find_pane(&Pane::Layers).is_some(), "a panel lost");
        assert!(arrange(&mut t, &square(&order), Some(DocId(2)), false));
        let pane = t.tiles.find_pane(&Pane::Document(DocId(1))).unwrap();
        let tabs = t.tiles.parent_of(pane).unwrap();
        let Some(Tile::Container(Container::Tabs(tabs))) = t.tiles.get(tabs) else { panic!("not in tabs") };
        assert_eq!(tabs.children.len(), 3);
        assert_eq!(tabs.active, t.tiles.find_pane(&Pane::Document(DocId(2))), "the active image not in front");
        assert_eq!(parent_kind(&t, 1), Some("linear"), "not where they were");
    }

    #[test]
    fn a_reference_is_pinned_beside_the_images_and_images_spread_among_panels_are_left() {
        let order = [DocId(1), DocId(2), DocId(3)];
        let mut t = tree(&[1, 2, 3]);
        assert!(pin_beside(&mut t, &order, DocId(3)));
        let pane = t.tiles.find_pane(&Pane::Document(DocId(3))).unwrap();
        let tabs = t.tiles.parent_of(pane).unwrap();
        let Some(Tile::Container(Container::Tabs(own))) = t.tiles.get(tabs) else { panic!() };
        assert_eq!(own.children.len(), 1, "not in tabs of its own");
        let row = t.tiles.parent_of(tabs).unwrap();
        let Some(Tile::Container(Container::Linear(row))) = t.tiles.get(row) else { panic!("not in a row") };
        assert_eq!(row.children.len(), 2);
        // The others still together, and tiling takes the reference in with them.
        assert!(arrange(&mut t, &square(&order), None, true));
        assert_eq!(parent_kind(&t, 3), Some("grid"));
        // An image dragged among the panels: nothing is arranged.
        let mut spread = tree(&[1, 2]);
        let p = spread.tiles.find_pane(&Pane::Document(DocId(2))).unwrap();
        detach(&mut spread.tiles, p);
        let side = spread.tiles.parent_of(spread.tiles.find_pane(&Pane::Layers).unwrap()).unwrap();
        if let Some(Tile::Container(c)) = spread.tiles.get_mut(side) {
            c.add_child(p);
        }
        assert!(!arrange(&mut spread, &square(&order[..2]), None, true));
    }

    #[test]
    fn a_grid_has_as_many_columns_as_shows_its_images_largest() {
        let wide = Vec2::new(1080.0, 680.0);
        // Two photographs side by side; three or four in two rows.
        assert_eq!(columns(wide, &[1.5; 2]), 2);
        assert_eq!(columns(wide, &[1.5; 3]), 2);
        assert_eq!(columns(wide, &[1.5; 4]), 2);
        // Tall ones, all in a row; long strips, one above another.
        assert_eq!(columns(wide, &[0.5; 3]), 3);
        assert_eq!(columns(wide, &[6.0; 3]), 1);
        assert_eq!(columns(wide, &[]), 1);
    }

    #[test]
    fn a_view_matched_shows_the_same_place_at_the_same_zoom() {
        let mut from = View { zoom: 2.0, viewport: Vec2::new(800.0, 600.0), placed: true, ..View::default() };
        from.offset = Vec2::new(-100.0, -40.0);
        let to = View { zoom: 0.5, viewport: Vec2::new(400.0, 300.0), placed: true, ..View::default() };
        // The other canvas's corner elsewhere: the same place of its image is matched.
        let m = matched(&from, Vec2::ZERO, &to, Vec2::new(10.0, 20.0));
        assert_eq!(m.zoom, 2.0);
        let middle_from = from.to_canvas(from.viewport * 0.5);
        let middle_to = m.to_canvas(m.viewport * 0.5) - Vec2::new(10.0, 20.0);
        assert!((middle_from - middle_to).length() < 1e-3, "{middle_from:?} {middle_to:?}");
    }
}
