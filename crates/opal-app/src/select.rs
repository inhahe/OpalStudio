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

/// A selection's marching ants: closed lines round what its mask selects, in document
/// coordinates (see `Mask::outline`).
pub type Ants = Arc<Vec<Vec<[f64; 2]>>>;

/// A modifier's dialog (Grow or Shrink, Feather, Smooth, Border), while it is open: the image it
/// acts on, the step it adds (or, from the selection panel, the step of the recipe it changes),
/// the selection it acts on, and the step that image's pending edit shows. What a step set
/// makes is worked out off the interface thread (`AppState::preview_modifying`), the latest
/// only.
#[derive(Debug)]
pub struct Modifying {
    doc: DocId,
    pub step: Step,
    pub before: Arc<Selection>,
    /// Which of `before`'s steps it changes, if it changes one rather than adds one: the
    /// selection is worked out again from its steps with that one changed, the steps before it
    /// once (`prefix`, made by the first job that needs it and kept for the rest).
    at: Option<usize>,
    prefix: Arc<std::sync::Mutex<Option<Arc<Selection>>>>,
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

    /// Whether it changes a step of the recipe (from the selection panel) rather than adds one.
    pub fn changes_a_step(&self) -> bool {
        self.at.is_some()
    }

    /// The history state what it makes with `step` is: the step added, or a step changed.
    fn label(&self, step: &Step) -> &'static str {
        if self.at.is_some() { STEP_CHANGED } else { modifier_label(step) }
    }

    /// What it makes with `step` from its selection `before` (changing step `at`, if it changes
    /// one, the steps before that kept worked out in `prefix`), unless `bounds` hold the work
    /// back. A step changed to what it was makes the selection as it is; one changed to leave
    /// the selection as it is is no step, as when it is added.
    fn make(
        at: Option<usize>,
        prefix: &std::sync::Mutex<Option<Arc<Selection>>>,
        before: &Arc<Selection>,
        step: Step,
        bounds: opal_core::modify::Bounds,
    ) -> Result<Selection, opal_core::modify::Halt> {
        let Some(i) = at else { return before.then_within(step, bounds) };
        if before.steps()[i] == step {
            return Ok((**before).clone());
        }
        let kept = prefix.lock().expect("not poisoned").clone();
        let first = match kept {
            Some(first) => first,
            None => {
                let first = Arc::new(Selection::of_within(before.steps()[..i].to_vec(), bounds)?);
                *prefix.lock().expect("not poisoned") = Some(first.clone());
                first
            }
        };
        let mut made = first.then_within(step, bounds)?;
        for later in &before.steps()[i + 1..] {
            made = made.then_within(later.clone(), bounds)?;
        }
        Ok(made)
    }
}

