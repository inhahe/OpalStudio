//! What is selected (design.md, *Selections*): Select All, Deselect and Invert, and the
//! commands that act on the selection (Clear, Cut, Crop to Selection). Copy copies within it
//! (`paste.rs`); the canvas shows its outline (`canvas.rs`).

use crate::doc::DocId;
use crate::state::{AppState, MessageKind};
use opal_core::document::{Document, Layer, LayerContent, LayerId, ObjectKind};
use opal_core::geom::IRect;
use opal_core::mask::{Combine, Extent, Mask, Shape};
use opal_core::selection::{Selection, Step};
use std::sync::Arc;

/// A selection's outline: closed lines, in document coordinates (see `Mask::outline`).
pub type Outline = Arc<Vec<Vec<[f64; 2]>>>;

/// The Grow or Shrink, or Feather, dialog, while it is open: the image it acts on, the step it
/// adds, the selection it adds it to, and the step that image's pending edit shows. What a
/// step set makes is worked out off the interface thread (`AppState::preview_modifying`), the
/// latest only.
#[derive(Debug)]
pub struct Modifying {
    doc: DocId,
    pub step: Step,
    pub before: Arc<Selection>,
    shown: Option<Step>,
    /// Why what the step makes could not be worked out (not memory enough, say), if it could
    /// not: the dialog says so, and keeps nothing until another step is set.
    failed: Option<String>,
    /// Whether OK was pressed while what the step makes was being worked out: it is kept
    /// once shown, the dialog waiting until then.
    keeping: bool,
    /// The number of the job last asked for (jobs are numbered from 1), and of the last that
    /// has ended (as `ends` has told).
    asked: u64,
    ended: u64,
    /// The job wanted: one asked for after it, or the dialog's end, makes it another, and a
    /// job not wanted stops, before it begins or between the pieces of its work.
    generation: Arc<std::sync::atomic::AtomicU64>,
    /// Where each job tells its end, however it ends (done, stopped, or failed), and what it
    /// made, if anything; and where the dialog reads them.
    ends: (std::sync::mpsc::Sender<Ended>, std::sync::mpsc::Receiver<Ended>),
}

impl Modifying {
    /// The image the dialog acts on.
    pub fn doc(&self) -> DocId {
        self.doc
    }

    /// Why what the step set makes could not be worked out, if it could not.
    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// Whether OK was pressed, and the dialog waits for what the step makes.
    pub fn keeping(&self) -> bool {
        self.keeping
    }
}

/// A step a modifier dialog was set to, and what it makes, or why that could not be made.
type Preview = (Step, Result<Arc<Selection>, String>);

/// A modifier dialog's job, ended: its number, and what it made, if anything.
type Ended = (u64, Option<Preview>);

/// The thread the modifier dialogs' previews are worked out on.
static PREVIEWS: crate::background::LatestJob = crate::background::LatestJob::new("selection previews");

/// The history state adding `step`, a modifier, makes.
pub fn modifier_label(step: &Step) -> &'static str {
    match step {
        Step::Grow { by, .. } if *by < 0.0 => "Shrink selection",
        Step::Grow { .. } => "Grow selection",
        _ => "Feather selection",
    }
}

impl crate::doc::DocState {
    /// What is selected now, if anything.
    pub fn pixel_selection(&self) -> Option<&Arc<Selection>> {
        self.doc().selection.as_ref()
    }

    /// Makes `selection` what is selected (nothing, if it is `None` or selects nothing), as a
    /// history state labeled `label`; none if it is what is selected already.
    pub fn set_pixel_selection(&mut self, selection: Option<Selection>, label: &str) {
        let selection = selection.filter(|s| !s.is_empty()).map(Arc::new);
        if selection.as_deref() == self.pixel_selection().map(|s| &**s) {
            return;
        }
        let _ = self.apply(label, |doc, _| {
            doc.selection = selection;
            Ok::<(), ()>(())
        });
    }

    /// The outline of what is selected (the marching ants' path, in document coordinates),
    /// round the pixels selected at least `threshold` (a level): worked out once for each
    /// selection's mask and threshold, and kept until another is asked for, or none (nothing
    /// is selected).
    pub fn selection_outline(&mut self, threshold: u16) -> Option<Outline> {
        let Some(s) = self.pixel_selection() else {
            self.outline = None;
            return None;
        };
        let mask = s.mask().clone();
        if let Some((of, at, lines)) = &self.outline
            && std::ptr::eq(of.as_ptr(), Arc::as_ptr(&mask))
            && *at == threshold
        {
            return Some(lines.clone());
        }
        let lines = Arc::new(mask.outline(threshold));
        self.outline = Some((Arc::downgrade(&mask), threshold, lines.clone()));
        Some(lines)
    }
}

impl AppState {
    /// Leaves the images not drawn in the pass before `pass` (shown in no pane): a Select tool
    /// drag on one is given up, as on Esc (its button may have been let go over another, and a
    /// call acting on it would commit its edit), and the textures of its selection overlay go.
    pub(crate) fn leave_undrawn(&mut self, pass: u64) {
        for d in self.docs.iter_mut().filter(|d| d.drawn + 1 < pass) {
            d.select_drag = None;
            if let Some(s) = d.shape_drag.take() {
                d.discard_pending(crate::select_tool::ShapeDrag::label(s.drag.part));
            }
            d.overlay = Default::default();
        }
    }

