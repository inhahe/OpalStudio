//! Saving and exporting, off the interface thread.

use crate::background::JobThread;
use crate::doc::DocId;
use eframe::egui;
use opal_core::history::NodeId;
use opal_io::Progress;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

/// What a write puts where.
#[derive(Clone, Debug)]
pub enum Written {
    /// Document `doc`, as it was at history state `at`, saved to `path`; `before` is the file
    /// it had before, and its title then (put back if the save fails).
    Save {
        doc: DocId,
        at: NodeId,
        path: PathBuf,
        before: (Option<PathBuf>, String),
    },
    Export {
        path: PathBuf,
    },
}

/// What to do once a write has succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    Nothing,
    /// Close the document saved (asking again if it changed meanwhile).
    Close,
    /// Answer the API call of this number (see [`AppState::answers`]), with what became of
    /// the write.
    ///
    /// [`AppState::answers`]: crate::state::AppState::answers
    Answer(u64),
}

/// The work of a write: writes the file, telling the progress how far it has got and
/// stopping once it is cancelled, or says why it could not.
pub type Job = Box<dyn FnOnce(&Progress) -> Result<(), String> + Send>;

/// Why a write was not done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// It was cancelled (it failed after being asked to stop, if only because it was).
    Cancelled,
    /// It failed, for this reason.
    Error(String),
}

/// A write done.
pub struct Finished {
    pub written: Written,
    pub then: Then,
    pub result: Result<(), Failure>,
}

/// A write asked for and not yet reported.
struct Pending {
    id: u64,
    written: Written,
    then: Then,
    progress: Arc<Progress>,
}

/// Writes files one after another on a thread of its own, in the order asked (so two writes
/// to one file never overlap), and reports each as it is done. A document is written as it
/// was when the write was asked for, so it can be edited meanwhile.
pub struct Writer {
    thread: JobThread,
    done_tx: mpsc::Sender<(u64, Result<(), Failure>)>,
    done: mpsc::Receiver<(u64, Result<(), Failure>)>,
    /// The writes asked for and not yet reported, in order.
    pending: Vec<Pending>,
    next: u64,
}

impl Default for Writer {
    fn default() -> Writer {
        let (done_tx, done) = mpsc::channel();
        Writer { thread: JobThread::new("writer"), done_tx, done, pending: Vec::new(), next: 0 }
    }
}

impl Writer {
    /// Starts writing `written` with `job`; `then` is done once it succeeds, and `ctx` is
    /// asked to repaint when it is done (see [`finished`](Self::finished)).
    pub fn write(&mut self, written: Written, then: Then, job: Job, ctx: &egui::Context) {
        self.next += 1;
        let id = self.next;
        let progress = Arc::new(Progress::default());
        self.pending.push(Pending { id, written, then, progress: progress.clone() });
        let done_tx = self.done_tx.clone();
        let work = {
            let progress = progress.clone();
            move || job(&progress)
        };
        let done = move |result: std::thread::Result<Result<(), String>>| {
            let result = match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(_)) if progress.is_cancelled() => Err(Failure::Cancelled),
                Ok(Err(e)) => Err(Failure::Error(e)),
                Err(_) => Err(Failure::Error(
                    "Something went wrong inside Opal Studio while writing the file; the file was left as \
                     it was."
                        .into(),
                )),
            };
            let _ = done_tx.send((id, result));
        };
        self.thread.run(work, done, ctx);
    }

    /// The writes done since the last call, in the order they were asked for.
    pub fn finished(&mut self) -> Vec<Finished> {
        let mut out = Vec::new();
        while let Ok((id, result)) = self.done.try_recv() {
            let at = self.pending.iter().position(|p| p.id == id).expect("a write asked for");
            let Pending { written, then, .. } = self.pending.remove(at);
            out.push(Finished { written, then, result });
        }
        out
    }

    /// The writes asked for and not yet reported, in order (the first is being written),
    /// with how far each has got.
    pub fn pending(&self) -> impl Iterator<Item = (&Written, &Progress)> {
        self.pending.iter().map(|p| (&p.written, &*p.progress))
    }

    /// Whether document `doc` is to be closed once a save still under way is written.
    pub fn closing(&self, doc: DocId) -> bool {
        self.pending
            .iter()
            .any(|p| p.then == Then::Close && matches!(&p.written, Written::Save { doc: d, .. } if *d == doc))
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A save of `doc` to `failed` has failed: the saves still to be done that would give it
    /// back that file if they failed give back `before` instead (what it had before it).
    pub fn pass_on(&mut self, doc: DocId, failed: &PathBuf, before: &(Option<PathBuf>, String)) {
        pass_on(self.pending.iter_mut().map(|p| &mut p.written), doc, failed, before);
    }
}

/// [`Writer::pass_on`] for `writes`.
pub fn pass_on<'a>(
    writes: impl Iterator<Item = &'a mut Written>,
    doc: DocId,
    failed: &PathBuf,
    before: &(Option<PathBuf>, String),
) {
    for w in writes {
        if let Written::Save { doc: d, before: b, .. } = w
            && *d == doc
            && b.0.as_ref() == Some(failed)
        {
            *b = before.clone();
        }
    }
}
