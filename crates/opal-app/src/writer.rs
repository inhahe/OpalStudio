//! Saving and exporting, off the interface thread.

use crate::background::JobThread;
use crate::doc::DocId;
use eframe::egui;
use opal_core::history::NodeId;
use std::path::PathBuf;
use std::sync::mpsc;

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
}

/// The work of a write: writes the file, or says why it could not.
pub type Job = Box<dyn FnOnce() -> Result<(), String> + Send>;

/// A write done.
pub struct Finished {
    pub written: Written,
    pub then: Then,
    pub result: Result<(), String>,
}

/// Writes files one after another on a thread of its own, in the order asked (so two writes
/// to one file never overlap), and reports each as it is done. A document is written as it
/// was when the write was asked for, so it can be edited meanwhile.
pub struct Writer {
    thread: JobThread,
    done_tx: mpsc::Sender<(u64, Result<(), String>)>,
    done: mpsc::Receiver<(u64, Result<(), String>)>,
    /// The writes asked for and not yet reported, in order.
    pending: Vec<(u64, Written, Then)>,
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
        self.pending.push((id, written, then));
        let done_tx = self.done_tx.clone();
        let done = move |result: std::thread::Result<Result<(), String>>| {
            let result = result.unwrap_or_else(|_| {
                Err("Something went wrong inside Opal Studio while writing the file; the file was left \
                     as it was."
                    .into())
            });
            let _ = done_tx.send((id, result));
        };
        self.thread.run(job, done, ctx);
    }

    /// The writes done since the last call, in the order they were asked for.
    pub fn finished(&mut self) -> Vec<Finished> {
        let mut out = Vec::new();
        while let Ok((id, result)) = self.done.try_recv() {
            let at = self.pending.iter().position(|p| p.0 == id).expect("a write asked for");
            let (_, written, then) = self.pending.remove(at);
            out.push(Finished { written, then, result });
        }
        out
    }

    /// The writes asked for and not yet reported, in order (the first is being written).
    pub fn pending(&self) -> impl Iterator<Item = &Written> {
        self.pending.iter().map(|p| &p.1)
    }

    /// Whether document `doc` is to be closed once a save still under way is written.
    pub fn closing(&self, doc: DocId) -> bool {
        self.pending
            .iter()
            .any(|(_, w, then)| *then == Then::Close && matches!(w, Written::Save { doc: d, .. } if *d == doc))
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}
