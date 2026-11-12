//! Edits that may take long (design.md, *Edits that take long*): merging, flattening,
//! rasterizing, turning, flipping and resizing the image, and a script's grow or feather.
//! Each is worked out off the interface thread from the image as it was, which is not to be
//! changed meanwhile (`DocState::busy`: its commands say why they are unavailable), and is
//! applied as one history state once done. Other images, and the busy one's view, stay free.

use crate::doc::DocId;
use crate::state::{AppState, MessageKind};
use opal_core::document::{Document, IdGen};
use opal_core::history::NodeId;
use opal_io::Progress;
use serde_json::Value;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The threads edits' parallel parts run on: a pool of their own, so that an edit neither
/// waits behind the background pool's work (the images' views, saves and exports, answers
/// made for scripts) nor keeps it waiting.
pub fn pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map_or(2, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("edit {i}"))
            .build()
            .expect("starting the edits' threads")
    })
}

/// How long an edit runs before it is shown under way (one done sooner shows nothing).
pub const SHOWN_AFTER: Duration = Duration::from_millis(300);

/// How long the interface thread waits for an edit as it begins (`AppState::edit_wait`): one
/// done by then is applied at once, as if it had been worked out there (the image never busy,
/// a script's call answered at once). Tests wait for each to the end (a minute at most), so
/// that what they look at next does not depend on how fast it was, unless they say not to.
pub const WAITED_FOR: Duration = if cfg!(test) { Duration::from_secs(60) } else { Duration::from_millis(60) };

/// What an edit's work gives when it stops because it was cancelled (never shown: a cancelled
/// edit is dropped as it is cancelled).
pub const CANCELLED: &str = "Cancelled.";

/// What is done on the interface thread once an edit is applied, given the image.
type Then = Box<dyn FnOnce(&mut AppState, DocId) + Send>;

/// What an edit's work made: the document, the identifiers it gave out, and what is done
/// once it is applied; or why it could not be made.
type Made = Result<(Document, IdGen, Then), String>;

/// What a script's call is answered with once its edit is applied, given the image it was of.
pub type AnswerWith = Box<dyn FnOnce(&AppState, DocId) -> Value>;

/// A script's call answered once its edit is applied: its number, the call (its name, for an
/// error; whether it is answered at all; the user's view as it found it, put back if the edit
/// fails and nothing has changed it since), and what it is answered with (given the image
/// the edit was of).
pub struct Answer {
    pub n: u64,
    pub call: crate::api::Calling,
    pub with: AnswerWith,
}

/// An edit under way on an image.
pub struct Busy {
    /// What it does, as the history state it makes is named.
    pub label: String,
    /// How far it has got (when its work says), and where it is asked to stop.
    pub progress: Arc<Progress>,
    pub started: Instant,
    /// The history state it is worked out from, which the state it makes follows.
    from: NodeId,
    /// The groups (script runs) the call that began it was in.
    groups: Vec<u64>,
    /// Where its work tells what came of it.
    done: mpsc::Receiver<std::thread::Result<Made>>,
    /// The script's call to answer once it is applied, if one began it.
    pub answer: Option<Answer>,
}

impl Busy {
    /// Whether it has run long enough to be shown under way.
    pub fn shown(&self) -> bool {
        self.started.elapsed() >= SHOWN_AFTER
    }

    /// Why the image cannot be changed while it runs.
    pub fn why(&self) -> String {
        format!("{} is under way: wait for it to finish, or cancel it in the status bar.", self.label)
    }
}

/// Stops (with [`CANCELLED`]) if the edit `progress` is of was cancelled: asked by its work
/// between the pieces of it.
pub fn go_on(progress: &Progress) -> Result<(), String> {
    if progress.is_cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
}

