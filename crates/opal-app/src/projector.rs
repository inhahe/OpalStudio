//! A document's projection, computed on a thread of its own.

use eframe::egui;
use opal_core::composite::{Projection, Quality};
use opal_core::document::Layer;
use opal_core::geom::IRect;
use opal_core::tile::{Tile, TileCoord};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How long the canvas waits for what it asked for before drawing the last result instead.
/// Tests wait for it, so that what they see does not depend on timing.
pub const WAIT: Duration = if cfg!(test) { Duration::from_secs(120) } else { Duration::from_millis(20) };

/// A view of the document to bring up to date and show.
#[derive(Clone)]
struct Request {
    layers: Vec<Arc<Layer>>,
    level: u32,
    rect: IRect,
    quality: Quality,
    budget: Option<u64>,
    /// Counts requests: a result answers the request with its serial.
    serial: u64,
}

impl Request {
    fn same_as(&self, other: &Request) -> bool {
        self.level == other.level
            && self.rect == other.rect
            && self.quality == other.quality
            && self.budget == other.budget
            && self.layers.len() == other.layers.len()
            && self.layers.iter().zip(&other.layers).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

/// An answered request: the tiles with content at `level` within the area asked for.
pub struct Shown {
    pub level: u32,
    /// Whether what changed was drawn as a preview (see [`Quality::Preview`]).
    #[cfg(test)]
    preview: bool,
    pub tiles: HashMap<TileCoord, Arc<Tile>>,
    serial: u64,
}

#[derive(Default)]
struct State {
    /// The request not yet taken up (a newer one replaces it).
    job: Option<(Request, egui::Context)>,
    shown: Option<Arc<Shown>>,
    /// Why the thread stopped, if an update panicked.
    failed: Option<String>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when a job is given, or the thread is to stop.
    job: Condvar,
    /// Signalled when a result is published.
    done: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Computes a document's projection (see [`Projection`]) on a thread of its own, so that
/// the interface never waits long for it. The canvas asks for the view it shows and draws
/// the latest result, which until the new one is done may answer an earlier request: the
/// previous zoom, or the document before a change.
pub struct Projector {
    shared: Arc<Shared>,
    asked: Option<Request>,
}

impl Projector {
    pub fn new() -> Projector {
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        std::thread::Builder::new().name("projection".into()).spawn(move || run(&s)).expect("starting a thread");
        Projector { shared, asked: None }
    }

    /// Asks for the tiles of pyramid level `level` within `rect` (canvas pixels) to be brought
    /// up to date with `layers` (see [`Projection::update`]), the cache held to `budget`
    /// bytes (see [`Projection::set_budget`]). `ctx` is asked to repaint when it is done.
    /// Asking again for the same costs nothing.
    pub fn ask(
        &mut self,
        layers: &[Arc<Layer>],
        level: u32,
        rect: &IRect,
        quality: Quality,
        budget: Option<u64>,
        ctx: &egui::Context,
    ) {
        let serial = self.asked.as_ref().map_or(1, |a| a.serial + 1);
        let request = Request { layers: layers.to_vec(), level, rect: *rect, quality, budget, serial };
        if self.asked.as_ref().is_some_and(|a| a.same_as(&request)) {
            return;
        }
        self.asked = Some(request.clone());
        self.shared.lock().job = Some((request, ctx.clone()));
        self.shared.job.notify_one();
    }

    /// The latest result, after waiting up to `wait` for the one last asked for; `None` if
    /// none is done yet. Panics if an update panicked on the thread.
    pub fn shown(&self, wait: Duration) -> Option<Arc<Shown>> {
        let asked = self.asked.as_ref().map_or(0, |a| a.serial);
        let s = self.shared.lock();
        let pending = |s: &mut State| s.failed.is_none() && s.shown.as_ref().is_none_or(|r| r.serial < asked);
        let s = match self.shared.done.wait_timeout_while(s, wait, pending) {
            Ok((s, _)) => s,
            Err(e) => e.into_inner().0,
        };
        if let Some(why) = &s.failed {
            panic!("computing the projection failed: {why}");
        }
        s.shown.clone()
    }

    /// Whether the latest result drew what changed as a preview.
    #[cfg(test)]
    pub fn shows_previews(&self) -> bool {
        self.shared.lock().shown.as_ref().is_some_and(|s| s.preview)
    }
}

impl Default for Projector {
    fn default() -> Projector {
        Projector::new()
    }
}

impl Drop for Projector {
    /// Lets the thread go: it ends once any update in progress is done.
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.job.notify_one();
    }
}

/// The thread: takes up the latest request, brings the projection up to date with it, and
/// publishes the result, until told to stop.
fn run(shared: &Shared) {
    let mut projection = Projection::new();
    loop {
        let (request, ctx) = {
            let mut s = shared.lock();
            loop {
                if s.stop {
                    return;
                }
                if let Some(job) = s.job.take() {
                    break job;
                }
                s = shared.job.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        let update = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            projection.set_budget(request.budget);
            let tiles = projection.update(&request.layers, request.level, &request.rect, request.quality);
            Shown {
                level: request.level,
                #[cfg(test)]
                preview: projection.shows_previews(),
                tiles: tiles.into_iter().collect(),
                serial: request.serial,
            }
        }));
        let mut s = shared.lock();
        match update {
            Ok(shown) => s.shown = Some(Arc::new(shown)),
            Err(e) => {
                let why = e
                    .downcast_ref::<&str>()
                    .map(|m| m.to_string())
                    .or_else(|| e.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic".into());
                s.failed = Some(why);
            }
        }
        let failed = s.failed.is_some();
        drop(s);
        shared.done.notify_all();
        ctx.request_repaint();
        if failed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::document::{Document, IdGen, Position};
    use opal_core::raster::RasterImage;
    use opal_core::tile::pixel_from_f32;

    fn one_layer(rgba: [f32; 4]) -> Vec<Arc<Layer>> {
        let mut ids = IdGen::default();
        let mut d = Document::new(512, 512);
        let mut img = RasterImage::new();
        img.set_pixel(5, 5, pixel_from_f32(rgba));
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img)).unwrap();
        d.layers
    }

    #[test]
    fn the_latest_request_is_answered_and_the_same_one_costs_nothing() {
        let ctx = egui::Context::default();
        let mut p = Projector::new();
        assert!(p.shown(Duration::ZERO).is_none(), "nothing asked, nothing shown");
        let red = one_layer([1.0, 0.0, 0.0, 1.0]);
        let rect = IRect::from_size(512, 512);
        p.ask(&red, 0, &rect, Quality::Exact, None, &ctx);
        let first = p.shown(WAIT).expect("answered");
        assert_eq!(first.tiles[&TileCoord::new(0, 0)].get(5, 5), pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        // The same again: no new result.
        p.ask(&red, 0, &rect, Quality::Exact, None, &ctx);
        assert!(Arc::ptr_eq(&first, &p.shown(WAIT).unwrap()));
        // Two in a row: the latest is what is shown.
        p.ask(&one_layer([0.0, 1.0, 0.0, 1.0]), 0, &rect, Quality::Exact, None, &ctx);
        p.ask(&one_layer([0.0, 0.0, 1.0, 1.0]), 0, &rect, Quality::Exact, None, &ctx);
        let last = p.shown(WAIT).unwrap();
        assert_eq!(last.tiles[&TileCoord::new(0, 0)].get(5, 5), pixel_from_f32([0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn without_waiting_the_last_result_is_shown_until_the_new_one_is_done() {
        let ctx = egui::Context::default();
        let mut p = Projector::new();
        let rect = IRect::from_size(512, 512);
        p.ask(&one_layer([1.0, 0.0, 0.0, 1.0]), 0, &rect, Quality::Exact, None, &ctx);
        let first = p.shown(WAIT).unwrap();
        p.ask(&one_layer([0.0, 1.0, 0.0, 1.0]), 0, &rect, Quality::Exact, None, &ctx);
        let now = p.shown(Duration::ZERO).unwrap();
        assert!(Arc::ptr_eq(&now, &first) || now.serial == 2, "something other than a result");
        assert_eq!(p.shown(WAIT).unwrap().serial, 2);
    }

    #[test]
    fn asking_does_not_wait_for_the_work() {
        // Two 4096 × 4096 layers at full resolution: 256 tiles to composite.
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 4096);
        for (i, a) in [1.0, 0.5].into_iter().enumerate() {
            let mut img = RasterImage::new();
            let tile = Arc::new(opal_core::tile::Tile::filled(pixel_from_f32([a, 0.0, 0.0, a])));
            for c in TileCoord::covering(&IRect::from_size(4096, 4096), 0) {
                img.set_tile(c, Some(tile.clone()));
            }
            d.insert(Position { parent: None, index: i }, Layer::new_raster(ids.layer(), ids.object(), "l", img))
                .unwrap();
        }
        let mut p = Projector::new();
        p.ask(&d.layers, 0, &IRect::from_size(4096, 4096), Quality::Exact, None, &egui::Context::default());
        assert!(p.shown(Duration::ZERO).is_none(), "the work was done before asking returned");
        assert_eq!(p.shown(WAIT).expect("done in the end").tiles.len(), 256);
    }
}
