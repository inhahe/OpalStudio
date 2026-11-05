//! Opal Studio's file formats.
//!
//! - [`image_files`]: importing PNG and JPEG into the working pixel format, and exporting
//!   the flattened canvas to them.
//! - [`native`]: the `.opal` format, which keeps everything editable.
//!
//! Every write goes to a temporary file beside the target and replaces the target only
//! once complete, so a failed or interrupted save never damages the file already there.

pub mod image_files;
pub mod native;
pub mod pixels;

use std::io::Write;
use std::path::{Path, PathBuf};

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
}

impl IoError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> IoError {
        IoError::Io { path: path.to_owned(), source }
    }
}

/// Writes a file by calling `write` on a temporary file in the same directory, then
/// renaming it over `path`. On any error, or a panic in `write`, the temporary file is
/// removed and `path` is left as it was.
pub fn write_atomically(
    path: &Path,
    write: impl FnOnce(&mut std::io::BufWriter<std::fs::File>) -> Result<(), IoError>,
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

    (|| {
        let file = std::fs::File::create(&tmp).map_err(|e| IoError::io(&tmp, e))?;
        let mut out = std::io::BufWriter::new(file);
        write(&mut out)?;
        let file = out.into_inner().map_err(|e| IoError::io(&tmp, e.into_error()))?;
        file.sync_all().map_err(|e| IoError::io(&tmp, e))?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|e| IoError::io(path, e))
    })()
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
            let _ = write_atomically(&path, |out| {
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
    fn a_failed_write_leaves_the_existing_file() {
        let dir = std::env::temp_dir().join(format!("opal-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.txt");
        std::fs::write(&path, b"original").unwrap();
        let r = write_atomically(&path, |out| {
            write_all(out, b"partial", &path)?;
            Err(IoError::Unsupported { path: path.clone(), message: "simulated failure".into() })
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "the temporary file was removed");

        write_atomically(&path, |out| write_all(out, b"new", &path)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
