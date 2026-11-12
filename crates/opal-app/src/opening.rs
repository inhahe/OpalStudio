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
    paths.iter().map(|p| (p.clone(), read_file(p))).collect()
}

/// Files being read on a worker thread, and what is then done with them.
pub struct FileReading {
    rx: mpsc::Receiver<Read>,
    /// What reading gave, once it has come back, while what it is for waits.
    result: Option<Read>,
    /// The image what was read goes into, if any: the one active when it was asked for.
    target: Option<DocId>,
    /// The transform box `target` had open then (its session's serial), and its crop box: a
    /// box opened (or crop box changed) since is the user's newer work, which this waits for
    /// rather than ending it.
    box_then: Option<u64>,
    crop_then: Option<crate::edit_box::EditBox>,
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
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("reading files".into())
            .spawn(move || {
                // Given up meanwhile (Esc), the receiver is gone, and what was read dropped.
                let _ = tx.send(read_files(&paths));
                ctx.request_repaint();
            })
            .expect("starting a thread to read files");
        let d = target.and_then(|t| self.doc(t));
        let reading = FileReading {
            rx,
            result: None,
            target,
            box_then: d.and_then(|d| d.transform.as_ref()).map(|s| s.serial),
            crop_then: d.and_then(|d| d.crop),
            notice: String::new(),
            what,
            then,
            answer,
            call: self.calling.clone(),
        };
        self.file_readings.push(reading);
        self.poll_files(false);
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
                    Err(mpsc::TryRecvError::Disconnected) => r.result = Some(Vec::new()),
                }
            }
            if pointer_held {
                return;
            }
            if let Some(until) = self.file_waits() {
                let r = self.file_readings.first().expect("there");
                let text = format!("{} will be used when {until}.", r.what);
                return self.file_notice(text);
            }
            let r = self.file_readings.remove(0);
            self.take_down(&r.notice);
            let gone = r.target.is_some_and(|t| self.doc(t).is_none());
            let result = if gone {
                Err(format!("The image was closed before {} was read.", r.what))
            } else {
                // A history state it makes joins the call's.
                let groups = r.answer.as_ref().map(|(_, g)| g.clone());
                let before = match (r.target.and_then(|t| self.doc_mut(t)), groups) {
                    (Some(d), Some(g)) => Some(std::mem::replace(&mut d.groups, g)),
                    _ => None,
                };
                let result = (r.then)(self, r.result.unwrap_or_default());
                if let (Some(before), Some(d)) = (before, r.target.and_then(|t| self.doc_mut(t))) {
                    d.groups = before;
                }
                result
            };
            match (r.answer, result) {
                // A notification, which nobody hears the answer to: what it says goes to the user.
                (Some(_), Err(why)) if r.call.as_ref().is_some_and(|c| !c.heard) => {
                    let name = r.call.map(|c| c.name).unwrap_or_default();
                    self.message(crate::state::MessageKind::Error, name, why);
                }
                (Some((n, _)), result) => self.answered.push((n, result)),
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

    /// What the first file reading, read, waits for, if it must wait.
    fn file_waits(&self) -> Option<&'static str> {
        let r = self.file_readings.first()?;
        if self.dialog_open() {
            return Some("the dialog closes");
        }
        let d = r.target.and_then(|t| self.doc(t))?;
        if d.busy.is_some() {
            Some("the edit under way on the image finishes")
        } else if d.transform.as_ref().is_some_and(|s| Some(s.serial) != r.box_then) {
            Some("the transform box closes")
        } else if d.crop.is_some() && d.crop != r.crop_then {
            Some("you crop or leave the Crop tool")
        } else {
            None
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
}