    /// Selects the canvas, as Photoshop's does: what lies past it (layers have no edge) is not
    /// selected.
    pub(crate) fn select_all(&mut self) {
        let d = self.active_mut().expect("available");
        let shape = Shape::rectangle(&d.doc().canvas.rect().to_f(), false);
        let s = Selection::new(Step::Shape { shape, combine: Combine::Replace });
        d.set_pixel_selection(Some(s), "Select all");
    }

    pub(crate) fn deselect(&mut self) {
        let d = self.active_mut().expect("available");
        d.set_pixel_selection(None, "Deselect");
    }

    /// Selects what is not selected, and nothing of what is (with everything selected,
    /// nothing is left selected).
    pub(crate) fn invert_selection(&mut self) {
        let d = self.active_mut().expect("available");
        let s = d.pixel_selection().expect("available").then(Step::Invert);
        d.set_pixel_selection(Some(s), "Invert selection");
    }

    /// Opens the Grow or Shrink dialog (`grow`), or the Feather dialog, on what the active
    /// image has selected, what it makes worked out off the interface thread at once.
    pub(crate) fn modify_selection(&mut self, grow: bool, ctx: &eframe::egui::Context) {
        use opal_core::modify::{Falloff, Reach, Side};
        let d = self.active().expect("available");
        let (doc, before) = (d.id, d.pixel_selection().expect("available").clone());
        let step = if grow {
            Step::Grow { by: 5.0, corners: Reach::Round }
        } else {
            Step::Feather { width: 10.0, side: Side::Centered, falloff: Falloff::Gaussian }
        };
        self.modifying = Some(Modifying {
            doc,
            step,
            before,
            shown: None,
            failed: None,
            keeping: false,
            asked: 0,
            ended: 0,
            generation: Default::default(),
            ends: std::sync::mpsc::channel(),
        });
        self.preview_modifying(ctx);
    }

    /// Shows `made`, what the open modifier dialog makes with `step`, as its image's pending
    /// edit, named for what the step does (the edit named for the one shown before given up).
    fn show_made(&mut self, step: Step, made: Arc<Selection>) {
        let Some(m) = self.modifying.as_mut() else { return };
        m.failed = None;
        let before = m.shown.replace(step.clone());
        let doc = m.doc;
        let Some(d) = self.doc_mut(doc) else { return };
        if let Some(before) = before.filter(|b| modifier_label(b) != modifier_label(&step)) {
            d.discard_pending(modifier_label(&before));
        }
        d.edit_pending(modifier_label(&step), |doc| doc.selection = (!made.is_empty()).then_some(made));
    }

    /// Has what the open modifier dialog makes with its step worked out off the interface
    /// thread, to be shown once done ([`poll_modifying`](Self::poll_modifying)); a step asked
    /// for after it stops it, between the pieces of its work, or before it begins. It stops,
    /// saying why, if there is not memory enough for it, measured as it goes.
    pub(crate) fn preview_modifying(&mut self, ctx: &eframe::egui::Context) {
        use std::sync::atomic::Ordering;
        let Some(m) = self.modifying.as_mut() else { return };
        m.asked += 1;
        let ours = m.asked;
        m.generation.store(ours, Ordering::SeqCst);
        m.failed = None;
        let (step, before, generation) = (m.step.clone(), m.before.clone(), m.generation.clone());
        let (ends, asked) = (m.ends.0.clone(), m.step.clone());
        let measure = crate::memory::Measure::here();
        PREVIEWS.run(
            move || {
                use opal_core::modify::{Bounds, Halt};
                let stop = || generation.load(Ordering::SeqCst) != ours;
                // Asked for after another, it may not be wanted by the time it begins.
                if stop() {
                    return None;
                }
                // What is free, measured as the work begins.
                let admission = crate::memory::Admission::new(measure.now());
                let admit = |n| admission.admit(n);
                match before.then_within(step, Bounds { stop: &stop, admit: &admit }) {
                    Ok(made) => Some(Ok(Arc::new(made))),
                    // Not wanted any more: nothing to tell, whatever held it back.
                    Err(_) if stop() => None,
                    Err(Halt::Stopped) => None,
                    Err(Halt::NoMemory) => Some(Err(admission.refused())),
                }
            },
            move |result| {
                let made = match result {
                    Ok(made) => made,
                    Err(panic) => Some(Err(format!("This went wrong: {}.", crate::background::panic_message(&*panic)))),
                };
                // Told whatever came of it, so that no one waits on. (The dialog may have closed.)
                let _ = ends.send((ours, made.map(|made| (asked, made))));
            },
            ctx,
        );
    }

