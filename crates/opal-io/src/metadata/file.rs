//! A file's metadata, read without opening its image, and rewritten in place: what scripts
//! use to rename a folder of photographs by when they were taken, or shift the times a
//! camera's clock got wrong, without decoding a pixel.

use super::{container, iptc, read, xmp::Xmp};
use crate::{IoError, write_all, write_atomically};
use opal_core::document::Metadata;
use std::path::Path;

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG")
}

/// The metadata of the image file at `path`: a JPEG's, a PNG's (see [`read`]), or an Opal
/// Studio file's (its layers not read).
pub fn of_file(path: &Path) -> Result<Metadata, IoError> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("opal")) {
        return crate::native::metadata_of(path);
    }
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    if !is_jpeg(&bytes) && !is_png(&bytes) {
        return Err(IoError::Unsupported {
            path: path.to_owned(),
            message: "not a PNG, JPEG or Opal Studio file".into(),
        });
    }
    Ok(read(&bytes))
}

/// Rewrites the metadata of the JPEG or PNG at `path` as `change` makes it of what it was:
/// written as it is (no orientation or size made true, as export does: the image is the
/// file's own), the image data untouched (neither decoded nor encoded again), and in a JPEG,
/// Photoshop's other resources kept (a clipping path, say). Replaced atomically. Why not, if
/// it could not be.
pub fn rewrite(path: &Path, change: impl FnOnce(&Metadata) -> Result<Metadata, String>) -> Result<(), IoError> {
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    let unsupported = |message: String| IoError::Unsupported { path: path.to_owned(), message };
    if !is_jpeg(&bytes) && !is_png(&bytes) {
        return Err(unsupported("only a PNG's or JPEG's metadata is rewritten in place".into()));
    }
    let new = change(&read(&bytes)).map_err(unsupported)?;
    let xmp = new.xmp.as_deref().and_then(Xmp::read);
    let out = if is_jpeg(&bytes) {
        let old_block = container::photoshop_block(&bytes);
        let photoshop =
            (new.iptc.is_some() || !old_block.is_empty()).then(|| iptc::with_iim(&old_block, new.iptc.as_deref()));
        let segments = container::jpeg_segments_of(new.exif.as_deref(), xmp.as_ref(), photoshop.as_deref())
            .map_err(|why| unsupported(format!("the metadata does not fit in a JPEG: {why}")))?;
        container::replace_jpeg(&bytes, &segments)
    } else {
        let chunks = container::png_chunks_of(new.exif.as_deref(), xmp.as_ref(), new.iptc.as_deref());
        container::replace_png(&bytes, &container::png_bytes(&chunks))
    };
    let out = out.ok_or_else(|| unsupported("its structure could not be read".into()))?;
    write_atomically(path, None, |w| write_all(w, &out, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::exif::{Exif, Ifd, Value};
    use std::sync::Arc;

    #[test]
    fn a_files_metadata_is_rewritten_its_pixels_untouched() {
        let dir = std::env::temp_dir().join(format!("opal-metadata-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, format) in [("a.jpg", image::ImageFormat::Jpeg), ("a.png", image::ImageFormat::Png)] {
            let path = dir.join(name);
            image::RgbImage::from_pixel(9, 7, image::Rgb([10, 200, 30])).save_with_format(&path, format).unwrap();
            let pixels = |p: &Path| image::open(p).unwrap().to_rgb8();
            let before = pixels(&path);
            // A capture time given, then shifted by an hour.
            rewrite(&path, |m| {
                let mut e = Exif::new();
                e.set(Ifd::Exif, 0x9003, &Value::Ascii("2024:05:06 14:03:22".into()));
                Ok(Metadata { exif: Some(Arc::from(e.into_bytes())), ..m.clone() })
            })
            .unwrap();
            rewrite(&path, |m| {
                let mut e = Exif::read(m.exif.as_deref().unwrap()).unwrap();
                e.set(Ifd::Exif, 0x9003, &Value::Ascii("2024:05:06 15:03:22".into()));
                Ok(Metadata { exif: Some(Arc::from(e.into_bytes())), ..m.clone() })
            })
            .unwrap();
            let read = of_file(&path).unwrap();
            let taken = Exif::read(read.exif.as_deref().unwrap()).unwrap().get(Ifd::Exif, 0x9003);
            assert_eq!(taken, Some(Value::Ascii("2024:05:06 15:03:22".into())), "{name}");
            assert_eq!(pixels(&path), before, "{name}'s pixels changed");
        }
        let text = dir.join("a.txt");
        std::fs::write(&text, "not an image").unwrap();
        assert!(rewrite(&text, |m| Ok(m.clone())).is_err() && of_file(&text).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
