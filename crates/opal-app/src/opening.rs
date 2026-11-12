//! Reading image files on a worker thread, for Open, Place and Open as Layers and scripts'
//! calls of them, so that the window keeps answering while a large file is decoded, as a
//! paste's clipboard is read (`paste`). What was read is used on the interface thread once
//! the user is not in the middle of something it would end.

use crate::doc::DocId;
use crate::state::AppState;
use eframe::egui;
use opal_io::{image_files, native};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// A file as read: an Opal Studio file's document, or an image file's pixels.
pub enum ReadFile {
    Native(Box<native::Loaded>),
    Image(Box<image_files::Imported>),
}

/// Each file read, by its path: what it held, or why it could not be read.
pub type Read = Vec<(PathBuf, Result<ReadFile, String>)>;

/// What files are read for: done with what was read, on the interface thread; for a script's
/// call, what it answers.
pub type Then = Box<dyn FnOnce(&mut AppState, Read) -> Result<Value, String>>;

/// Reads the file at `path` as Open reads it: an Opal Studio file by its extension, else an
/// image file by what it holds. Why not, if it cannot be.
pub fn read_file(path: &Path) -> Result<ReadFile, String> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("opal")) {
        native::load(path, crate::memory::free()).map(|l| ReadFile::Native(Box::new(l))).map_err(|e| e.to_string())
    } else {
        image_files::import(path, crate::memory::free())
            .map(|i| ReadFile::Image(Box::new(i)))
            .map_err(|e| e.to_string())
    }
}

/// Reads each of `paths` (see [`read_file`]).
pub fn read_files(paths: &[PathBuf]) -> Read {
    read_files_with(paths, read_file)
}

/// Reads each of `paths` with `read`: one whose reading fails unexpectedly (a decoder's bug
/// that panics) said to have, the others read still.
fn read_files_with(paths: &[PathBuf], read: impl Fn(&Path) -> Result<ReadFile, String>) -> Read {
    paths
        .iter()
        .map(|p| {
            let file = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(p))).unwrap_or_else(|panic| {
                Err(format!(
                    "Reading {} went wrong: {}.",
                    crate::state::file_title(p),
                    crate::background::panic_message(&*panic)
                ))
            });
            (p.clone(), file)
        })
        .collect()
}

/// Files being read on a worker thread, and what is then done with them.
pub struct FileReading {
    rx: mpsc::Receiver<Read>,
    /// The files being read.
    paths: Vec<PathBuf>,
    /// What reading gave, once it has come back, while what it is for waits.
    result: Option<Read>,
    /// The image what was read goes into, if any, as it was when this was asked for.
    target: crate::state::Target,
    /// What it is called in the status bar ("photo.jpg", "3 files").
    what: String,
    /// The status bar's notice it put up, taken down when it is done.
    notice: String,
    then: Then,
    /// The API call it answers, and the call's groups (a script run, a step), which a
    /// history state it makes in `target` joins.
    answer: Option<(u64, Vec<u64>)>,
    /// The call being carried out when it was asked for, if one was: a notification's
    /// failure, which no one hears, goes to the user.
    call: Option<crate::api::Calling>,
}

impl AppState {
    /// Reads `paths` on a worker thread, then does `then` with what was read, once nothing
    /// the user is in the middle of would be ended by it (see [`poll_files`](Self::poll_files)),
    /// into image `target` if it is one's (and not at all if `target` is closed meanwhile),
    /// answering call `answer` (its number and groups) with what `then` gives.
    pub(crate) fn read_then(
        &mut self,
        paths: Vec<PathBuf>,
        target: Option<DocId>,
        answer: Option<(u64, Vec<u64>)>,
        then: Then,
        ctx: &egui::Context,
    ) {
        let what = match &paths[..] {
            [one] => crate::state::file_title(one),
            many => format!("{} files", many.len()),
        };
        let (tx, rx) = mpsc::channel();
        let waker = ctx.clone();
        let reading = paths.clone();
        std::thread::Builder::new()
            .name("reading files".into())
            .spawn(move || {
                // Given up meanwhile (Esc), the receiver is gone, and what was read dropped.
                let _ = tx.send(read_files(&reading));
                waker.request_repaint();
            })
            .expect("starting a thread to read files");
        let reading = FileReading {
            rx,
            paths,
            result: None,
            target: self.target_now(target),
            notice: String::new(),
            what,
            then,
            answer,
            call: self.calling.clone(),
        };
        self.file_readings.push(reading);
        // Its notice put up by the next frame: what was read already is used between frames,
        // not inside whatever asked for this (a script's call, which would take what that
        // tells the user as its own).
        ctx.request_repaint();
    }