/// `layers` composited within `range` (everywhere when `None`: see
/// [`composite_until`](opal_core::composite::composite_until)), telling `progress` how far it
/// has got, and stopping if the edit was cancelled.
pub fn composited(
    layers: &[Arc<opal_core::document::Layer>],
    range: Option<&opal_core::geom::IRect>,
    progress: &Progress,
) -> Result<opal_core::raster::RasterImage, String> {
    let (mut told, mut began) = (0, false);
    opal_core::composite::composite_until(layers, range, &mut |placed, total| {
        if !began && total > 0 {
            progress.begin(total as u64);
            began = true;
        }
        progress.advance((placed - told) as u64);
        told = placed;
        !progress.is_cancelled()
    })
    .ok_or_else(|| CANCELLED.to_owned())
}

impl AppState {
    /// Starts an edit of image `doc` named `label`, as one history state: `work` is given the
    /// document as it is (an edit in progress committed first), the image's identifiers to
    /// give out, and the edit's progress, off the interface thread, its parallel parts on the
    /// edits' own threads ([`pool`]); `memory`, set aside for it already, is kept while it
    /// runs. A rename left open in the image is finished first, as an edit in progress is. Once it
    /// is done, what it made is the image's next state and `then` is given what else it gave;
    /// if it fails, the user is told why (or the script whose call began it). The image is
    /// busy meanwhile, unless it is done within [`edit_wait`](AppState::edit_wait).
    pub(crate) fn edit_later<R: Send + 'static>(
        &mut self,
        doc: DocId,
        label: &str,
        memory: crate::memory::Allowance,
        work: impl FnOnce(&mut Document, &mut IdGen, &Progress) -> Result<R, String> + Send + 'static,
        then: impl FnOnce(&mut AppState, DocId, R) + Send + 'static,
    ) {
        let repaint = self.repaint.clone();
        // The user's own, not part of the script's call that may be carrying the edit out.
        if self.renaming.as_ref().is_some_and(|r| r.0 == doc) {
            let groups = self.doc_mut(doc).map(|d| std::mem::take(&mut d.groups)).unwrap_or_default();
            self.finish_rename(true);
            if let Some(d) = self.doc_mut(doc) {
                d.groups = groups;
            }
        }
        let Some(d) = self.doc_mut(doc) else { return };
        assert!(d.busy.is_none(), "an edit begun on an image busy with another");
        d.commit_pending();
        let (mut made, mut ids) = (d.history.doc().clone(), d.ids.clone());
        let progress = Arc::new(Progress::default());
        let (tx, done) = mpsc::channel();
        let theirs = progress.clone();
        let started = std::thread::Builder::new().name("edit".into()).spawn(move || {
            let _memory = memory;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let out = pool().install(|| work(&mut made, &mut ids, &theirs))?;
                Ok((made, ids, Box::new(move |st: &mut AppState, doc| then(st, doc, out)) as Then))
            }));
            // Not wanted any more if it was cancelled: then no one is listening.
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        if let Err(e) = started {
            return self.message(MessageKind::Error, label, format!("It could not be begun: {e}."));
        }
        let d = self.doc_mut(doc).expect("found above");
        d.busy = Some(Busy {
            label: label.to_owned(),
            progress,
            started: Instant::now(),
            from: d.history.current(),
            groups: d.groups.clone(),
            done,
            answer: None,
        });
        d.revision += 1;
        self.end_edit(doc, self.edit_wait);
    }

    /// Applies the edits that have finished, or tells why they could not be (to the user, or
    /// to the script whose call began one); while any runs, asks for a frame when the first
    /// is to be shown under way.
    pub fn poll_edits(&mut self) {
        let busy: Vec<DocId> = self.docs.iter().filter(|d| d.busy.is_some()).map(|d| d.id).collect();
        for id in busy {
            self.end_edit(id, Duration::ZERO);
        }
    }

    /// Applies the edit under way on image `doc` if it is done, or once it is if that is
    /// within `wait` (see [`poll_edits`](Self::poll_edits)).
    fn end_edit(&mut self, id: DocId, wait: Duration) {
        let repaint = self.repaint.clone();
        let Some(d) = self.doc_mut(id) else { return };
        let Some(b) = d.busy.as_ref() else { return };
        let outcome = match b.done.recv_timeout(wait) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !b.shown() {
                    repaint.request_repaint_after(SHOWN_AFTER.saturating_sub(b.started.elapsed()));
                }
                return;
            }
            // Its thread always sends before it ends.
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Box::new("its thread ended without a word") as _),
        };
        let b = d.busy.take().expect("looked at above");
        d.revision += 1;
        let changed = d.history.current() != b.from;
        let (told, notice) = (self.messages.len(), self.notice.clone());
        // What its work refused is told as a refusal; a panic as an error.
        let kind = if outcome.is_err() { MessageKind::Error } else { MessageKind::Info };
        let result = match outcome {
            // The image cannot change while it is busy; if it did, the user's change stays.
            Ok(Ok(_)) if changed => {
                Err("The image was changed while this was being worked out, so it was not applied.".to_owned())
            }
            Ok(Ok((made, ids, then))) => {
                // One that changes nothing makes no state.
                let d = self.doc_mut(id).expect("listed");
                if !made.same_as(d.history.doc()) {
                    d.commit_made(made, ids, &b.label, &b.groups);
                }
                then(self, id);
                Ok(())
            }
            Ok(Err(why)) => Err(why),
            Err(panic) => Err(format!("This went wrong: {}.", crate::background::panic_message(&*panic))),
        };
        match (b.answer, result) {
            // A notification, which nobody hears the answer to: what it says goes to the user.
            // Answered still: its connection waits for that before it reads its next request.
            (Some(a), Err(why)) if !a.call.heard => {
                self.message(kind, a.call.name, why.clone());
                self.answered.push((a.n, Err(why)));
            }
            // A notification's answer nobody reads: what was told stays for the user.
            (Some(a), Ok(())) if !a.call.heard => {
                let value = (a.with)(self, id);
                self.answered.push((a.n, Ok(value)));
            }
            (Some(a), Ok(())) => {
                let value = (a.with)(self, id);
                let (refusals, notes) = self.told_since(told, notice);
                let answer = if refusals.is_empty() {
                    Ok(crate::api::with_notes(value, notes))
                } else {
                    Err(refusals.join("\n"))
                };
                self.answered.push((a.n, answer));
            }
            (Some(a), Err(why)) => {
                self.answered.push((a.n, Err(format!("{}: {why}", a.call.name))));
                // As a call refused at once has it put back, unless the user has moved on: the
                // image, and the layers the call chose in the one it edited (which, busy, the
                // user could not choose meanwhile), not those chosen in others since.
                if let Some((back, layers)) = a.call.view.filter(|_| self.active_doc == Some(id)) {
                    if let Some(back) = back {
                        self.back_to(back);
                    }
                    if let Some((_, state)) = layers.into_iter().find(|(d, _)| *d == id)
                        && let Some(d) = self.doc_mut(id)
                    {
                        d.set_layer_selection_state(state);
                    }
                }
            }
            // Told under the name of the call carrying it out, if one is (as its refusal).
            (None, Err(why)) => {
                let title = self.calling.as_ref().map_or(b.label, |c| c.name.clone());
                self.message(kind, title, why);
            }
            (None, Ok(())) => {}
        }
    }

    /// Cancels the edit under way on image `doc`, if there is one: the image is left as it
    /// was, free at once (the work stops as soon as it next looks).
    pub fn cancel_edit(&mut self, doc: DocId) {
        if let Some(label) = self.give_up_edit(doc, "cancelled") {
            self.notify(format!("{label} was cancelled; the image is as it was."));
        }
    }

    /// Stops the edit under way on image `doc`, if there is one, and tells the script whose
    /// call began it that it was `why` (cancelled, or closed); what it was, if there was one.
    pub(crate) fn give_up_edit(&mut self, doc: DocId, why: &str) -> Option<String> {
        let d = self.doc_mut(doc)?;
        let b = d.busy.take()?;
        b.progress.cancel();
        d.revision += 1;
        if let Some(a) = b.answer {
            self.answered.push((a.n, Err(format!("{}: {why}.", a.call.name))));
        }
        Some(b.label)
    }

    /// Whether an edit is under way on any image.
    pub fn editing(&self) -> bool {
        self.docs.iter().any(|d| d.busy.is_some())
    }

    /// Waits until no edit is under way, applying each as it ends (for tests, and for what
    /// must wait for them).
    #[cfg(test)]
    pub fn finish_edits(&mut self) {
        let mut waited = 0;
        while self.editing() {
            assert!(waited < 6000, "an edit never ended");
            self.poll_edits();
            std::thread::sleep(Duration::from_millis(5));
            waited += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Cmd, availability};
    use crate::state::{Background, NewImage};
    use eframe::egui;
    use opal_core::document::{Document, IdGen};

    fn app() -> AppState {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        st
    }

    /// An edit that waits for `go` before it renames the first layer `name`, and then sends
    /// what `then` is given back on `told`.
    fn waiting_edit(st: &mut AppState, name: &'static str, go: mpsc::Receiver<()>, told: mpsc::Sender<u32>) {
        let doc = st.active_doc.unwrap();
        let memory = crate::memory::Allowance::now();
        st.edit_later(
            doc,
            "Rename it",
            memory,
            move |doc, _, progress| {
                let _ = go.recv();
                if progress.is_cancelled() {
                    return Err(CANCELLED.into());
                }
                Arc::make_mut(&mut doc.layers[0]).name = name.into();
                Ok(7)
            },
            move |_, _, n| told.send(n).unwrap(),
        );
    }

    #[test]
    fn an_edit_is_worked_out_off_the_interface_thread_its_image_kept_as_it_is_meanwhile() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let doc = st.active_doc.unwrap();
        let states = st.active().unwrap().history.nodes().count();
        let (go, wait) = mpsc::channel();
        let (told, heard) = mpsc::channel();
        waiting_edit(&mut st, "Renamed", wait, told);
        // Meanwhile the image is as it was, and nothing may change it: commands say why;
        // looking at it, and other images, are free.
        st.poll_edits();
        assert!(st.editing() && st.active().unwrap().doc().layers[0].name != "Renamed");
        let why = availability(&st, Cmd::NewLayer).unwrap_err();
        assert!(why.starts_with("Rename it is under way"), "{why}");
        assert!(availability(&st, Cmd::Undo).is_err());
        assert!(availability(&st, Cmd::ZoomIn).is_ok() && availability(&st, Cmd::Copy).is_ok());
        assert!(st.scripts_wait(false), "scripts' calls do not wait for it");
        st.create_image(&NewImage { width: 10, height: 10, background: Background::White, resolution: 72.0 });
        assert!(availability(&st, Cmd::NewLayer).is_ok(), "another image is not free");
        st.show_doc(doc);
        // Done: one state of its own, named for it, and what was to follow, done.
        go.send(()).unwrap();
        st.finish_edits();
        let d = st.active().unwrap();
        assert_eq!(d.doc().layers[0].name, "Renamed");
        assert_eq!(d.history.nodes().count(), states + 1);
        assert_eq!(d.history.undo_label(), Some("Rename it"));
        assert_eq!(heard.try_recv(), Ok(7));
        assert!(availability(&st, Cmd::NewLayer).is_ok());
    }

    #[test]
    fn an_edit_is_given_up_when_its_image_closes_not_when_closing_it_is_asked() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let doc = st.active_doc.unwrap();
        // Changes to save, so that closing asks.
        st.run(Cmd::NewLayer, &egui::Context::default());
        let (_go, wait) = mpsc::channel();
        let (told, _heard) = mpsc::channel();
        waiting_edit(&mut st, "Renamed", wait, told);
        st.request_close(doc);
        assert!(matches!(st.confirm, Some(crate::state::Confirm::Close(_))));
        // Answered Cancel: the image stays, and so does its edit.
        st.confirm = None;
        assert!(st.editing(), "asking gave the edit up");
        st.close_now(doc);
        assert!(!st.editing() && st.doc(doc).is_none());
    }

    #[test]
    fn an_edit_runs_on_threads_of_its_own_while_the_background_ones_are_busy() {
        use rayon::prelude::*;
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let held = crate::background::testing::hold(Some(crate::background::pool()));
        let doc = st.active_doc.unwrap();
        let work = |doc: &mut Document, _: &mut IdGen, _: &Progress| {
            let sum: u64 = (0..100_000u64).into_par_iter().sum();
            Arc::make_mut(&mut doc.layers[0]).name = format!("{sum}");
            Ok(())
        };
        st.edit_later(doc, "Sum", crate::memory::Allowance::now(), work, |_, _, ()| {});
        let mut waited = 0;
        while st.editing() && waited < 400 {
            st.poll_edits();
            std::thread::sleep(Duration::from_millis(5));
            waited += 1;
        }
        let ended = !st.editing();
        drop(held);
        st.finish_edits();
        assert!(ended, "the edit waited for the background threads");
        assert_eq!(st.active().unwrap().doc().layers[0].name, "4999950000");
    }

    #[test]
    fn a_busy_image_refuses_what_would_change_it_and_a_rename_open_is_finished_first() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let doc = st.active_doc.unwrap();
        let layer = st.active().unwrap().doc().layers[0].id;
        st.renaming = Some((doc, layer, "Named first".into()));
        let (go, wait) = mpsc::channel();
        let (told, _heard) = mpsc::channel();
        waiting_edit(&mut st, "Renamed", wait, told);
        assert!(st.renaming.is_none(), "the rename stayed open");
        assert_eq!(st.active().unwrap().doc().layers[0].name, "Named first");
        // While it runs: a change not made, the status bar saying why; the state before it not
        // taken back, nor undone.
        let d = st.active_mut().unwrap();
        let states = d.history.nodes().count();
        let refused = d.apply::<()>("Anything", |doc, _| {
            doc.canvas.width = 1;
            Ok(())
        });
        assert_eq!(refused, Err(crate::doc::NotMade::Busy));
        assert_eq!(d.history.nodes().count(), states, "a change was made");
        assert!(d.notice.as_ref().is_some_and(|n| n.contains("under way")), "{:?}", d.notice);
        let current = d.history.current();
        let path = d.history.path_from_root(current);
        let from = path[path.len() - 2];
        assert!(!d.take_back(current, from), "taken back while busy");
        assert!(!d.undo(), "undone while busy");
        go.send(()).unwrap();
        st.finish_edits();
        assert_eq!(st.active().unwrap().doc().layers[0].name, "Renamed");
    }

    #[test]
    fn a_rename_finished_as_a_script_s_edit_begins_is_not_the_script_s() {
        let mut st = app();
        let doc = st.active_doc.unwrap();
        let layer = st.active().unwrap().doc().layers[0].id;
        st.renaming = Some((doc, layer, "Named first".into()));
        // The script's call (group 7) carries out an edit, which finishes the rename first.
        st.active_mut().unwrap().groups = vec![7];
        let (go, wait) = mpsc::channel();
        let (told, _heard) = mpsc::channel();
        go.send(()).unwrap();
        waiting_edit(&mut st, "Renamed", wait, told);
        st.finish_edits();
        let d = st.active_mut().unwrap();
        d.groups.clear();
        d.group_ended(7, "Script: go.py", false);
        // Undone, the script's state goes; the user's rename stays.
        assert_eq!(d.history.undo_label(), Some("Script: go.py"));
        d.undo();
        assert_eq!(d.doc().layers[0].name, "Named first");
    }

    #[test]
    fn a_paste_read_at_once_waits_for_the_edit_under_way_on_its_image() {
        use crate::clipboard::{Pasted, TestClipboard};
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let px = vec![opal_core::tile::pixel_from_f32([1.0; 4]); 4];
        let image = opal_core::raster::RasterImage::from_pixels(2, 2, &px);
        let pasted = Pasted { width: 2, height: 2, image, from: None, resolution: None, warnings: Vec::new() };
        st.clipboard = Box::new(TestClipboard { image: Some(pasted), ..Default::default() });
        let (go, wait) = mpsc::channel();
        let (told, _heard) = mpsc::channel();
        waiting_edit(&mut st, "Renamed", wait, told);
        st.paste(false, &egui::Context::default());
        assert!(st.active().unwrap().transform.is_none(), "a box opened on the busy image");
        assert!(st.notice.as_ref().is_some_and(|(n, _)| n.contains("edit under way")), "{:?}", st.notice);
        go.send(()).unwrap();
        st.finish_edits();
        st.poll_paste(false);
        let d = st.active().unwrap();
        assert!(d.transform.is_some(), "the paste was not placed once the edit was done");
        assert_eq!(d.doc().layers[0].name, "Renamed", "the edit was lost");
    }

    #[test]
    fn a_rename_left_open_as_a_merge_begins_is_kept_by_it() {
        let mut st = app();
        let ctx = egui::Context::default();
        st.run(Cmd::NewGroup, &ctx);
        let doc = st.active_doc.unwrap();
        let group = st.active().unwrap().active_layer().unwrap();
        st.renaming = Some((doc, group, "Named first".into()));
        st.run(Cmd::MergeGroup, &ctx);
        st.finish_edits();
        let d = st.active().unwrap();
        assert_eq!(d.history.undo_label(), Some("Merge group"));
        assert_eq!(d.doc().layer(group).unwrap().name, "Named first");
    }

    #[test]
    fn a_cancelled_edit_frees_its_image_at_once_and_a_failed_one_says_why() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let states = st.active().unwrap().history.nodes().count();
        let (go, wait) = mpsc::channel();
        let (told, heard) = mpsc::channel();
        waiting_edit(&mut st, "Renamed", wait, told);
        st.cancel_edit(st.active_doc.unwrap());
        assert!(!st.editing(), "still busy once cancelled");
        assert!(st.notice.as_ref().is_some_and(|(n, _)| n.starts_with("Rename it was cancelled")));
        go.send(()).unwrap();
        // What it made, if it made anything, is never applied.
        std::thread::sleep(Duration::from_millis(50));
        st.poll_edits();
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
        assert!(heard.try_recv().is_err());
        // One that fails: the image as it was, and the user told.
        let doc = st.active_doc.unwrap();
        let memory = crate::memory::Allowance::now();
        st.edit_later(doc, "Fail", memory, |_, _, _| Err::<(), _>("Not today.".into()), |_, _, ()| {});
        st.finish_edits();
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
        assert_eq!(st.messages.back().map(|m| m.body.as_str()), Some("Not today."));
        st.messages.clear();
        // One that panics, the same.
        let memory = crate::memory::Allowance::now();
        st.edit_later(doc, "Panic", memory, |_, _, _| -> Result<(), String> { panic!("broken") }, |_, _, ()| {});
        st.finish_edits();
        assert!(st.messages.back().is_some_and(|m| m.body.contains("broken")));
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
    }

    #[test]
    fn a_notification_s_edit_that_fails_is_told_to_the_user_and_its_connection_answered() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let doc = st.active_doc.unwrap();
        let memory = crate::memory::Allowance::now();
        st.edit_later(doc, "Refuse", memory, |_, _, _| Err::<u32, String>("Not this time".into()), |_, _, _| {});
        let call = crate::api::Calling { name: "refuse".into(), heard: false, view: None };
        let answer = Answer { n: 9, call, with: Box::new(|_, _| serde_json::json!({})) };
        st.doc_mut(doc).unwrap().busy.as_mut().unwrap().answer = Some(answer);
        st.finish_edits();
        assert!(st.messages.iter().any(|m| m.body.contains("Not this time")), "not told");
        assert!(
            matches!(st.answered.last(), Some((9, Err(why))) if why.contains("Not this time")),
            "{:?}",
            st.answered
        );
    }

    #[test]
    fn a_notification_s_edit_done_leaves_what_it_told_for_the_user() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let doc = st.active_doc.unwrap();
        let memory = crate::memory::Allowance::now();
        st.edit_later(
            doc,
            "Tell",
            memory,
            |_, _, _| Ok::<u32, String>(1),
            |st, _, _| {
                st.message(crate::state::MessageKind::Warning, "Tell", "Worth knowing".to_owned());
            },
        );
        let call = crate::api::Calling { name: "tell".into(), heard: false, view: None };
        let answer = Answer { n: 5, call, with: Box::new(|_, _| serde_json::json!({})) };
        st.doc_mut(doc).unwrap().busy.as_mut().unwrap().answer = Some(answer);
        st.finish_edits();
        assert!(matches!(st.answered.last(), Some((5, Ok(_)))), "{:?}", st.answered);
        assert!(st.messages.iter().any(|m| m.body == "Worth knowing"), "taken into an answer no one reads");
    }

    #[test]
    fn a_refused_edit_puts_back_the_image_not_the_layers_chosen_meanwhile() {
        let mut st = app();
        st.edit_wait = Duration::ZERO;
        let first = st.active_doc.unwrap();
        // A second image of two layers, its upper one chosen when the call was made.
        st.create_image(&NewImage { width: 10, height: 10, background: Background::White, resolution: 72.0 });
        let other = st.active_doc.unwrap();
        st.run(Cmd::NewLayer, &egui::Context::default());
        let (lower, upper) = {
            let d = st.doc(other).unwrap();
            (d.doc().layers[0].id, d.doc().layers[1].id)
        };
        assert_eq!(st.doc(other).unwrap().active_layer(), Some(upper));
        st.show_doc(first);
        // The first image of two layers too, its upper chosen; the call chooses its lower.
        st.run(Cmd::NewLayer, &egui::Context::default());
        let (first_lower, first_upper) = {
            let d = st.doc(first).unwrap();
            (d.doc().layers[0].id, d.doc().layers[1].id)
        };
        let view = (
            Some(first),
            vec![
                (first, st.doc(first).unwrap().layer_selection_state()),
                (other, st.doc(other).unwrap().layer_selection_state()),
            ],
        );
        st.doc_mut(first).unwrap().select_only(first_lower);
        // The call's edit on the first image; the user, meanwhile, chooses the other's lower layer.
        st.edit_later(
            first,
            "Refuse",
            crate::memory::Allowance::now(),
            |_, _, _| Err::<u32, String>("No".into()),
            |_, _, _| {},
        );
        let call = crate::api::Calling { name: "refuse".into(), heard: true, view: Some(view) };
        st.doc_mut(first).unwrap().busy.as_mut().unwrap().answer =
            Some(Answer { n: 4, call, with: Box::new(|_, _| serde_json::json!({})) });
        st.doc_mut(other).unwrap().select_only(lower);
        st.finish_edits();
        assert_eq!(st.active_doc, Some(first));
        assert_eq!(st.doc(other).unwrap().active_layer(), Some(lower), "the layer chosen meanwhile undone");
        assert_eq!(st.doc(first).unwrap().active_layer(), Some(first_upper), "the call's choice left");
    }
}