    /// What a frame of the open modifier dialog came to: its fields set `step`, and OK
    /// (`Some(true)`) or Cancel (`Some(false)`) was pressed, if either was. While what the step
    /// makes is worked out, the image shows what it made last (its job asks for a frame once
    /// done); after OK, the dialog closes once that is shown.
    pub(crate) fn answer_modifying(&mut self, step: Step, done: Option<bool>, ctx: &eframe::egui::Context) {
        // Cancel first: what a job made meanwhile must not close the dialog keeping it.
        if done == Some(false) {
            return self.end_modifying(false);
        }
        if let Some(m) = self.modifying.as_mut()
            && m.step != step
        {
            m.step = step;
            self.preview_modifying(ctx);
        }
        self.poll_modifying();
        if done == Some(true) {
            self.end_modifying(true);
        }
    }

    /// Shows what a job made for the open modifier dialog, if it is done and for the step it
    /// has now, or why it could not be made (the image then shows the selection as it was);
    /// once that is shown after OK, ends the dialog keeping it. Whether the last job asked
    /// for is still under way.
    pub(crate) fn poll_modifying(&mut self) -> bool {
        let Some(m) = self.modifying.as_mut() else { return false };
        // The jobs end one after another: the last to have made something is the latest.
        let mut made = None;
        while let Ok((n, preview)) = m.ends.1.try_recv() {
            m.ended = m.ended.max(n);
            made = preview.or(made);
        }
        let under_way = m.ended < m.asked;
        match made {
            Some((step, Ok(made))) if step == m.step => self.show_made(step, made),
            Some((step, Err(why))) if step == m.step => self.take_back_modifying(why),
            _ => {}
        }
        let Some(m) = self.modifying.as_ref() else { return false };
        if m.keeping && m.shown.as_ref() == Some(&m.step) {
            self.close_modifying(true);
        } else if m.keeping && !under_way {
            // Nothing to keep: the dialog stays, saying why.
            let why = m.failed.clone().unwrap_or_else(|| "This could not be worked out.".into());
            self.take_back_modifying(why);
        }
        under_way
    }

    /// Shows the selection as it was before the open modifier dialog, with `why` what its step
    /// makes could not be worked out; OK pressed waits no longer.
    fn take_back_modifying(&mut self, why: String) {
        let Some(m) = self.modifying.as_mut() else { return };
        m.failed = Some(why);
        m.keeping = false;
        let (shown, doc) = (m.shown.take(), m.doc);
        if let (Some(shown), Some(d)) = (shown, self.doc_mut(doc)) {
            d.discard_pending(modifier_label(&shown));
        }
    }

    /// Ends the open modifier dialog: if `keep` (OK), what it makes a history state of its
    /// own once shown, at once if it is, else once its job is done, the dialog waiting
    /// meanwhile ([`poll_modifying`](Self::poll_modifying)); else (Cancel) given up.
    pub(crate) fn end_modifying(&mut self, keep: bool) {
        let Some(m) = self.modifying.as_mut() else { return };
        if keep {
            m.keeping = true;
            self.poll_modifying();
        } else {
            self.close_modifying(false);
        }
    }

    /// Closes the open modifier dialog, keeping what it shows as a history state of its own if
    /// `keep`, else giving it up. The jobs it asked for stop.
    fn close_modifying(&mut self, keep: bool) {
        let Some(m) = self.modifying.take() else { return };
        // No job is wanted now.
        m.generation.store(0, std::sync::atomic::Ordering::SeqCst);
        let Some(d) = self.doc_mut(m.doc) else { return };
        if keep {
            d.commit_pending();
        } else if let Some(shown) = m.shown {
            d.discard_pending(modifier_label(&shown));
        }
    }

