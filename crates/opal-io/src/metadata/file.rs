//! A file's metadata, read without opening its image, and rewritten in place: what scripts
//! use to rename a folder of photographs by when they were taken, or shift the times a
//! camera's clock got wrong, without decoding a pixel.

use super::container::{is_jpeg, is_png};
use super::{container, iptc, read, xmp::Xmp};
use crate::{IoError, write_all, write_atomically};
use opal_core::document::Metadata;
use std::path::Path;

/// The metadata of the image file at `path`: a JPEG's, a PNG's (see [`read`]), or an Opal
/// Studio file's (its layers not read). Of a JPEG or PNG, only what comes before or between
/// its image data is read, not the image data itself.
pub fn of_file(path: &Path) -> Result<Metadata, IoError> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("opal")) {
        return crate::native::metadata_of(path);
    }
    let head = head_of(path).map_err(|e| IoError::io(path, e))?;
    if !is_jpeg(&head) && !is_png(&head) {
        return Err(IoError::Unsupported {
            path: path.to_owned(),
            message: "not a PNG, JPEG or Opal Studio file".into(),
        });
    }
    Ok(read(&head))
}

/// What a JPEG's or PNG's metadata can be in, read from the file at `path` without its image
/// data: a JPEG up to its first scan; a PNG's chunks but its IDAT ones (skipped, not read).
/// The bytes are as the file has them, so that `read` reads them as it reads a whole file.
/// Anything else, its first bytes.
fn head_of(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut head = vec![0u8; 8];
    let n = f.read(&mut head)?;
    head.truncate(n);
    if is_jpeg(&head) {
        // SOI, then each segment (marker, length, data) until the scan.
        let mut at: usize = 2;
        loop {
            let short = (at + 4).saturating_sub(head.len());
            if short > 0 && !take(&mut f, &mut head, short)? {
                break;
            }
            let (marker, len) = (head[at + 1], usize::from(u16::from_be_bytes([head[at + 2], head[at + 3]])));
            // A fill byte before a marker.
            if head[at] == 0xFF && marker == 0xFF {
                at += 1;
                continue;
            }
            if head[at] != 0xFF || marker == 0xDA || marker == 0xD9 || len < 2 {
                break;
            }
            if !take(&mut f, &mut head, len - 2)? {
                break;
            }
            at += 2 + len;
        }
    } else if is_png(&head) {
        // Each chunk (length, type, data, CRC), but the image data's (IDAT), left out, its bytes
        // skipped rather than read.
        loop {
            let at = head.len();
            if !take(&mut f, &mut head, 8)? {
                break;
            }
            let len = u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]) as usize;
            let kind = [head[at + 4], head[at + 5], head[at + 6], head[at + 7]];
            if &kind == b"IDAT" {
                head.truncate(at);
                f.seek(SeekFrom::Current(len as i64 + 4))?;
                continue;
            }
            if !take(&mut f, &mut head, len + 4)? || &kind == b"IEND" {
                break;
            }
        }
    }
    Ok(head)
}

/// Reads `len` more bytes from `f` onto the end of `head`: whether there were that many.
fn take(f: &mut impl std::io::Read, head: &mut Vec<u8>, len: usize) -> std::io::Result<bool> {
    use std::io::Read;
    let had = head.len();
    f.by_ref().take(len as u64).read_to_end(head)?;
    Ok(head.len() - had == len)
}