/// `before` carried by `m` (see `Selection::transformed`), within `memory`, unless `stop` says
/// to stop; nothing selected, if it comes to nothing. What Transform Selection, Reselect and
/// Rotate, Flip, Resize Image and Crop carry a selection with.
pub(crate) fn carried(
    before: &Selection,
    m: &opal_core::geom::Affine,
    memory: crate::memory::Allowance,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Arc<Selection>>, String> {
    let made = crate::memory::Admission::within(memory, stop, |b| before.transformed_within(m, b))?;
    Ok((!made.is_empty()).then(|| Arc::new(made)))
}

/// The Save Selection dialog's field: the name it is saved under.
#[derive(Clone, Debug)]
pub struct SaveSelection {
    pub name: String,
}

/// The Load Selection dialog's settings: which saved selection, how it combines with what is
/// selected, and whether it is inverted first.
#[derive(Clone, Debug)]
pub struct LoadSelection {
    pub name: String,
    pub combine: Combine,
    pub invert: bool,
}

/// A name to save a selection in `doc` under that none has: "Selection 1", "Selection 2"...
pub fn next_selection_name(doc: &Document) -> String {
    (1..).map(|n| format!("Selection {n}")).find(|name| doc.saved_selection(name).is_none()).expect("one is free")
}

/// What `make` makes within the memory free now (measured here), or why there was not memory
/// enough for it: what a selection made on the interface thread, by a tool or a script's call,
/// is made within.
pub(crate) fn within_memory<T>(
    make: impl FnOnce(opal_core::modify::Bounds) -> Result<T, opal_core::modify::Halt>,
) -> Result<T, String> {
    crate::memory::Admission::within(crate::memory::Measure::here().now(), &|| false, make)
}

/// The selection `kept` is, brought back: carried as the image has changed since, if it has
/// (within the memory `measure` finds, unless `stop` says to stop); nothing, if it selects
/// nothing. What Load Selection and Reselect bring back.
pub(crate) fn brought_back(
    kept: &opal_core::document::KeptSelection,
    measure: crate::memory::Measure,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Arc<Selection>>, String> {
    if kept.since == opal_core::geom::Affine::IDENTITY {
        return Ok(Some(kept.selection.clone()).filter(|s| !s.is_empty()));
    }
    carried(&kept.selection, &kept.since, measure.now(), stop)
}

/// The history state a step of the recipe changed (in the selection panel, or by a script) is.
pub const STEP_CHANGED: &str = "Change selection step";

/// A step a modifier dialog was set to, and what it makes, or why that could not be made.
type Preview = (Step, Result<Arc<Selection>, String>);

/// A modifier dialog's job, ended: its number, and what it made, if anything.
type Ended = (u64, Option<Preview>);

/// The history state adding `step`, a modifier, makes.
pub fn modifier_label(step: &Step) -> &'static str {
    match step {
        Step::Grow { by, .. } if *by < 0.0 => "Shrink selection",
        Step::Grow { .. } => "Grow selection",
        Step::Smooth { .. } => "Smooth selection",
        Step::Border { .. } => "Border selection",
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

    /// Selects what the maker `step` makes with what is selected (see `with_maker`), as a
    /// history state labeled `label`, its mask made within the memory free now; or says why
    /// not: there was not memory enough for it (nothing changes then).
    pub fn select_with(&mut self, step: Step, label: &str) -> Result<(), String> {
        let before = self.pixel_selection().cloned();
        let made = within_memory(|b| opal_core::selection::with_maker_within(before.as_deref(), step, b))?;
        self.set_pixel_selection(Some(made), label);
        Ok(())
    }

    /// Selects nothing, as a history state labeled "Deselect", keeping what was selected for
    /// Reselect (`Document::deselected`); with nothing selected, nothing happens.
    pub fn deselect_pixels(&mut self) {
        if self.pixel_selection().is_none() {
            return;
        }
        let _ = self.apply("Deselect", |doc, _| {
            // (A pending edit that left nothing selected, committed first, leaves nothing.)
            if let Some(selection) = doc.selection.take() {
                doc.deselected = Some(opal_core::document::KeptSelection::new(selection));
            }
            Ok::<(), ()>(())
        });
    }

    /// The outline of what is selected (the marching ants' path, in document coordinates),
    /// round the pixels selected at least `threshold` (a level): worked out once for each
    /// selection's mask and threshold, and kept until another is asked for, or none (nothing
    /// is selected).
    pub fn selection_outline(&mut self, threshold: u16) -> Option<Ants> {
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
            d.outlining = None;
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
        self.active_mut().expect("available").deselect_pixels();
    }

    /// Saves what the active image has selected under `name` (in place of a selection saved
    /// under it, without regard to case, if there is one), as a history state: kept in the
    /// image, and in its file.
    pub(crate) fn save_selection(&mut self, name: &str) {
        use opal_core::document::{KeptSelection, SavedSelection};
        let Some(name) = SavedSelection::name_of(name).map(str::to_owned) else {
            return self.message(MessageKind::Info, "Save Selection", "Give it a name.");
        };
        let d = self.active_mut().expect("available");
        // What is selected once an edit in progress (a box) is applied: read as the state is made.
        let saved = d.apply("Save selection", |doc, _| {
            let Some(selection) = doc.selection.clone().filter(|s| !s.is_empty()) else { return Err(()) };
            let kept = KeptSelection::new(selection);
            match doc.saved.iter_mut().find(|s| s.named(&name)) {
                Some(s) => *s = SavedSelection { name, kept },
                None => doc.saved.push(SavedSelection { name, kept }),
            }
            Ok(())
        });
        if saved == Err(crate::doc::NotMade::Failed(())) {
            self.message(MessageKind::Info, "Save Selection", "Nothing is selected.");
        }
    }

    /// Selects the selection saved in the active image as `name`, inverted if `invert`,
    /// combined with what is selected as `combine` says (replacing it, its steps come back, to
    /// change in the selection panel; else it is a step of its own); carried as the image has
    /// changed since it was saved, if it has. Worked out off the interface thread (see
    /// `edits`), memory measured as it goes; refused, saying why, if it would leave nothing
    /// selected.
    pub(crate) fn load_selection(&mut self, name: &str, combine: Combine, invert: bool) {
        let d = self.active().expect("available");
        let Some(saved) = d.doc().saved_selection(name).cloned() else {
            let why = format!("No selection is saved as \"{name}\" in this image.");
            return self.message(MessageKind::Info, "Load Selection", why);
        };
        let (id, measure) = (d.id, crate::memory::Measure::here());
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            let mut loaded = brought_back(&saved.kept, measure, &|| progress.is_cancelled())?;
            if invert {
                let s = loaded.map_or_else(|| Selection::of(Vec::new()), |s| (*s).clone()).then(Step::Invert);
                loaded = (!s.is_empty()).then(|| Arc::new(s));
            }
            let made = match (combine, loaded) {
                // With nothing selected, adding it is bringing it back as it was, its recipe too.
                (Combine::Replace, loaded) => loaded,
                (Combine::Add, loaded) if doc.selection.is_none() => loaded,
                (_, Some(s)) => {
                    let step = Step::Mask { mask: s.mask().clone(), combine, of: opal_core::selection::MaskOf::Saved };
                    let made = opal_core::selection::with_maker(doc.selection.as_deref(), step);
                    (!made.is_empty()).then(|| Arc::new(made))
                }
                // Nothing to add, take away or keep what is in both with.
                (Combine::Add, None) => doc.selection.clone(),
                (Combine::Subtract, None) => doc.selection.clone(),
                (Combine::Intersect, None) => None,
            };
            if made.as_ref().is_none_or(|s| s.is_empty()) {
                return Err("Nothing would be selected.".to_owned());
            }
            doc.selection = made;
            Ok(())
        };
        self.edit_later(id, "Load selection", crate::memory::Allowance::unlimited(), work, |_, _, ()| {});
    }

    /// Deletes the selection saved in the active image as `name`, as a history state.
    pub(crate) fn delete_saved_selection(&mut self, name: &str) {
        let d = self.active_mut().expect("available");
        let _ = d.apply("Delete saved selection", |doc, _| {
            doc.saved.retain(|s| !s.named(name));
            Ok::<(), ()>(())
        });
    }

    /// Selects again what Deselect last took away in the active image; carried as the image has
    /// been turned, flipped, resized or cropped since, if it has, off the interface thread (see
    /// `edits`), memory measured as it goes.
    pub(crate) fn reselect(&mut self) {
        let d = self.active_mut().expect("available");
        let Some(again) = d.doc().deselected.clone() else { return };
        if again.since == opal_core::geom::Affine::IDENTITY {
            return d.set_pixel_selection(Some((*again.selection).clone()), "Reselect");
        }
        let (id, measure) = (d.id, crate::memory::Measure::here());
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            doc.selection = brought_back(&again, measure, &|| progress.is_cancelled())?;
            Ok(())
        };
        self.edit_later(id, "Reselect", crate::memory::Allowance::unlimited(), work, |_, _, ()| {});
    }

    /// Selects layer `id` of the active image's pixels, each as much as it is opaque (as the
    /// layer holds them, whatever its opacity and visibility), combined with what is selected
    /// as `combine` says, as a history state worked out off the interface thread (see `edits`),
    /// the mask's memory measured as it is made. Refused, saying why, if it would select
    /// nothing (what is selected stays).
    pub(crate) fn select_layer_pixels(&mut self, id: LayerId, combine: Combine) {
        let d = self.active().expect("available");
        let Some(layer) = d.doc().layer(id) else { return };
        let layer = Arc::new(Layer { visible: true, opacity: 1.0, ..layer.clone() });
        let memory = match crate::layer_ops::check_size(std::slice::from_ref(&layer), &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Select Layer's Pixels", why),
        };
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            let image = crate::edits::composited(&[layer], None, progress)?;
            let stop = || progress.is_cancelled();
            let mask = crate::memory::Admission::within(measure.now(), &stop, |b| Mask::of_opacity_within(&image, b))?;
            if mask.is_none() {
                return Err("The layer has no pixels to select.".to_owned());
            }
            let made = opal_core::selection::with_maker(
                doc.selection.as_deref(),
                Step::Mask { mask: Arc::new(mask), combine, of: opal_core::selection::MaskOf::LayerPixels },
            );
            // What is selected stays, rather than taken away with no word.
            if made.is_empty() {
                return Err("Nothing would be left selected.".to_owned());
            }
            doc.selection = Some(Arc::new(made));
            Ok(())
        };
        self.edit_later(d.id, "Select layer's pixels", memory, work, |_, _, ()| {});
    }

    /// Selects what is not selected, and nothing of what is (with everything selected,
    /// nothing is left selected).
    pub(crate) fn invert_selection(&mut self) {
        let d = self.active_mut().expect("available");
        let s = d.pixel_selection().expect("available").then(Step::Invert);
        d.set_pixel_selection(Some(s), "Invert selection");
    }

    /// Opens the dialog of `cmd`, a modifier (Grow or Shrink, Feather, Smooth or Border), on
    /// what the active image has selected, what it makes worked out off the interface thread at
    /// once.
    pub(crate) fn modify_selection(&mut self, cmd: crate::commands::Cmd, ctx: &eframe::egui::Context) {
        use crate::commands::Cmd;
        use opal_core::modify::{Falloff, Reach, Side};
        let d = self.active().expect("available");
        let (doc, before) = (d.id, d.pixel_selection().expect("available").clone());
        let step = match cmd {
            Cmd::GrowSelection => Step::Grow { by: 5.0, corners: Reach::Round },
            Cmd::SmoothSelection => Step::Smooth { radius: 4.0 },
            Cmd::BorderSelection => Step::Border { width: 4.0, side: Side::Centered },
            _ => Step::Feather { width: 10.0, side: Side::Centered, falloff: Falloff::Gaussian },
        };
        self.modifying = Some(Modifying {
            doc,
            step,
            before,
            at: None,
            prefix: Default::default(),
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

    /// Why the steps of the active image's selection may not be changed now, if they may not:
    /// a box is open on it, whose edit changing them would apply under the user (the panel
    /// greys its buttons, and scripts wait, meanwhile).
    pub(crate) fn steps_refused(&self) -> Option<&'static str> {
        self.active().is_some_and(|d| d.transform.is_some()).then_some("Apply or cancel the box first.")
    }

    /// Opens the dialog of step `index` of the active image's selection, a modifier, to change
    /// it (from the selection panel): what the selection then comes to, worked out again from
    /// its steps with that one changed, shown at once, worked out off the interface thread.
    pub(crate) fn edit_selection_step(&mut self, index: usize, ctx: &eframe::egui::Context) {
        if let Some(why) = self.steps_refused() {
            return self.message(MessageKind::Info, "Selection", why);
        }
        let d = self.active().expect("an image with a selection");
        let before = d.pixel_selection().expect("a selection").clone();
        let Some(step) = before.steps().get(index).cloned() else { return };
        self.modifying = Some(Modifying {
            doc: d.id,
            step,
            before,
            at: Some(index),
            prefix: Default::default(),
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

    /// Takes step `index` away from the active image's selection, which is worked out again
    /// from the rest (nothing selected, if none is left).
    pub(crate) fn remove_selection_step(&mut self, index: usize) {
        if let Some(why) = self.steps_refused() {
            return self.message(MessageKind::Info, "Selection", why);
        }
        let d = self.active().expect("an image with a selection");
        let mut steps = d.pixel_selection().expect("a selection").steps().to_vec();
        if index < steps.len() {
            steps.remove(index);
            self.rework_selection(steps, "Remove selection step");
        }
    }

    /// Makes step `index` of the active image's selection, a maker (a shape, or a mask), combine
    /// with the steps before it as `combine` says; the selection is worked out again.
    pub(crate) fn combine_selection_step(&mut self, index: usize, combine: Combine) {
        if let Some(why) = self.steps_refused() {
            return self.message(MessageKind::Info, "Selection", why);
        }
        let d = self.active().expect("an image with a selection");
        let mut steps = d.pixel_selection().expect("a selection").steps().to_vec();
        match steps.get_mut(index) {
            Some(Step::Shape { combine: c, .. } | Step::Outline { combine: c, .. } | Step::Mask { combine: c, .. })
                if *c != combine =>
            {
                *c = combine
            }
            _ => return,
        }
        self.rework_selection(steps, STEP_CHANGED);
    }

    /// Makes `steps` the recipe of the active image's selection, the selection worked out again
    /// from them off the interface thread (see `edits`), memory measured as it goes, as a
    /// history state labeled `label`. Coming to nothing, what was selected is kept for Reselect,
    /// as Deselect keeps it.
    pub(crate) fn rework_selection(&mut self, steps: Vec<Step>, label: &'static str) {
        let id = self.active_doc.expect("an image");
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            let stop = || progress.is_cancelled();
            let made = crate::memory::Admission::within(measure.now(), &stop, |b| Selection::of_within(steps, b))?;
            if !made.is_empty() {
                doc.selection = Some(Arc::new(made));
            } else if let Some(selection) = doc.selection.take() {
                doc.deselected = Some(opal_core::document::KeptSelection::new(selection));
            }
            Ok(())
        };
        self.edit_later(id, label, crate::memory::Allowance::unlimited(), work, |_, _, ()| {});
    }

    /// Shows `made`, what the open modifier dialog makes with `step`, as its image's pending
    /// edit, named for what the dialog does (the edit named for the one shown before given up).
    fn show_made(&mut self, step: Step, made: Arc<Selection>) {
        let Some(m) = self.modifying.as_mut() else { return };
        m.failed = None;
        let before = m.shown.replace(step.clone());
        let (doc, label) = (m.doc, m.label(&step));
        let was = before.map(|b| m.label(&b));
        let Some(d) = self.doc_mut(doc) else { return };
        if let Some(was) = was.filter(|&b| b != label) {
            d.discard_pending(was);
        }
        d.edit_pending(label, |doc| doc.selection = (!made.is_empty()).then_some(made));
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
        let (step, before, generation, at) = (m.step.clone(), m.before.clone(), m.generation.clone(), m.at);
        let prefix = m.prefix.clone();
        let (ends, asked) = (m.ends.0.clone(), m.step.clone());
        let measure = crate::memory::Measure::here();
        self.previews.run(
            move || {
                let stop = || generation.load(Ordering::SeqCst) != ours;
                // Asked for after another, it may not be wanted by the time it begins.
                if stop() {
                    return None;
                }
                // What is free, measured as the work begins.
                match crate::memory::Admission::within(measure.now(), &stop, |b| {
                    Modifying::make(at, &prefix, &before, step, b)
                }) {
                    Ok(made) => Some(Ok(Arc::new(made))),
                    // Not wanted any more: nothing to tell, whatever held it back.
                    Err(_) if stop() => None,
                    Err(why) => Some(Err(why)),
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
        let (shown, doc) = (m.shown.take().map(|s| m.label(&s)), m.doc);
        if let (Some(shown), Some(d)) = (shown, self.doc_mut(doc)) {
            d.discard_pending(shown);
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
        } else if let Some(shown) = &m.shown {
            d.discard_pending(m.label(shown));
        }
    }

    /// Adds `step`, a modifier, to what is selected, as a history state, worked out off the
    /// interface thread (see `edits`); refused, saying why, if there is not memory enough for
    /// it, measured as it goes.
    pub(crate) fn add_modifier(&mut self, step: Step) {
        let d = self.active().expect("available");
        let before = d.pixel_selection().expect("available").clone();
        let label = modifier_label(&step);
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            let stop = || progress.is_cancelled();
            let made = crate::memory::Admission::within(measure.now(), &stop, |b| before.then_within(step, b))?;
            doc.selection = (!made.is_empty()).then(|| Arc::new(made));
            Ok(())
        };
        self.edit_later(d.id, label, crate::memory::Allowance::unlimited(), work, |_, _, ()| {});
    }

    /// Transform Selection: the box round what is selected (see
    /// [`DocState::begin_selection_transform`]), or why not.
    pub(crate) fn transform_selection(&mut self) {
        let d = self.active_mut().expect("available");
        if let Err(why) = d.begin_selection_transform() {
            self.message(MessageKind::Info, "Transform Selection", why);
        }
    }

    /// Applies the box open on image `id`, as Enter and its Apply button do: a Transform
    /// Selection box's selection worked out off the interface thread (see `edits`), since a
    /// recipe's modifiers are worked out again where they are carried; any other box at once.
    pub(crate) fn commit_box(&mut self, id: DocId) {
        let Some(d) = self.doc_mut(id) else { return };
        let Some(s) = d.transform.as_ref() else { return };
        if s.kind != crate::transform::Kind::Selection || s.current == s.start {
            return d.commit_pending();
        }
        let delta = s.delta();
        let before = d.pixel_selection().expect("a box round the selection").clone();
        d.cancel_transform();
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            doc.selection = carried(&before, &delta, measure.now(), &|| progress.is_cancelled())?;
            Ok(())
        };
        let label = crate::transform::Kind::Selection.label();
        self.edit_later(id, label, crate::memory::Allowance::unlimited(), work, |_, _, ()| {});
    }

    /// Layer via Copy, or (`cut`) Layer via Cut: what the selected layers show within the
    /// selection, as much of each pixel as it is selected, past the canvas too, on a new layer
    /// just above the topmost of them, which becomes the one selected; cut, it is erased from
    /// them, as Clear erases. With nothing selected, Layer via Copy duplicates the layers.
    /// Worked out off the interface thread (see `edits`).
    pub(crate) fn layer_via(&mut self, cut: bool) {
        let d = self.active().expect("available");
        let Some(selection) = d.pixel_selection() else {
            return self.duplicate_layers();
        };
        let mask = selection.mask().clone();
        let title = if cut { "Layer via Cut" } else { "Layer via Copy" };
        // Only within the selection's reach: what lies past it is left out anyway.
        let range = match mask.extent() {
            Extent::Within(b) => Some(b),
            Extent::Unbounded => None,
            Extent::Nothing => return self.message(MessageKind::Info, title, "Nothing is selected."),
        };
        let doc = d.doc();
        let selected = d.selected_layers();
        let top = *selected.last().expect("available");
        let layers = d.selected_as_shown();
        let erased: Vec<LayerId> =
            if cut { cleared(d).iter().filter(|l| !l.is_group()).map(|l| l.id).collect() } else { Vec::new() };
        let memory = match range {
            Some(b) => {
                let mut memory = crate::memory::Allowance::now();
                memory.take_tiles(opal_core::composite::composite_cost(&layers, Some(&b))).map(|()| memory)
            }
            None => crate::layer_ops::check_size(&layers, &doc.canvas.rect()),
        };
        let memory = match memory {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, title, why),
        };
        let work = move |doc: &mut Document, ids: &mut opal_core::document::IdGen, progress: &opal_io::Progress| {
            let flat = crate::edits::composited(&layers, range.as_ref(), progress)?;
            let kept = opal_core::mask::keep_selected(&flat, [0, 0], &mask);
            if kept.content_bounds().is_none() {
                return Err("The selected layers show nothing within the selection.".to_owned());
            }
            if cut {
                clear(doc, &erased, &mask);
            }
            let id = ids.layer();
            let pos = crate::layer_ops::above(doc, top).ok_or("The layer is gone.")?;
            // Named as the edit finds it (a rename left open is finished as it begins).
            let name = format!("{} copy", doc.layer(top).map_or("Layer", |l| l.name.as_str()));
            doc.insert(pos, Layer::new_raster(id, ids.object(), name, kept)).map_err(|e| e.to_string())?;
            Ok(id)
        };
        let label = if cut { "Layer via cut" } else { "Layer via copy" };
        self.edit_later(d.id, label, memory, work, |st, doc, id| {
            if let Some(d) = st.doc_mut(doc) {
                d.select_only(id);
            }
        });
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
        if result == Err(crate::doc::NotMade::Failed(())) {
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
    fn a_layer_s_pixels_are_selected_as_they_are_opaque_and_reselect_brings_back_what_deselect_took() {
        use crate::commands::Cmd;
        use opal_core::mask::{Combine, FULL, Shape, level};
        use opal_core::selection::{Selection, Step};
        use opal_core::tile::{Tile, TileCoord};
        use std::sync::Arc;
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 600, height: 300, background: Background::White, resolution: 72.0 });
        let ctx = eframe::egui::Context::default();
        // A layer holding a tile opaque, and one beside it half transparent; hidden, and at
        // half opacity, which do not count.
        let d = st.active_mut().unwrap();
        let mut img = RasterImage::new();
        let opaque = [half::f16::from_f32(1.0); 4];
        img.set_tile(TileCoord::new(0, 0), Some(Arc::new(Tile::filled(opaque))));
        img.set_tile(TileCoord::new(1, 0), Some(Arc::new(Tile::filled([half::f16::from_f32(0.5); 4]))));
        let mut made = None;
        let _ = d.apply("Add", |doc, ids| {
            let id = ids.layer();
            let mut l = Layer::new_raster(id, ids.object(), "pixels", img);
            (l.visible, l.opacity) = (false, 0.5);
            made = Some(id);
            doc.insert(Position { parent: None, index: 1 }, l).map_err(|_| ())
        });
        let id = made.unwrap();
        let level_at =
            |st: &AppState, x, y| st.active().unwrap().pixel_selection().map_or(0, |s| s.mask().level_at(x, y));
        st.select_layer_pixels(id, Combine::Replace);
        assert_eq!([level_at(&st, 10, 10), level_at(&st, 300, 10), level_at(&st, 550, 10)], [FULL, level(0.5), 0]);
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Select layer's pixels"));
        // Added to a rectangle beside it: both, as a step of the recipe after the rectangle.
        let shape = Shape::rectangle(&IRect::new(520, 0, 580, 50).to_f(), false);
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
        st.select_layer_pixels(id, Combine::Add);
        let s = st.active().unwrap().pixel_selection().unwrap().clone();
        assert!(matches!(s.steps(), [Step::Shape { .. }, Step::Mask { combine: Combine::Add, .. }]), "{:?}", s.steps());
        assert_eq!([level_at(&st, 10, 10), level_at(&st, 550, 10)], [FULL, FULL]);
        // Deselected, then reselected: the same selection, steps and all, a state each.
        st.run(Cmd::Deselect, &ctx);
        assert!(st.active().unwrap().pixel_selection().is_none());
        st.run(Cmd::Reselect, &ctx);
        let d = st.active().unwrap();
        assert_eq!(d.pixel_selection(), Some(&s));
        assert_eq!(d.history.undo_label(), Some("Reselect"));
        // With nothing selected, taking pixels away leaves nothing: refused, saying so.
        st.run(Cmd::Deselect, &ctx);
        st.select_layer_pixels(id, Combine::Subtract);
        assert!(st.active().unwrap().pixel_selection().is_none());
        assert!(st.messages.back().is_some_and(|m| m.body.contains("Nothing would be left selected")));
    }

    #[test]
    fn transform_selection_carries_the_selection_itself_worked_out_off_the_interface_thread() {
        use crate::commands::{Cmd, availability};
        use opal_core::geom::Affine;
        use opal_core::mask::{Combine, Extent, Shape};
        use opal_core::modify::Reach;
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let ctx = eframe::egui::Context::default();
        let selected = |st: &AppState| st.active().unwrap().pixel_selection().unwrap().mask().extent();
        let shape = Shape::rectangle(&IRect::new(20, 20, 120, 80).to_f(), false);
        let grown = Selection::new(Step::Shape { shape, combine: Combine::Replace })
            .then(Step::Grow { by: 5.0, corners: Reach::Square });
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(grown), "Select");
        let layers = d.doc().layers.clone();
        st.run(Cmd::TransformSelection, &ctx);
        let d = st.active_mut().unwrap();
        assert_eq!(d.transform.as_ref().expect("no box").frame, IRect::new(15, 15, 125, 85).to_f());
        // Twice the size, from the canvas's corner: the shape carried exactly, its grow worked
        // out again, twice as far; nothing else changed.
        d.set_box(Affine::scale(2.0, 2.0), None);
        st.edit_wait = std::time::Duration::ZERO;
        let held = crate::background::testing::hold(Some(crate::edits::pool()));
        let id = st.active_doc.unwrap();
        st.commit_box(id);
        let off_thread = st.editing();
        let closed = st.active().unwrap().transform.is_none();
        drop(held);
        st.finish_edits();
        assert!(off_thread, "worked out on the interface thread");
        assert!(closed, "the box stayed");
        assert_eq!(selected(&st), Extent::Within(IRect::new(30, 30, 250, 170)));
        let d = st.active().unwrap();
        assert_eq!(d.history.undo_label(), Some("Transform selection"));
        assert!(d.doc().layers.iter().zip(&layers).all(|(a, b)| std::sync::Arc::ptr_eq(a, b)), "a layer changed");
        // Applied another way (as another change is made), at once.
        st.run(Cmd::TransformSelection, &ctx);
        let d = st.active_mut().unwrap();
        d.set_box(Affine::translate(10.0, 0.0), None);
        d.commit_pending();
        assert_eq!(selected(&st), Extent::Within(IRect::new(40, 30, 260, 170)));
        // Cancelled, or left where it was: nothing, and no history state.
        let states = st.active().unwrap().history.nodes().count();
        st.run(Cmd::TransformSelection, &ctx);
        let d = st.active_mut().unwrap();
        d.set_box(Affine::translate(10.0, 0.0), None);
        d.cancel_transform();
        st.run(Cmd::TransformSelection, &ctx);
        st.commit_box(id);
        assert_eq!(selected(&st), Extent::Within(IRect::new(40, 30, 260, 170)));
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
        // Inverted, it reaches past every edge: no box goes round it.
        st.run(Cmd::InvertSelection, &ctx);
        let why = availability(&st, Cmd::TransformSelection).unwrap_err();
        assert!(why.contains("no box"), "{why}");
    }

    #[test]
    fn the_selection_panel_takes_steps_away_and_changes_them_the_rest_worked_out_again() {
        use crate::select::STEP_CHANGED;
        use opal_core::mask::{Combine, Extent, Shape};
        use opal_core::modify::Reach;
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let ctx = eframe::egui::Context::default();
        let selected = |st: &AppState| st.active().unwrap().pixel_selection().map(|s| s.mask().extent());
        let steps = |st: &AppState| st.active().unwrap().pixel_selection().unwrap().steps().len();
        let label = |st: &AppState| st.active().unwrap().history.undo_label().map(str::to_owned);
        let rect = |x0, y0, x1, y1| Shape::rectangle(&IRect::new(x0, y0, x1, y1).to_f(), false);
        // Two rectangles, the second added, grown by 5 with square corners.
        let made = Selection::new(Step::Shape { shape: rect(20, 20, 120, 80), combine: Combine::Replace })
            .then(Step::Shape { shape: rect(200, 20, 260, 80), combine: Combine::Add })
            .then(Step::Grow { by: 5.0, corners: Reach::Square });
        st.active_mut().unwrap().set_pixel_selection(Some(made), "Select");
        assert_eq!(selected(&st), Some(Extent::Within(IRect::new(15, 15, 265, 85))));
        // The second taken away rather than added: the first alone, grown.
        st.combine_selection_step(1, Combine::Subtract);
        assert_eq!(selected(&st), Some(Extent::Within(IRect::new(15, 15, 125, 85))));
        assert_eq!((steps(&st), label(&st).as_deref()), (3, Some(STEP_CHANGED)));
        st.active_mut().unwrap().undo();
        // Taken away: the same, a step fewer.
        st.remove_selection_step(1);
        assert_eq!(selected(&st), Some(Extent::Within(IRect::new(15, 15, 125, 85))));
        assert_eq!((steps(&st), label(&st).as_deref()), (2, Some("Remove selection step")));
        st.active_mut().unwrap().undo();
        // The grow changed in its dialog: by 10, both rectangles grown that far; the dialog
        // shows what it makes before it is kept, as a step changed.
        st.edit_selection_step(2, &ctx);
        let m = st.modifying.as_ref().expect("no dialog");
        assert!(m.changes_a_step());
        assert_eq!(m.step, Step::Grow { by: 5.0, corners: Reach::Square });
        st.answer_modifying(Step::Grow { by: 10.0, corners: Reach::Square }, Some(true), &ctx);
        let mut waited = 0;
        while st.modifying.is_some() {
            assert!(waited < 1000, "the dialog never closed");
            st.poll_modifying();
            std::thread::sleep(std::time::Duration::from_millis(5));
            waited += 1;
        }
        assert_eq!(selected(&st), Some(Extent::Within(IRect::new(10, 10, 270, 90))));
        assert_eq!((steps(&st), label(&st).as_deref()), (3, Some(STEP_CHANGED)));
        // The first taken away: the second, added to nothing, grown alone; then that, and the
        // grow of nothing is nothing.
        st.remove_selection_step(0);
        assert_eq!(selected(&st), Some(Extent::Within(IRect::new(190, 10, 270, 90))));
        st.remove_selection_step(0);
        assert_eq!(selected(&st), None);
        // What was selected then, Reselect brings back.
        st.reselect();
        assert_eq!(selected(&st), Some(Extent::Within(IRect::new(190, 10, 270, 90))));
    }

    #[test]
    fn layer_via_copy_names_its_layer_after_a_rename_left_open() {
        use crate::commands::Cmd;
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let doc = st.active_doc.unwrap();
        let d = st.active_mut().unwrap();
        let layer = d.doc().layers[0].id;
        let shape = Shape::rectangle(&IRect::new(10, 10, 20, 20).to_f(), false);
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
        st.renaming = Some((doc, layer, "Sky".into()));
        st.run(Cmd::LayerViaCopy, &eframe::egui::Context::default());
        st.finish_edits();
        let names: Vec<String> = st.active().unwrap().doc().layers.iter().map(|l| l.name.clone()).collect();
        assert_eq!(names, ["Sky", "Sky copy"]);
    }

    #[test]
    fn select_layer_pixels_selecting_nothing_is_refused_and_its_mask_measured() {
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        use opal_core::tile::{Tile, TileCoord};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 600, height: 300, background: Background::White, resolution: 72.0 });
        let shape = Shape::rectangle(&IRect::new(10, 10, 50, 50).to_f(), false);
        let selected = Selection::new(Step::Shape { shape, combine: Combine::Replace });
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(selected.clone()), "Select");
        // An empty layer, and one half transparent away from the rectangle.
        let mut faint = RasterImage::new();
        faint.set_tile(TileCoord::new(2, 0), Some(std::sync::Arc::new(Tile::filled([half::f16::from_f32(0.5); 4]))));
        let mut ids = Vec::new();
        for (name, image) in [("empty", RasterImage::new()), ("faint", faint)] {
            let _ = d.apply("Add", |doc, made| {
                let id = made.layer();
                ids.push(id);
                doc.insert(Position { parent: None, index: 1 }, Layer::new_raster(id, made.object(), name, image))
                    .map_err(|_| ())
            });
        }
        let said = |st: &mut AppState| st.messages.pop_back().map(|m| m.body).unwrap_or_default();
        // Nothing to select: refused, what was selected kept.
        st.select_layer_pixels(ids[0], Combine::Replace);
        assert!(said(&mut st).contains("no pixels to select"));
        assert_eq!(st.active().unwrap().pixel_selection().map(|s| &**s), Some(&selected));
        // Nothing left: refused too.
        st.select_layer_pixels(ids[1], Combine::Intersect);
        let why = said(&mut st);
        assert!(why.contains("Nothing would be left selected"), "{why}");
        // Its levels' memory asked for: not enough, refused.
        let _pretend = crate::memory::testing::pretend_free(Some(64 << 10));
        st.select_layer_pixels(ids[1], Combine::Replace);
        let why = said(&mut st);
        assert!(why.contains("would need"), "{why}");
        assert_eq!(st.active().unwrap().pixel_selection().map(|s| &**s), Some(&selected));
    }

    #[test]
    fn a_step_changed_works_the_steps_before_it_out_once_and_waits_for_a_box() {
        use crate::commands::Cmd;
        use opal_core::mask::{Combine, Shape};
        use opal_core::modify::Reach;
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let ctx = eframe::egui::Context::default();
        let shape = Shape::rectangle(&IRect::new(20, 20, 120, 80).to_f(), false);
        let made = Selection::new(Step::Shape { shape, combine: Combine::Replace })
            .then(Step::Grow { by: 5.0, corners: Reach::Round })
            .then(Step::Grow { by: 3.0, corners: Reach::Round });
        st.active_mut().unwrap().set_pixel_selection(Some(made.clone()), "Select");
        let settle = |st: &mut AppState| {
            let mut waited = 0;
            while st.poll_modifying() {
                assert!(waited < 1000, "the preview never came");
                std::thread::sleep(std::time::Duration::from_millis(5));
                waited += 1;
            }
        };
        // Opened on the last grow, unchanged: the selection as it is, nothing worked out.
        st.edit_selection_step(2, &ctx);
        settle(&mut st);
        let m = st.modifying.as_ref().unwrap();
        assert!(m.prefix.lock().unwrap().is_none(), "worked out though nothing changed");
        // Changed: the steps before it worked out once, kept for the next change.
        st.answer_modifying(Step::Grow { by: 6.0, corners: Reach::Round }, None, &ctx);
        settle(&mut st);
        let kept = st.modifying.as_ref().unwrap().prefix.lock().unwrap().clone().expect("not kept");
        assert_eq!(kept.steps().len(), 2);
        st.answer_modifying(Step::Grow { by: 7.0, corners: Reach::Round }, None, &ctx);
        settle(&mut st);
        let again = st.modifying.as_ref().unwrap().prefix.lock().unwrap().clone().unwrap();
        assert!(std::sync::Arc::ptr_eq(&kept, &again), "worked out again");
        // Changed to nothing, it is no step.
        st.answer_modifying(Step::Grow { by: 0.0, corners: Reach::Round }, Some(true), &ctx);
        while st.modifying.is_some() {
            st.poll_modifying();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(st.active().unwrap().pixel_selection().unwrap().steps().len(), 2);
        // A box open on the image: its steps wait, saying why, and the box stays.
        st.run(Cmd::TransformSelection, &ctx);
        let before = st.active().unwrap().pixel_selection().unwrap().clone();
        st.remove_selection_step(1);
        let d = st.active().unwrap();
        assert!(d.transform.is_some(), "the box was applied");
        assert_eq!(d.pixel_selection().unwrap(), &before);
        assert!(st.messages.back().is_some_and(|m| m.body.contains("Apply or cancel the box first")));
    }

    #[test]
    fn save_selection_saves_what_a_box_open_leaves_selected_and_wants_a_name() {
        use crate::commands::Cmd;
        use opal_core::geom::Affine;
        use opal_core::mask::{Combine, Extent, Shape};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let ctx = eframe::egui::Context::default();
        let shape = Shape::rectangle(&IRect::new(20, 20, 60, 50).to_f(), false);
        let s = Selection::new(Step::Shape { shape, combine: Combine::Replace });
        st.active_mut().unwrap().set_pixel_selection(Some(s), "Select");
        // A box moving it 100 across, open as it is saved: what the box makes is saved.
        st.run(Cmd::TransformSelection, &ctx);
        st.active_mut().unwrap().set_box(Affine::translate(100.0, 0.0), None);
        st.save_selection("Moved");
        let d = st.active().unwrap();
        let saved = d.doc().saved_selection("moved").expect("not saved");
        assert_eq!(saved.kept.selection.mask().extent(), Extent::Within(IRect::new(120, 20, 160, 50)));
        // A blank name, whatever asked: refused, nothing saved.
        st.save_selection("   ");
        assert!(st.messages.back().is_some_and(|m| m.body.contains("Give it a name")));
        assert_eq!(st.active().unwrap().doc().saved.len(), 1);
        // Told apart without the spaces round them.
        st.save_selection("  Moved ");
        assert_eq!(st.active().unwrap().doc().saved.len(), 1, "saved twice under one name");
    }

    #[test]
    fn an_outline_step_combines_as_changed_and_loading_with_nothing_selected_keeps_the_recipe() {
        use opal_core::mask::{Combine, Shape};
        use opal_core::modify::Reach;
        use opal_core::outline::{Outline, Segment};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let rect = Shape::rectangle(&IRect::new(20, 20, 200, 80).to_f(), false);
        let triangle = Outline::new(
            [150.0, 20.0],
            vec![Segment::Line { to: [250.0, 20.0] }, Segment::Line { to: [250.0, 80.0] }],
            false,
        );
        let made = Selection::new(Step::Shape { shape: rect, combine: Combine::Replace })
            .then(Step::Outline { outline: triangle, combine: Combine::Add })
            .then(Step::Grow { by: 2.0, corners: Reach::Square });
        st.active_mut().unwrap().set_pixel_selection(Some(made), "Select");
        // The outline taken away rather than added, as the panel's menu says.
        st.combine_selection_step(1, Combine::Subtract);
        st.finish_edits();
        let steps = st.active().unwrap().pixel_selection().unwrap().steps().to_vec();
        assert!(matches!(steps[1], Step::Outline { combine: Combine::Subtract, .. }), "{steps:?}");
        // Saved, nothing selected, and loaded as an addition: brought back as it was, its
        // recipe and all, not as a mask.
        st.save_selection("Kept");
        st.active_mut().unwrap().set_pixel_selection(None, "Deselect");
        st.load_selection("Kept", Combine::Add, false);
        st.finish_edits();
        let loaded = st.active().unwrap().pixel_selection().expect("nothing loaded").steps().to_vec();
        assert_eq!(loaded, steps, "the recipe was not brought back");
    }

    #[test]
    fn a_shape_reshaped_past_the_memory_free_is_refused_and_said_so() {
        use opal_core::mask::{Combine, Shape, ShapeKind};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let shape = Shape::rectangle(&IRect::new(20, 20, 60, 50).to_f(), false);
        st.active_mut()
            .unwrap()
            .set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
        let before = st.active().unwrap().pixel_selection().cloned();
        // Its box dragged out to a turned ellipse whose edge's tiles would take some 470 MB.
        let huge = Shape { kind: ShapeKind::Ellipse, center: [0.0, 0.0], size: [2e5, 9e4], angle: 30.0, smooth: true };
        let d = st.active_mut().unwrap();
        {
            let _pretend = crate::memory::testing::pretend_free(Some(64 << 20));
            crate::select_tool::reshape(d, &Selection::of(Vec::new()), huge, Combine::Replace, "Resize selection");
        }
        assert!(d.notice.as_ref().is_some_and(|n| n.contains("memory")), "{:?}", d.notice);
        assert_eq!(d.pixel_selection().cloned(), before, "the selection changed");
    }

    #[test]
    fn layer_via_copy_works_out_only_what_the_selection_reaches() {
        use crate::commands::Cmd;
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        use opal_core::tile::{Tile, TileCoord};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        // A layer at half opacity of 64 tiles, each its own: composited, each would be made.
        let mut img = RasterImage::new();
        for y in 0..8 {
            for x in 0..8 {
                let t = Tile::filled([half::f16::from_f32(1.0); 4]);
                img.set_tile(TileCoord::new(x, y), Some(std::sync::Arc::new(t)));
            }
        }
        let d = st.active_mut().unwrap();
        let mut made = None;
        let _ = d.apply("Add", |doc, ids| {
            let id = ids.layer();
            let mut l = Layer::new_raster(id, ids.object(), "tiles", img);
            l.opacity = 0.5;
            made = Some(id);
            doc.insert(Position { parent: None, index: 1 }, l).map_err(|_| ())
        });
        d.select_only(made.unwrap());
        let shape = Shape::rectangle(&IRect::new(10, 10, 20, 20).to_f(), false);
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
        // 4 MB: the 64 tiles (32 MB) would not fit; the one the selection reaches does.
        let _pretend = crate::memory::testing::pretend_free(Some(4 << 20));
        st.run(Cmd::LayerViaCopy, &eframe::egui::Context::default());
        assert!(st.messages.is_empty(), "{:?}", st.messages.front().map(|m| &m.body));
        assert_eq!(st.active().unwrap().doc().layers.len(), 3);
    }

    #[test]
    fn layer_via_copy_and_cut_put_what_is_selected_on_a_layer_of_its_own() {
        use crate::commands::Cmd;
        use opal_core::mask::{Combine, Shape};
        use opal_core::selection::{Selection, Step};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let alpha = |st: &AppState, layer: usize, x: i64, y: i64| {
            let LayerContent::Objects(o) = &st.active().unwrap().doc().layers[layer].content else { panic!("a group") };
            o.first().map_or(0.0, |o| match &o.kind {
                ObjectKind::Pixels(p) => {
                    opal_core::tile::pixel_to_f32(p.image.pixel(x - p.offset[0], y - p.offset[1]))[3]
                }
                ObjectKind::Image(_) => panic!("an image object"),
            })
        };
        // With nothing selected, Layer via Copy duplicates the layers, as Duplicate does.
        st.run(Cmd::LayerViaCopy, &eframe::egui::Context::default());
        assert_eq!(st.active().unwrap().doc().layers.len(), 2);
        st.active_mut().unwrap().undo();
        // A rectangle reaching past the canvas: copied there too, on a layer just above, which
        // becomes the selected one; the original as it was.
        let shape = Shape::rectangle(&IRect::new(250, 50, 400, 150).to_f(), false);
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
        let states = d.history.nodes().count();
        st.run(Cmd::LayerViaCopy, &eframe::egui::Context::default());
        let d = st.active().unwrap();
        assert_eq!(d.history.nodes().count(), states + 1);
        assert_eq!(d.history.undo_label(), Some("Layer via copy"));
        assert_eq!(d.doc().layers.len(), 2);
        assert_eq!(d.selected_layers(), [d.doc().layers[1].id], "the new layer not selected");
        assert_eq!([alpha(&st, 1, 275, 100), alpha(&st, 1, 200, 100), alpha(&st, 0, 275, 100)], [1.0, 0.0, 1.0]);
        // Cut from the background: erased there too.
        st.active_mut().unwrap().undo();
        st.run(Cmd::LayerViaCut, &eframe::egui::Context::default());
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Layer via cut"));
        assert_eq!([alpha(&st, 1, 275, 100), alpha(&st, 0, 275, 100), alpha(&st, 0, 200, 100)], [1.0, 0.0, 1.0]);
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
        st.modify_selection(crate::commands::Cmd::GrowSelection, &ctx);
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
        // Smooth's dialog, as the others: what it makes shown under its own name.
        st.modify_selection(crate::commands::Cmd::SmoothSelection, &ctx);
        settle(&mut st);
        assert_eq!(st.active().unwrap().pending_label(), Some("Smooth selection"));
        st.end_modifying(false);
        assert_eq!(selected(&st), Extent::Within(IRect::new(100, 50, 200, 150)));
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
        // OK while what it makes is worked out (a job taken as under way here, and asked for
        // after): the dialog waits, saying so, then closes, keeping it, once it is shown.
        st.modify_selection(crate::commands::Cmd::GrowSelection, &ctx);
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
        st.modify_selection(crate::commands::Cmd::GrowSelection, &ctx);
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
        st.modify_selection(crate::commands::Cmd::GrowSelection, &ctx);
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
        st.modify_selection(crate::commands::Cmd::GrowSelection, &ctx);
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
        st.modify_selection(crate::commands::Cmd::GrowSelection, &ctx);
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
        st.modify_selection(crate::commands::Cmd::FeatherSelection, &ctx);
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