    /// Does what each file read was read for (see [`read_then`](Self::read_then)), in the
    /// order asked for, unless the user is in the middle of something it would end: a dialog,
    /// a drag (`pointer_held`: done when the button is let go), an edit under way on the
    /// image it goes into, or a box opened there since. Meanwhile the status bar says what
    /// is being read, or what it waits for.
    pub(crate) fn poll_files(&mut self, pointer_held: bool) {
        let esc = if self.box_takes_escape() { "" } else { " (Esc cancels)" };
        while let Some(r) = self.file_readings.first_mut() {
            if r.result.is_none() {
                match r.rx.try_recv() {
                    Ok(read) => r.result = Some(read),
                    Err(mpsc::TryRecvError::Empty) => {
                        let text = format!("Reading {}…{esc}", r.what);
                        return self.file_notice(text);
                    }
                    // (Its thread always sends before it ends; were it not to, each file is
                    // said to have failed rather than to have been read as nothing.)
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let failed = |p: &PathBuf| Err(format!("Reading {} stopped.", crate::state::file_title(p)));
                        r.result = Some(r.paths.iter().map(|p| (p.clone(), failed(p))).collect());
                    }
                }
            }
            if pointer_held {
                return;
            }
            if let Some(until) = self.file_readings.first().and_then(|r| self.waits_for(&r.target)) {
                let r = self.file_readings.first().expect("there");
                let text = format!("{} will be used when {until}.", r.what);
                return self.file_notice(text);
            }
            let r = self.file_readings.remove(0);
            self.take_down(&r.notice);
            let target = r.target.id;
            let gone = target.is_some_and(|t| self.doc(t).is_none());
            let result = if gone {
                Err(format!("The image was closed before {} was read.", r.what))
            } else {
                // A history state it makes joins the call's.
                let groups = r.answer.as_ref().map(|(_, g)| g.clone());
                let before = match (target.and_then(|t| self.doc_mut(t)), groups) {
                    (Some(d), Some(g)) => Some(std::mem::replace(&mut d.groups, g)),
                    _ => None,
                };
                let result = (r.then)(self, r.result.unwrap_or_default());
                if let (Some(before), Some(d)) = (before, target.and_then(|t| self.doc_mut(t))) {
                    d.groups = before;
                }
                result
            };
            match (r.answer, result) {
                // A notification, which nobody hears the answer to: what it says goes to the user.
                // Answered still: its connection waits for that before it reads its next request.
                (Some((n, _)), Err(why)) if r.call.as_ref().is_some_and(|c| !c.heard) => {
                    let name = r.call.map(|c| c.name).unwrap_or_default();
                    self.message(crate::state::MessageKind::Error, name, why.clone());
                    self.answered.push((n, Err(why)));
                }
                (Some((n, _)), result) => {
                    // Refused once read, as one refused at once, the call puts the user's view
                    // back (it made the image it names the active one), unless the user has
                    // moved on.
                    if result.is_err()
                        && let Some(view) =
                            r.call.and_then(|c| c.view).filter(|_| target.is_some() && self.active_doc == target)
                    {
                        self.restore_view(view);
                    }
                    self.answered.push((n, result));
                }
                // Said, if it went wrong: what was read for has said whatever else needs saying.
                (None, Err(why)) => self.message(crate::state::MessageKind::Error, "Could not open", why),
                (None, Ok(_)) => {}
            }
        }
    }

    /// Puts `text` up for the first file reading, taking down what it said before.
    fn file_notice(&mut self, text: String) {
        let r = self.file_readings.first_mut().expect("there");
        if r.notice != text {
            let old = std::mem::replace(&mut r.notice, text.clone());
            self.take_down(&old);
            self.notify(text);
        }
    }

    /// Whether files are being read, or wait to be used.
    pub(crate) fn reading_files(&self) -> bool {
        !self.file_readings.is_empty()
    }

    /// Gives up every file being read or waiting to be used (Esc); the calls they would have
    /// answered are told so.
    pub(crate) fn cancel_files(&mut self) {
        for r in std::mem::take(&mut self.file_readings) {
            self.take_down(&r.notice);
            if let Some((n, _)) = r.answer {
                self.answered.push((n, Err("Reading the file was cancelled.".into())));
            }
        }
        self.notify("Opening cancelled.");
    }
}