    /// Adds `step`, a modifier, to what is selected, as a history state, worked out off the
    /// interface thread (see `edits`); refused, saying why, if there is not memory enough for
    /// it, measured as it goes.
    pub(crate) fn add_modifier(&mut self, step: Step) {
        use opal_core::modify::{Bounds, Halt};
        let d = self.active().expect("available");
        let before = d.pixel_selection().expect("available").clone();
        let label = modifier_label(&step);
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            let admission = crate::memory::Admission::new(measure.now());
            let (stop, admit) = (|| progress.is_cancelled(), |n| admission.admit(n));
            let made = before.then_within(step, Bounds { stop: &stop, admit: &admit }).map_err(|halt| match halt {
                Halt::Stopped => crate::edits::CANCELLED.to_owned(),
                Halt::NoMemory => admission.refused(),
            })?;
            doc.selection = (!made.is_empty()).then(|| Arc::new(made));
            Ok(())
        };
        self.edit_later(d.id, label, crate::memory::Allowance::unlimited(), work, |_, _, ()| {});
    }

    /// Erases what is selected on the selected layers (as much of each pixel as it is
    /// selected), past the canvas too.
    pub(crate) fn clear_selected(&mut self) {
        self.clear_within("Clear", None);
    }

    /// Erases what is selected on the selected layers, as a history state labeled `label`
    /// (only within `area`, if given); whether anything was erased (if not, the user is told
    /// why).
    fn clear_within(&mut self, label: &str, area: Option<&IRect>) -> bool {
        let d = self.active_mut().expect("available");
        let mut mask = d.pixel_selection().expect("available").mask().clone();
        if let Some(r) = area {
            mask = Arc::new(mask.combined(&Mask::of(&Shape::rectangle(&r.to_f(), false)), Combine::Intersect));
        }
        let layers: Vec<LayerId> = cleared(d).iter().filter(|l| !l.is_group()).map(|l| l.id).collect();
        let mut changed = false;
        let result = d.apply(label, |doc, _| {
            changed = clear(doc, &layers, &mask);
            if changed { Ok(()) } else { Err(()) }
        });
        if result.is_err() {
            self.notify("Nothing selected on the selected layers had pixels to clear.");
        }
        changed
    }

    /// Copies what the selected layers show within the selection and the canvas, then erases
    /// that from them (what lies past the canvas, which Copy leaves out, stays). The copy
    /// reaches the clipboard off the interface thread; should it not, the erasing is taken
    /// back (see [`AppState::poll_copies`]). `answer`: the API call to answer once the copy is
    /// on the clipboard. Whether the copy was started (if not, it was refused, and said why).
    pub(crate) fn cut(&mut self, answer: Option<u64>, ctx: &eframe::egui::Context) -> bool {
        if !self.copy_within(false, crate::memory::Allowance::now(), answer, ctx) {
            return false;
        }
        let canvas = self.active().expect("available").doc().canvas.rect();
        if self.clear_within("Cut", Some(&canvas)) {
            let d = self.active().expect("available");
            let made = d.history.current();
            let from = d.history.node(made).parent.expect("a state made has one");
            self.copying.back_mut().expect("the copy just started").cut = Some((d.id, made, from));
        }
        true
    }

    /// Makes the canvas the rectangle round what is selected.
    pub(crate) fn crop_to_selection(&mut self) {
        let d = self.active().expect("available");
        let b = match d.pixel_selection().expect("available").mask().extent() {
            Extent::Within(b) => b,
            Extent::Nothing => {
                return self.message(MessageKind::Info, "Crop to Selection", "The selection selects no pixel.");
            }
            Extent::Unbounded => {
                return self.message(
                    MessageKind::Info,
                    "Crop to Selection",
                    "The selection reaches past every edge: there is no rectangle round it.",
                );
            }
        };
        self.crop_canvas_to(&b, "Crop to selection", "The canvas is already the selection's rectangle.");
    }
}

/// Erases what `mask` selects from the pixel objects of `layers` in `doc`; whether anything
/// changed.
fn clear(doc: &mut Document, layers: &[LayerId], mask: &Mask) -> bool {
    let mut changed = false;
    for &id in layers {
        let Some(layer) = doc.layer_mut(id) else { continue };
        let LayerContent::Objects(objects) = &mut layer.content else { continue };
        for o in objects.iter_mut() {
            let ObjectKind::Pixels(p) = &mut o.kind else { continue };
            let cleared = opal_core::mask::clear_selected(&p.image, p.offset, mask);
            if !same_tiles(&cleared, &p.image) {
                p.image = Arc::new(cleared);
                changed = true;
            }
        }
    }
    changed
}

/// Whether two bitmaps hold the very same tiles.
fn same_tiles(a: &opal_core::raster::RasterImage, b: &opal_core::raster::RasterImage) -> bool {
    a.tile_count() == b.tile_count() && a.tiles().all(|(c, t)| b.tile(c).is_some_and(|u| Arc::ptr_eq(t, u)))
}

/// What Clear and Cut act on: the selected layers, and inside the selected groups the layers
/// and groups shown (a hidden one is left as it is: nothing of it is seen), each after the
/// group it is in.
fn cleared(d: &crate::doc::DocState) -> Vec<&Layer> {
    fn walk<'a>(l: &'a Layer, out: &mut Vec<&'a Layer>) {
        out.push(l);
        for c in l.children().unwrap_or(&[]) {
            if c.visible {
                walk(c, out);
            }
        }
    }
    let doc = d.doc();
    let mut out = Vec::new();
    for id in d.selected_layers() {
        if let Some(l) = doc.layer(id) {
            walk(l, &mut out);
        }
    }
    out
}

