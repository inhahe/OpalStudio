//! Opal Studio's file formats.
//!
//! - [`image_files`]: importing PNG and JPEG into the working pixel format, and exporting
//!   the flattened canvas to them.
//! - [`native`]: the `.opal` format, which keeps everything editable.
//!
//! Every write goes to a temporary file beside the target and replaces the target only
//! once complete, so a failed, cancelled or interrupted save never damages the file
//! already there. A long write tells how far it has got, and can be stopped, through a
//! [`Progress`].

pub mod image_files;
pub mod native;
pub mod pixels;

use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// Why reading or writing a file failed.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: the image could not be decoded: {message}")]
    Decode { path: PathBuf, message: String },
    #[error("{path}: the image could not be encoded: {message}")]
    Encode { path: PathBuf, message: String },
    #[error("{path}: not a valid Opal Studio file: {message}")]
    Format { path: PathBuf, message: String },
    #[error("{path}: {message}")]
    Unsupported { path: PathBuf, message: String },
    /// The write was cancelled (through its [`Progress`]); the file was left as it was.
    #[error("{path}: cancelled")]
    Cancelled { path: PathBuf },
}

impl IoError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> IoError {
        IoError::Io { path: path.to_owned(), source }
    }
}

/// How far a write has got, and a way to stop it, shared between the thread writing and
/// the one showing how it goes. The write counts in units of its own choosing (pixels,
/// tiles), `total` of which make the whole.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    /// How much of the write is done, from 0 to 1; `None` until it knows how much there is.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Relaxed);
        (total > 0).then(|| (self.done.load(Relaxed) as f64 / total as f64).min(1.0) as f32)
    }

    /// Asks the write to stop. It fails with [`IoError::Cancelled`] and leaves the file as it
    /// was, unless it was already in place.
    pub fn cancel(&self) {
        self.cancelled.store(true, Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Relaxed)
    }

    /// The write has `total` units of work to do, none of them done.
    pub fn begin(&self, total: u64) {
        self.done.store(0, Relaxed);
        self.total.store(total, Relaxed);
    }

    /// `n` more units are done.
    pub fn advance(&self, n: u64) {
        self.done.fetch_add(n, Relaxed);
    }

    /// [`IoError::Cancelled`] for `path` once cancelled.
    pub(crate) fn check(&self, path: &Path) -> Result<(), IoError> {
        if self.is_cancelled() { Err(IoError::Cancelled { path: path.to_owned() }) } else { Ok(()) }
    }
}

/// The temporary file [`write_atomically`] writes to, failing every write and seek once the
/// write's [`Progress`] is cancelled, so that whatever is writing (an encoder, an archive)
/// stops at its next one.
pub struct Stoppable<'p> {
    file: std::fs::File,
    progress: Option<&'p Progress>,
}

impl Stoppable<'_> {
    fn stopped(&self) -> std::io::Result<()> {
        match self.progress {
            Some(p) if p.is_cancelled() => Err(std::io::Error::other("the write was cancelled")),
            _ => Ok(()),
        }
    }
}

impl Write for Stoppable<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stopped()?;
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stopped()?;
        self.file.flush()
    }
}

impl Seek for Stoppable<'_> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.stopped()?;
        self.file.seek(pos)
    }
}

/// What [`write_atomically`] gives its writer to write to.
pub type Out<'p> = std::io::BufWriter<Stoppable<'p>>;

/// Writes a file by calling `write` on a temporary file in the same directory, then
/// renaming it over `path`. On any error, or a panic in `write`, the temporary file is
/// removed and `path` is left as it was. Once `progress` is cancelled, the temporary file
/// takes no more writes, and the write fails with [`IoError::Cancelled`] (unless it was
/// already renamed).
pub fn write_atomically(
    path: &Path,
    progress: Option<&Progress>,
    write: impl FnOnce(&mut Out) -> Result<(), IoError>,
) -> Result<(), IoError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| IoError::Unsupported { path: path.to_owned(), message: "not a file path".into() })?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    // Removes the temporary file however this ends (once renamed, there is none to remove).
    struct Remove<'a>(&'a Path);
    impl Drop for Remove<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _remove = Remove(&tmp);

    let result = (|| {
        let file = std::fs::File::create(&tmp).map_err(|e| IoError::io(&tmp, e))?;
        let mut out = std::io::BufWriter::new(Stoppable { file, progress });
        write(&mut out)?;
        let file = out.into_inner().map_err(|e| IoError::io(&tmp, e.into_error()))?.file;
        file.sync_all().map_err(|e| IoError::io(&tmp, e))?;
        drop(file);
        if let Some(p) = progress {
            p.check(path)?;
        }
        std::fs::rename(&tmp, path).map_err(|e| IoError::io(path, e))
    })();
    // Whatever failed once the write was cancelled failed because it was.
    match result {
        Err(_) if progress.is_some_and(Progress::is_cancelled) => Err(IoError::Cancelled { path: path.to_owned() }),
        r => r,
    }
}

/// `std::io::Write` errors reported against `path`.
pub fn write_all(out: &mut impl Write, bytes: &[u8], path: &Path) -> Result<(), IoError> {
    out.write_all(bytes).map_err(|e| IoError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_while_writing_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("opal-atomic-panic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("y.txt");
        let r = std::panic::catch_unwind(|| {
            let _ = write_atomically(&path, None, |out| {
                write_all(out, b"partial", &path)?;
                panic!("an encoder went wrong");
            });
        });
        assert!(r.is_err());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn a_cancelled_write_stops_and_leaves_the_existing_file() {
        let dir = std::env::temp_dir().join(format!("opal-atomic-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("z.txt");
        std::fs::write(&path, b"original").unwrap();
        // Cancelled midway (the writes larger than the buffer, so they reach the file).
        let p = Progress::default();
        let mut went_on = false;
        let midway = write_atomically(&path, Some(&p), |out| {
            write_all(out, &[1; 100_000], &path)?;
            p.cancel();
            write_all(out, &[2; 100_000], &path)?;
            went_on = true;
            Ok(())
        });
        assert!(!went_on, "a write went through once cancelled");
        // Cancelled once everything was written, before the rename.
        let p = Progress::default();
        let at_the_end = write_atomically(&path, Some(&p), |out| {
            write_all(out, b"all of it", &path)?;
            out.flush().map_err(|e| IoError::io(&path, e))?;
            p.cancel();
            Ok(())
        });
        let back = std::fs::read(&path).unwrap();
        let entries = std::fs::read_dir(&dir).unwrap().count();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(midway, Err(IoError::Cancelled { .. })), "{midway:?}");
        assert!(matches!(at_the_end, Err(IoError::Cancelled { .. })), "{at_the_end:?}");
        assert_eq!(back, b"original");
        assert_eq!(entries, 1, "the temporary files were removed");
    }

    #[test]
    fn a_failed_write_leaves_the_existing_file() {
        let dir = std::env::temp_dir().join(format!("opal-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.txt");
        std::fs::write(&path, b"original").unwrap();
        let r = write_atomically(&path, None, |out| {
            write_all(out, b"partial", &path)?;
            Err(IoError::Unsupported { path: path.clone(), message: "simulated failure".into() })
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "the temporary file was removed");

        write_atomically(&path, None, |out| write_all(out, b"new", &path)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