/// Waiting for files being read, for tests.
#[cfg(test)]
pub(crate) mod testing {
    use crate::state::AppState;

    /// Waits until every file being read has been read and used.
    pub fn wait(st: &mut AppState) {
        for _ in 0..2000 {
            st.poll_files(false);
            if !st.reading_files() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the files were never read");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Background, NewImage};

    fn image() -> AppState {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 40, height: 30, background: Background::White, resolution: 72.0 });
        st
    }

    fn png(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("opal-opening-{}-{name}", std::process::id()));
        image::RgbaImage::from_pixel(6, 4, image::Rgba([9, 99, 199, 255])).save(&path).unwrap();
        path
    }

    /// Waits until the files being read have been read (not used).
    fn read(st: &mut AppState) {
        for _ in 0..2000 {
            if st.file_readings.iter().all(|r| r.result.is_some()) {
                return;
            }
            for r in &mut st.file_readings {
                if r.result.is_none()
                    && let Ok(read) = r.rx.try_recv()
                {
                    r.result = Some(read);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("never read");
    }

    #[test]
    fn a_file_read_waits_for_the_dialog_open_and_is_then_opened() {
        let mut st = image();
        let ctx = egui::Context::default();
        let path = png("waits.png");
        st.rotate_by = Some(crate::image_ops::RotateBy { degrees: 30.0, grow: true });
        st.open_later(vec![path.clone()], None, &ctx);
        read(&mut st);
        st.poll_files(false);
        assert_eq!(st.docs.len(), 1, "opened behind the dialog");
        let said = st.notice.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
        assert!(said.contains("will be used when the dialog closes"), "{said}");
        // Held by a drag too, until the button is let go.
        st.rotate_by = None;
        st.poll_files(true);
        assert_eq!(st.docs.len(), 1, "opened in the middle of a drag");
        st.poll_files(false);
        assert_eq!((st.docs.len(), st.reading_files()), (2, false));
        assert!(st.notice.is_none(), "the notice left up");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn esc_gives_files_up_and_a_scripts_call_is_told() {
        let mut st = image();
        let ctx = egui::Context::default();
        let path = png("given-up.png");
        st.open_later(vec![path.clone()], Some((41, Vec::new())), &ctx);
        assert!(st.reading_files());
        st.cancel_files();
        read(&mut st);
        st.poll_files(false);
        assert_eq!(st.docs.len(), 1, "opened though given up");
        assert!(matches!(st.answered.last(), Some((41, Err(why))) if why.contains("cancelled")), "{:?}", st.answered);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_file_placed_into_an_image_closed_meanwhile_is_dropped_and_said() {
        let mut st = image();
        let ctx = egui::Context::default();
        let path = png("closed.png");
        st.place_later(path.clone(), None, false, None, &ctx);
        let id = st.active_doc.unwrap();
        st.close_now(id);
        read(&mut st);
        st.poll_files(false);
        assert!(st.docs.is_empty());
        let said = st.messages.back().map(|m| m.body.clone()).unwrap_or_default();
        assert!(said.contains("closed before"), "{said}");
        std::fs::remove_file(&path).ok();
    }

    /// An Opal Studio file of its own (one whose image keeps its path), called `name`.
    fn opal_file(name: &str) -> PathBuf {
        use opal_core::document::{Document, IdGen, Layer, Position};
        let path = std::env::temp_dir().join(format!("opal-opening-{}-{name}", std::process::id()));
        let (mut doc, mut ids) = (Document::new(8, 8), IdGen::default());
        let layer = Layer::new_raster(ids.layer(), ids.object(), "l", Default::default());
        doc.insert(Position { parent: None, index: 0 }, layer).unwrap();
        opal_io::native::save(&doc, &ids, &path, &Default::default()).unwrap();
        path
    }

    #[test]
    fn a_file_opened_again_while_it_is_read_opens_once() {
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let path = opal_file("twice.opal");
        // Both wait for the dialog: neither is opened before the other is asked for.
        st.rotate_by = Some(crate::image_ops::RotateBy { degrees: 30.0, grow: true });
        st.open_later(vec![path.clone()], None, &ctx);
        st.open_later(vec![path.clone()], None, &ctx);
        read(&mut st);
        st.rotate_by = None;
        st.poll_files(false);
        std::fs::remove_file(&path).ok();
        assert_eq!(st.docs.len(), 1, "opened twice");
        assert_eq!(st.active_doc, Some(st.docs[0].id));
    }

    #[test]
    fn layers_opened_with_no_image_open_go_into_a_new_one_whatever_is_active_then() {
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let path = png("layers-new.png");
        st.open_as_layers_later(vec![path.clone()], None, &ctx);
        // An image made meanwhile: the user's work, which the layers stay out of.
        st.create_image(&NewImage { width: 40, height: 30, background: Background::White, resolution: 72.0 });
        let made = st.active_doc.unwrap();
        read(&mut st);
        st.poll_files(false);
        std::fs::remove_file(&path).ok();
        assert_eq!(st.doc(made).unwrap().doc().layers.len(), 1, "put into the image made meanwhile");
        assert_eq!(st.docs.len(), 2, "not opened in a new image");
        let new = st.docs.iter().find(|d| d.id != made).unwrap();
        assert_eq!((new.doc().canvas.width, new.doc().layers.len()), (6, 1));
    }

    #[test]
    fn files_being_read_are_work_under_way() {
        let mut st = image();
        let ctx = egui::Context::default();
        let path = png("work.png");
        assert!(!st.work_under_way());
        // Held by a dialog, so that it is still being read when asked.
        st.rotate_by = Some(crate::image_ops::RotateBy { degrees: 30.0, grow: true });
        st.open_later(vec![path.clone()], Some((7, Vec::new())), &ctx);
        assert!(st.work_under_way(), "a file being read is not work under way");
        st.rotate_by = None;
        read(&mut st);
        st.poll_files(false);
        std::fs::remove_file(&path).ok();
        assert!(matches!(st.answered.last(), Some((7, Ok(_)))), "{:?}", st.answered);
    }

    #[test]
    fn a_reader_that_fails_unexpectedly_says_so_of_its_file_and_reads_the_others() {
        let (a, b) = (PathBuf::from("a.png"), PathBuf::from("b.png"));
        let read = read_files_with(&[a.clone(), b.clone()], |p| {
            if p == Path::new("a.png") { panic!("a decoder's bug") } else { Err("b's own".to_owned()) }
        });
        assert!(
            matches!(&read[0], (p, Err(why)) if *p == a && why.contains("a decoder's bug")),
            "{:?}",
            read[0].1.as_ref().err()
        );
        assert!(matches!(&read[1], (p, Err(why)) if *p == b && why == "b's own"));
    }

    #[test]
    fn what_was_read_is_used_between_frames_not_inside_another_asking() {
        let mut st = image();
        let ctx = egui::Context::default();
        let (first, second) = (png("first.png"), png("second.png"));
        st.open_later(vec![first.clone()], None, &ctx);
        read(&mut st);
        // A script's call asking for another file now: the first, read already, waits for the
        // frame's own turn, so that what opening it tells the user is not taken as the call's.
        st.open_later(vec![second.clone()], Some((5, Vec::new())), &ctx);
        assert_eq!(st.docs.len(), 1, "used inside the call");
        read(&mut st);
        st.poll_files(false);
        std::fs::remove_file(&first).ok();
        std::fs::remove_file(&second).ok();
        assert_eq!(st.docs.len(), 3);
    }

    #[test]
    fn a_place_asked_for_with_no_image_open_is_answered() {
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let apply = Some((3, Vec::new(), crate::api::BoxArgs::default(), false));
        st.place_later(png("nowhere.png"), None, false, apply, &ctx);
        assert!(matches!(st.answered.last(), Some((3, Err(_)))), "{:?}", st.answered);
        assert!(!st.reading_files());
    }
}