/// Rewrites the metadata of the JPEG or PNG at `path` as `change` makes it of what it was:
/// written as it is (no orientation or size made true, as export does: the image is the
/// file's own), the image data untouched (neither decoded nor encoded again). Only the parts
/// `change` changed are written; the others, what Opal Studio could not read among them, stay
/// as the file has them, and in a JPEG, Photoshop's other resources too (a clipping path, say).
/// A change to a part the file has but Opal Studio could not read is refused, rather than
/// written over it. Replaced atomically. What the metadata is now; or why not, if it could
/// not be.
pub fn rewrite(path: &Path, change: impl FnOnce(&Metadata) -> Result<Metadata, String>) -> Result<Metadata, IoError> {
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    let unsupported = |message: String| IoError::Unsupported { path: path.to_owned(), message };
    if !is_jpeg(&bytes) && !is_png(&bytes) {
        return Err(unsupported("only a PNG's or JPEG's metadata is rewritten in place".into()));
    }
    let raw = if is_jpeg(&bytes) { container::read_jpeg(&bytes) } else { container::read_png(&bytes) };
    let had = container::Parts { exif: raw.exif.is_some(), xmp: raw.xmp.is_some(), iptc: raw.iptc.is_some() };
    let old = raw.into_metadata();
    let new = change(&old).map_err(unsupported)?;
    let parts = container::Parts {
        exif: new.exif != old.exif,
        xmp: new.xmp != old.xmp,
        iptc: new.iptc != old.iptc || new.iptc_changed != old.iptc_changed,
    };
    if parts == (container::Parts { exif: false, xmp: false, iptc: false }) {
        return Ok(new);
    }
    for (name, changed, unread) in [
        ("EXIF", parts.exif, had.exif && old.exif.is_none()),
        ("XMP", parts.xmp, had.xmp && old.xmp.is_none()),
        ("IPTC", parts.iptc, had.iptc && old.iptc.is_none()),
    ] {
        if changed && unread {
            return Err(unsupported(format!(
                "its {name} could not be read, and would be lost: the file is left as it is"
            )));
        }
    }
    let exif = new.exif.as_deref().filter(|_| parts.exif);
    // XMP that would not read back is refused, rather than written as none.
    let xmp = match new.xmp.as_deref().filter(|_| parts.xmp) {
        Some(bytes) => Some(Xmp::read(bytes).ok_or_else(|| unsupported("the XMP made does not read".into()))?),
        None => None,
    };
    let out = if is_jpeg(&bytes) {
        let old_block = container::photoshop_block(&bytes);
        let photoshop = (parts.iptc && (new.iptc.is_some() || !old_block.is_empty()))
            .then(|| iptc::with_iim(&old_block, new.iptc.as_deref()));
        let segments = container::jpeg_segments_of(exif, xmp.as_ref(), photoshop.as_deref())
            .map_err(|why| unsupported(format!("the metadata does not fit in a JPEG: {why}")))?;
        container::replace_jpeg(&bytes, &segments, parts)
    } else {
        let iim = new.iptc.as_deref().filter(|_| parts.iptc);
        let chunks = container::png_chunks_of(exif, xmp.as_ref(), iim);
        container::replace_png(&bytes, &container::png_bytes(&chunks), parts)
    };
    let out = out.ok_or_else(|| unsupported("its structure could not be read".into()))?;
    write_atomically(path, None, |w| write_all(w, &out, path))?;
    Ok(new)
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

    #[test]
    fn what_could_not_be_read_or_was_not_changed_stays_as_the_file_has_it() {
        let dir = std::env::temp_dir().join(format!("opal-metadata-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jpg");
        image::RgbImage::from_pixel(9, 7, image::Rgb([10, 200, 30])).save(&path).unwrap();
        // XMP no XML reader takes (an unescaped ampersand), and an IIM whose digest says it was
        // changed after the XMP.
        let mut xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>A & B</x:xmpmeta>".to_vec();
        let mut segments = vec![0xFF, 0xE1];
        segments.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        segments.append(&mut xmp);
        let mut iim = iptc::Iim::default();
        iim.set_texts(2, 5, &["Old".into()]);
        let mut block = iptc::to_resources(&iim.bytes());
        let title = block.windows(3).position(|w| w == b"Old").unwrap();
        block[title] = b'N';
        segments.extend_from_slice(&[0xFF, 0xED]);
        segments.extend_from_slice(&((block.len() + 2) as u16).to_be_bytes());
        segments.extend_from_slice(&block);
        let jpeg = std::fs::read(&path).unwrap();
        std::fs::write(&path, container::replace_jpeg(&jpeg, &segments, container::Parts::ALL).unwrap()).unwrap();
        assert!(of_file(&path).unwrap().iptc_changed);
        // EXIF changed alone: the XMP and the IIM, digest and all, kept as they were.
        rewrite(&path, |m| {
            let mut e = Exif::new();
            e.set(Ifd::Exif, 0x9003, &Value::Ascii("2024:05:06 15:03:22".into()));
            Ok(Metadata { exif: Some(Arc::from(e.into_bytes())), ..m.clone() })
        })
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(5).any(|w| w == b"A & B"), "the XMP that could not be read was lost");
        let read = of_file(&path).unwrap();
        assert!(read.iptc_changed, "the IIM's digest made to say it agrees with the XMP");
        assert!(read.exif.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A JPEG of its own, in a folder of `folder`'s own.
    fn jpeg_in(folder: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("opal-metadata-{folder}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jpg");
        image::RgbImage::from_pixel(9, 7, image::Rgb([10, 200, 30])).save(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn a_fill_byte_before_a_segment_is_skipped_as_readers_skip_it() {
        let (dir, path) = jpeg_in("fill");
        // EXIF, then XMP long enough that a head cut short where a fill byte was taken for a
        // marker would not hold it.
        rewrite(&path, |m| {
            let mut e = Exif::new();
            e.set(Ifd::Image, 0x010F, &Value::Ascii("Camera".into()));
            let mut x = Xmp::default();
            x.set_default_text(crate::metadata::xmp::DC, "description", &"sea ".repeat(15_000));
            Ok(Metadata {
                exif: Some(Arc::from(e.into_bytes())),
                xmp: Some(Arc::from(x.packet().into_bytes())),
                ..m.clone()
            })
        })
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let app1 = bytes.windows(2).position(|w| w == [0xFF, 0xE1]).unwrap();
        bytes.insert(app1, 0xFF);
        std::fs::write(&path, &bytes).unwrap();
        let read = of_file(&path).unwrap();
        assert!(read.exif.is_some(), "the EXIF after the fill byte was not read");
        assert!(read.xmp.is_some(), "the XMP after it was not read");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_change_to_a_part_that_could_not_be_read_is_refused() {
        let (dir, path) = jpeg_in("unread");
        // XMP no XML reader takes (an unescaped ampersand).
        let mut xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>A & B</x:xmpmeta>".to_vec();
        let mut segments = vec![0xFF, 0xE1];
        segments.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        segments.append(&mut xmp);
        let jpeg = std::fs::read(&path).unwrap();
        std::fs::write(&path, container::replace_jpeg(&jpeg, &segments, container::Parts::ALL).unwrap()).unwrap();
        // A keyword, which XMP holds too: refused, the file left as it was.
        let keyword = serde_json::json!({ "iptc": { "2:25": ["sea"] } });
        let refused = rewrite(&path, |m| crate::metadata::json::changed(m, &keyword));
        assert!(refused.is_err(), "taken");
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(5).any(|w| w == b"A & B"), "the XMP that could not be read was lost");
        assert!(of_file(&path).unwrap().iptc.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn xmp_that_would_not_read_back_is_refused_not_written_as_none() {
        let (dir, path) = jpeg_in("unreadable");
        rewrite(&path, |m| {
            let mut x = Xmp::default();
            x.set_default_text(crate::metadata::xmp::DC, "title", "Kept");
            Ok(Metadata { xmp: Some(Arc::from(x.packet().into_bytes())), ..m.clone() })
        })
        .unwrap();
        let refused =
            rewrite(&path, |m| Ok(Metadata { xmp: Some(Arc::from(&b"<x:xmpmeta><unclosed"[..])), ..m.clone() }));
        assert!(refused.is_err(), "taken");
        let x = Xmp::read(of_file(&path).unwrap().xmp.as_deref().unwrap()).unwrap();
        assert_eq!(x.text(crate::metadata::xmp::DC, "title").as_deref(), Some("Kept"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