/// Why the selected layers cannot be cleared within the selection, if they cannot: none is
/// selected, or one (or a group, selected or holding one) is hidden, or one it would clear
/// (or a group it is in) is locked, or holds a placed image (whose pixels are its own, drawn
/// through a transform: rasterize it first).
pub(crate) fn clear_refused(d: &crate::doc::DocState) -> Result<(), String> {
    if d.pixel_selection().is_none() {
        return Err("Select an area first (the Select tool, or Select All).".into());
    }
    let doc = d.doc();
    let selected: Vec<&Layer> = d.selected_layers().into_iter().filter_map(|l| doc.layer(l)).collect();
    if selected.is_empty() {
        return Err("Select a layer in the Layers panel first.".into());
    }
    // Each selected layer, after the groups it is in.
    let around: Vec<&Layer> =
        selected.iter().flat_map(|l| crate::commands::groups_above(doc, l.id).into_iter().chain([*l])).collect();
    if let Some(l) = around.iter().find(|l| !l.visible) {
        return Err(format!("\"{}\" is hidden: clearing it would change nothing you can see. Show it first.", l.name));
    }
    let layers = cleared(d);
    if let Some(l) = around.iter().chain(&layers).find(|l| l.locked) {
        return Err(format!("\"{}\" is locked. Unlock it first.", l.name));
    }
    if let Some(l) = layers.iter().find(|l| !l.is_group() && crate::layer_ops::has_image_object(l)) {
        return Err(format!(
            "\"{}\" holds a placed image, whose pixels are its own, drawn through its transform. Rasterize it first.",
            l.name
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::state::{AppState, Background, NewImage};
    use opal_core::document::{Document, IdGen, Layer, LayerContent, LayerId, ObjectKind, Position};
    use opal_core::geom::IRect;
    use opal_core::raster::RasterImage;

    #[test]
    fn an_edit_that_selects_nothing_keeps_its_steps_to_be_put_right() {
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let a = Shape::rectangle(&IRect::new(0, 0, 100, 100).to_f(), true);
        let b = Shape::rectangle(&IRect::new(50, 0, 150, 100).to_f(), true);
        let s = Selection::new(Step::Shape { shape: a, combine: Combine::Replace })
            .then(Step::Shape { shape: b, combine: Combine::Intersect });
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(s), "Select");
        // B moved off A: nothing is in both, but the steps stay, and so does the box.
        let before = d.pixel_selection().unwrap().before_last();
        let away = Shape::rectangle(&IRect::new(200, 0, 300, 100).to_f(), true);
        crate::select_tool::reshape(d, &before, away, Combine::Intersect, "Edit selection");
        let now = d.pixel_selection().expect("the selection was dropped");
        assert!(now.is_empty() && now.steps().len() == 2);
        assert_eq!(crate::select_tool::editable(d), Some((away, Combine::Intersect)));
        // Once the edit is done, nothing is selected: one way of having nothing.
        d.commit_pending();
        assert!(d.pixel_selection().is_none());
    }

    #[test]
    fn the_modifier_dialogs_show_what_they_make_at_once_and_keep_it_only_on_ok() {
        use opal_core::mask::{Combine, Extent, Shape};
        use opal_core::modify::Reach;
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let a = Shape::rectangle(&IRect::new(100, 50, 200, 150).to_f(), false);
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape: a, combine: Combine::Replace })), "Select");
        let states = d.history.nodes().count();
        let selected = |st: &AppState| st.active().unwrap().pixel_selection().unwrap().mask().extent();
        // Not while a box is open: its edit would be applied under the dialog's.
        assert!(st.active_mut().unwrap().begin_transform(opal_core::resample::Sampling::default()));
        let why = crate::commands::availability(&st, crate::commands::Cmd::GrowSelection).unwrap_err();
        assert!(why.contains("Apply or cancel the box first"), "{why}");
        st.active_mut().unwrap().cancel_transform();
        let ctx = eframe::egui::Context::default();
        // What a job makes, once done (the image keeps what it showed until then).
        let settle = |st: &mut AppState| {
            let mut waited = 0;
            while st.poll_modifying() {
                assert!(waited < 1000, "the preview never came");
                std::thread::sleep(std::time::Duration::from_millis(5));
                waited += 1;
            }
        };
        // Opened: grown by 5, worked out off the interface thread, the selection as it was
        // until then; as an edit not yet a state; a dialog, which keys and calls wait for.
        st.modify_selection(true, &ctx);
        assert!(st.dialog_open());
        assert_eq!(selected(&st), Extent::Within(IRect::new(100, 50, 200, 150)), "shown before it was worked out");
        settle(&mut st);
        assert_eq!(selected(&st), Extent::Within(IRect::new(95, 45, 205, 155)));
        assert_eq!(st.active().unwrap().pending_label(), Some("Grow selection"));
        // Set to shrink by 10, square: shown so, under its own name.
        st.modifying.as_mut().unwrap().step = Step::Grow { by: -10.0, corners: Reach::Square };
        st.preview_modifying(&ctx);
        settle(&mut st);
        assert_eq!(selected(&st), Extent::Within(IRect::new(110, 60, 190, 140)));
        assert_eq!(st.active().unwrap().pending_label(), Some("Shrink selection"));
        // Cancelled: as it was, no state.
        st.end_modifying(false);
        assert_eq!(selected(&st), Extent::Within(IRect::new(100, 50, 200, 150)));
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
        // OK while what it makes is worked out (a job taken as under way here, and asked for
        // after): the dialog waits, saying so, then closes, keeping it, once it is shown.
        st.modify_selection(true, &ctx);
        settle(&mut st);
        let m = st.modifying.as_mut().unwrap();
        m.step = Step::Grow { by: 30.0, corners: Reach::Square };
        m.asked += 1;
        st.end_modifying(true);
        assert!(st.modifying.as_ref().is_some_and(|m| m.keeping()), "closed before it was shown");
        assert_eq!(selected(&st), Extent::Within(IRect::new(95, 45, 205, 155)));
        st.preview_modifying(&ctx);
        settle(&mut st);
        assert!(st.modifying.is_none(), "still waiting once shown");
        assert_eq!(selected(&st), Extent::Within(IRect::new(70, 20, 230, 180)));
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Grow selection"));
        st.active_mut().unwrap().undo();
        // (Its state stays, to be redone: counted from here.)
        let states = st.active().unwrap().history.nodes().count();
        // Set to a step there is not memory enough for: the dialog says so, the image shows
        // the selection as it was, and OK keeps nothing, the dialog staying; a step that fits
        // is shown, and kept.
        st.modify_selection(true, &ctx);
        settle(&mut st);
        assert_eq!(st.active().unwrap().pending_label(), Some("Grow selection"));
        let pretend = crate::memory::testing::pretend_free(Some(1000));
        st.modifying.as_mut().unwrap().step = Step::Grow { by: 7.0, corners: Reach::Square };
        st.preview_modifying(&ctx);
        drop(pretend);
        settle(&mut st);
        let why = st.modifying.as_ref().unwrap().failed().map(str::to_owned);
        assert!(why.as_deref().is_some_and(|w| w.starts_with("This would need about")), "{why:?}");
        assert!(st.active().unwrap().pending_label().is_none(), "what was shown before stayed");
        assert_eq!(selected(&st), Extent::Within(IRect::new(100, 50, 200, 150)));
        st.end_modifying(true);
        assert!(st.modifying.as_ref().is_some_and(|m| !m.keeping()), "OK waited, or closed");
        // A later job's for the same step, done after all: shown, the refusal gone.
        let m = st.modifying.as_mut().unwrap();
        let fits = std::sync::Arc::new(m.before.then(m.step.clone()));
        m.ends.0.send((m.asked, Some((m.step.clone(), Ok(fits))))).unwrap();
        st.poll_modifying();
        let m = st.modifying.as_ref().unwrap();
        assert!(m.failed().is_none(), "the refusal stayed beside what was shown");
        st.modifying.as_mut().unwrap().step = Step::Grow { by: 6.0, corners: Reach::Square };
        st.preview_modifying(&ctx);
        settle(&mut st);
        assert!(st.modifying.as_ref().unwrap().failed().is_none());
        st.end_modifying(true);
        assert_eq!(selected(&st), Extent::Within(IRect::new(94, 44, 206, 156)));
        st.active_mut().unwrap().undo();
        let states = states + 1;
        // Cancel pressed in the frame what OK waited for comes: nothing kept.
        st.modify_selection(true, &ctx);
        settle(&mut st);
        let step = Step::Grow { by: 30.0, corners: Reach::Square };
        let m = st.modifying.as_mut().unwrap();
        m.step = step.clone();
        m.asked += 1;
        st.end_modifying(true);
        let m = st.modifying.as_ref().unwrap();
        let made = std::sync::Arc::new(m.before.then(step.clone()));
        m.ends.0.send((m.asked, Some((step.clone(), Ok(made))))).unwrap();
        st.answer_modifying(step, Some(false), &ctx);
        assert!(st.modifying.is_none());
        assert_eq!(st.active().unwrap().history.nodes().count(), states, "kept though cancelled");
        assert_eq!(selected(&st), Extent::Within(IRect::new(100, 50, 200, 150)));
        // Set twice before either is shown: the last shown until the later is done, the
        // earlier never.
        st.modify_selection(true, &ctx);
        settle(&mut st);
        st.modifying.as_mut().unwrap().step = Step::Grow { by: 30.0, corners: Reach::Square };
        st.preview_modifying(&ctx);
        st.modifying.as_mut().unwrap().step = Step::Grow { by: 20.0, corners: Reach::Square };
        st.preview_modifying(&ctx);
        assert_eq!(selected(&st), Extent::Within(IRect::new(95, 45, 205, 155)), "not the last shown meanwhile");
        settle(&mut st);
        assert_eq!(selected(&st), Extent::Within(IRect::new(80, 30, 220, 170)));
        st.end_modifying(false);
        // Its image is its own: another shown meanwhile is left alone, and Cancel takes back
        // the preview on its own image.
        let first = st.active_doc.unwrap();
        st.modify_selection(true, &ctx);
        st.create_image(&NewImage { width: 50, height: 50, background: Background::White, resolution: 72.0 });
        settle(&mut st);
        assert!(st.active().unwrap().pending_label().is_none(), "the other image took the preview");
        assert_eq!(st.doc(first).unwrap().pending_label(), Some("Grow selection"));
        // Saved meanwhile (the quit confirmation's Save all), the preview, which changes nothing
        // saved, stays a preview. (The file goes however the test ends.)
        struct Gone(std::path::PathBuf);
        impl Drop for Gone {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!("opal-modify-{}.opal", std::process::id()));
        let _gone = Gone(path.clone());
        st.save_to(first, path.clone(), crate::writer::Then::Nothing, &ctx);
        assert_eq!(st.doc(first).unwrap().pending_label(), Some("Grow selection"), "the save committed it");
        st.end_modifying(false);
        assert_eq!(st.doc(first).unwrap().history.nodes().count(), states, "cancelled, a state was left");
        // A box open, though, a save applies it: one that changes what is saved (its pixels
        // an image object while it is open), and one that does not (unmoved, over a layer
        // turned, an image object already).
        let sampling = opal_core::resample::Sampling::default();
        for changes in [true, false] {
            if !changes {
                let d = st.doc_mut(first).unwrap();
                assert!(d.begin_transform(sampling));
                let turned = d.transform.as_ref().unwrap().start.then(opal_core::geom::Affine::rotate(0.1));
                d.set_box(turned, None);
                d.finish_transform();
            }
            assert!(st.doc_mut(first).unwrap().begin_transform(sampling));
            let d = st.doc(first).unwrap();
            assert_eq!(!d.pending.as_ref().unwrap().doc.saves_as(d.history.doc()), changes);
            st.save_to(first, path.clone(), crate::writer::Then::Nothing, &ctx);
            assert!(st.doc(first).unwrap().transform.is_none(), "the box stayed open through a save");
        }
        st.show_doc(first);
        // Feathered, OK at once: kept once shown, a state of its own, the feather a step of the
        // recipe.
        st.modify_selection(false, &ctx);
        st.end_modifying(true);
        settle(&mut st);
        assert!(st.modifying.is_none());
        let d = st.active().unwrap();
        assert_eq!(d.history.undo_label(), Some("Feather selection"));
        assert!(matches!(d.pixel_selection().unwrap().steps(), [_, Step::Feather { .. }]));
        // The file saved is written before it goes.
        let mut waited = 0;
        while st.writer.busy() {
            assert!(waited < 2000, "the save never ended");
            st.poll_writes(false);
            std::thread::sleep(std::time::Duration::from_millis(5));
            waited += 1;
        }
    }

    #[test]
    fn an_image_not_drawn_has_its_drag_given_up_and_its_overlay_s_textures_dropped() {
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let a = Shape::rectangle(&IRect::new(0, 0, 100, 100).to_f(), true);
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape: a, combine: Combine::Replace })), "Select");
        // A box drag under way, its edit pending, the image drawn in pass 10.
        let before = d.pixel_selection().unwrap().before_last();
        let moved = Shape::rectangle(&IRect::new(20, 0, 120, 100).to_f(), true);
        crate::select_tool::reshape(d, &before, moved, Combine::Replace, "Move selection");
        d.shape_drag = Some(crate::select_tool::ShapeDrag::testing(a));
        d.drawn = 10;
        // Drawn in the pass before: kept.
        st.leave_undrawn(11);
        assert!(st.active().unwrap().shape_drag.is_some());
        // Not: given up, its edit with it.
        st.leave_undrawn(12);
        let d = st.active().unwrap();
        assert!(d.shape_drag.is_none() && d.pending_label().is_none(), "the drag was left");
        assert_eq!(crate::select_tool::editable(d), Some((a, Combine::Replace)));
    }

    #[test]
    fn a_locked_group_takes_no_new_layer_and_gives_no_layer_in_it_pixels() {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let mut ids: [Option<LayerId>; 2] = [None; 2];
        let d = st.active_mut().unwrap();
        d.apply("Group", |doc: &mut Document, made: &mut IdGen| {
            let g = made.layer();
            let mut group = Layer::new_group(g, "G");
            group.locked = true;
            doc.insert(Position { parent: None, index: 1 }, group)?;
            let a = made.layer();
            doc.insert(Position { parent: Some(g), index: 0 }, Layer::new_empty(a, "A"))?;
            ids = [Some(g), Some(a)];
            Ok::<(), opal_core::document::TreeError>(())
        })
        .unwrap();
        let [g, a] = ids.map(Option::unwrap);
        d.select_only(a);
        // A new layer goes above the locked group, not into it; so does a copy of A.
        let made = st.new_layer_named(false, Some("New")).unwrap();
        let doc = st.active().unwrap().doc();
        let (pos, group) = (doc.position(made).unwrap(), doc.position(g).unwrap());
        assert_eq!((pos.parent, pos.index), (None, group.index + 1), "added into the locked group");
        st.active_mut().unwrap().select_only(a);
        st.duplicate_layers();
        assert_eq!(st.active().unwrap().doc().layer(g).unwrap().children().unwrap().len(), 1, "copied into it");
        // Pixels cannot go into A, though it is not locked itself; into G, it is said to be a
        // group first.
        let refused = crate::api::pixels_go_into(st.active().unwrap().doc(), a).unwrap_err();
        assert!(refused.contains("\"G\" is locked"), "{refused}");
        let refused = crate::api::pixels_go_into(st.active().unwrap().doc(), g).unwrap_err();
        assert!(refused.contains("is a group"), "{refused}");
        // A is not grouped, moved up or down, or dragged out of it.
        st.active_mut().unwrap().select_only(a);
        for cmd in [crate::commands::Cmd::GroupLayers, crate::commands::Cmd::MoveUp] {
            let why = crate::commands::availability(&st, cmd).unwrap_err();
            assert!(why.contains("\"G\" is locked"), "{cmd:?}: {why}");
        }
        st.move_layer_to(a, Position { parent: None, index: 0 });
        assert_eq!(st.active().unwrap().doc().position(a).unwrap().parent, Some(g), "dragged out of it");
    }

    #[test]
    fn copies_put_above_a_locked_group_keep_their_order() {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let mut ids: [Option<LayerId>; 3] = [None; 3];
        let d = st.active_mut().unwrap();
        d.apply("Group", |doc: &mut Document, made: &mut IdGen| {
            let g = made.layer();
            let mut group = Layer::new_group(g, "G");
            group.locked = true;
            doc.insert(Position { parent: None, index: 1 }, group)?;
            for (i, name) in ["A", "B"].into_iter().enumerate() {
                let id = made.layer();
                doc.insert(Position { parent: Some(g), index: i }, Layer::new_empty(id, name))?;
                ids[i + 1] = Some(id);
            }
            ids[0] = Some(g);
            Ok::<(), opal_core::document::TreeError>(())
        })
        .unwrap();
        let [_, a, b] = ids.map(Option::unwrap);
        d.select_only(a);
        d.toggle_selected(b);
        st.duplicate_layers();
        let names: Vec<String> = st.active().unwrap().doc().layers.iter().map(|l| l.name.clone()).collect();
        assert_eq!(names, ["Background", "G", "A copy", "B copy"]);
    }

    #[test]
    fn a_selected_group_is_cleared_where_it_shows_and_refused_as_its_own_flags_say() {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        // A group holding A, shown, and B, hidden, each red all over.
        let red = opal_core::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let mut ids: [Option<LayerId>; 3] = [None; 3];
        let d = st.active_mut().unwrap();
        d.apply("Group", |doc: &mut Document, made: &mut IdGen| {
            let g = made.layer();
            doc.insert(Position { parent: None, index: 1 }, Layer::new_group(g, "G"))?;
            for (i, name) in ["A", "B"].into_iter().enumerate() {
                let id = made.layer();
                let mut l =
                    Layer::new_raster(id, made.object(), name, RasterImage::filled(&IRect::new(0, 0, 300, 200), red));
                l.visible = name == "A";
                doc.insert(Position { parent: Some(g), index: i }, l)?;
                ids[i + 1] = Some(id);
            }
            ids[0] = Some(g);
            Ok::<(), opal_core::document::TreeError>(())
        })
        .unwrap();
        let [g, a, b] = ids.map(Option::unwrap);
        d.select_only(g);
        st.select_all();
        let flag = |st: &mut AppState, id: LayerId, f: fn(&mut Layer)| {
            st.active_mut()
                .unwrap()
                .apply("Flag", |doc, _| {
                    f(doc.layer_mut(id).unwrap());
                    Ok::<(), ()>(())
                })
                .unwrap();
        };
        let refused = |st: &AppState| super::clear_refused(st.active().unwrap()).err().unwrap_or_default();
        // The group locked: refused, though nothing inside it is.
        flag(&mut st, g, |l| l.locked = true);
        assert!(refused(&st).contains("\"G\" is locked"), "{}", refused(&st));
        // Hidden instead: refused too.
        flag(&mut st, g, |l| (l.locked, l.visible) = (false, false));
        assert!(refused(&st).contains("\"G\" is hidden"), "{}", refused(&st));
        // Shown, with B (hidden inside it) locked: B is no part of it, so cleared, A alone.
        flag(&mut st, g, |l| l.visible = true);
        flag(&mut st, b, |l| l.locked = true);
        assert_eq!(refused(&st), "");
        st.clear_selected();
        let alpha = |st: &AppState, id: LayerId| {
            let LayerContent::Objects(o) = &st.active().unwrap().doc().layer(id).unwrap().content else { panic!() };
            match o.first().map(|o| &o.kind) {
                Some(ObjectKind::Pixels(p)) => opal_core::tile::pixel_to_f32(p.image.pixel(150, 100))[3],
                _ => 0.0,
            }
        };
        assert_eq!([alpha(&st, a), alpha(&st, b)], [0.0, 1.0], "A cleared, B, hidden, left as it was");
        // A locked: refused.
        flag(&mut st, a, |l| l.locked = true);
        assert!(refused(&st).contains("\"A\" is locked"), "{}", refused(&st));
        // A selected itself, in G locked: refused, as are Transform and the Move tool.
        flag(&mut st, a, |l| l.locked = false);
        flag(&mut st, g, |l| l.locked = true);
        st.active_mut().unwrap().select_only(a);
        assert!(refused(&st).contains("\"G\" is locked"), "{}", refused(&st));
        let transform = crate::commands::availability(&st, crate::commands::Cmd::Transform).unwrap_err();
        assert!(transform.contains("\"G\" is locked"), "{transform}");
        assert_eq!(crate::move_tool::locked_in_selected_layers(st.active().unwrap()).as_deref(), Some("G"));
        // In G hidden: refused.
        flag(&mut st, g, |l| (l.locked, l.visible) = (false, false));
        assert!(refused(&st).contains("\"G\" is hidden"), "{}", refused(&st));
    }
}
