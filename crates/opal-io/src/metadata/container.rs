//! Where JPEG and PNG files keep metadata, read and written: a JPEG's APP1 (EXIF, XMP and
//! Extended XMP) and APP13 (Photoshop's block, with the IIM) segments; a PNG's eXIf and iTXt
//! chunks, and ImageMagick's "Raw profile type" text chunks, which older writers use.

use super::{Raw, iptc};
use std::io::Read as _;

const EXIF_HEADER: &[u8] = b"Exif\0\0";
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const EXTENDED_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
/// The most a JPEG segment's data can be (its length counts its own two bytes).
const SEGMENT: usize = 65533;
/// The most of an Extended XMP packet one segment carries (after its header, the GUID, the
/// whole length and the offset).
const EXTENDED_CHUNK: usize = SEGMENT - 35 - 32 - 8;

/// Whether `bytes` begin as a JPEG's do.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
}

/// Whether `bytes` begin as a PNG's do.
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_SIGNATURE)
}

/// The metadata of a JPEG's segments (`bytes`, the whole file, or its header at least).
pub fn read_jpeg(bytes: &[u8]) -> Raw {
    let mut raw = Raw::default();
    // Extended XMP's chunks: each GUID's, with their offsets.
    let mut extended: Vec<(Vec<u8>, u32, Vec<u8>)> = Vec::new();
    for (marker, data) in jpeg_segments(bytes) {
        match marker {
            0xE1 if data.starts_with(EXIF_HEADER) && raw.exif.is_none() => {
                raw.exif = Some(data[EXIF_HEADER.len()..].to_vec());
            }
            // Some writers put a different byte after "Exif\0".
            0xE1 if data.starts_with(b"Exif\0") && raw.exif.is_none() => raw.exif = data.get(6..).map(<[u8]>::to_vec),
            0xE1 if data.starts_with(XMP_HEADER) && raw.xmp.is_none() => {
                raw.xmp = Some(data[XMP_HEADER.len()..].to_vec());
            }
            0xE1 if data.starts_with(EXTENDED_HEADER) => {
                let rest = &data[EXTENDED_HEADER.len()..];
                if rest.len() >= 40 {
                    let offset = u32::from_be_bytes([rest[36], rest[37], rest[38], rest[39]]);
                    extended.push((rest[..32].to_vec(), offset, rest[40..].to_vec()));
                }
            }
            _ => {}
        }
    }
    if let Some((iim, changed)) = iptc::from_resources(&photoshop_block(bytes)) {
        raw.iptc = Some(iim);
        raw.iptc_changed = changed;
    }
    // Extended XMP joined to the standard packet, if that names it (segments of another, a
    // packet since rewritten, are nothing a reader reads).
    if let Some(xmp) = &raw.xmp
        && let Some(guid) = extended_guid(xmp)
    {
        let mut chunks: Vec<(u32, Vec<u8>)> =
            extended.into_iter().filter(|(g, ..)| *g == guid).map(|(_, o, d)| (o, d)).collect();
        raw.extended_segments = !chunks.is_empty();
        chunks.sort_by_key(|(o, _)| *o);
        let mut joined = Vec::new();
        for (offset, data) in chunks {
            if offset as usize == joined.len() {
                joined.extend_from_slice(&data);
            }
        }
        if !joined.is_empty() {
            raw.extended_xmp = Some(joined);
        }
    }
    raw
}

/// The GUID the standard XMP packet `xmp` gives its extended packet, as bytes.
fn extended_guid(xmp: &[u8]) -> Option<Vec<u8>> {
    let x = super::xmp::Xmp::read(xmp)?;
    let guid = x.text(super::xmp::XMP_NOTE, "HasExtendedXMP")?;
    (guid.len() == 32).then(|| guid.into_bytes())
}

/// A JPEG's segments before its image data (the scan): each marker's second byte and its
/// data. Stops at what it cannot read.
fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, &[u8])> {
    jpeg_walk(bytes).0.into_iter().map(|s| (s.marker, s.data)).collect()
}

/// A JPEG segment: its marker's second byte, its data, and where it lies whole (its marker
/// and length included).
struct Segment<'a> {
    marker: u8,
    data: &'a [u8],
    at: usize,
    end: usize,
}

/// A JPEG's segments before its image data, and where the walk stopped (the scan, or what
/// could not be read): what follows, copied as it is by whatever rewrites the segments.
fn jpeg_walk(bytes: &[u8]) -> (Vec<Segment<'_>>, usize) {
    let mut out = Vec::new();
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return (out, 0);
    }
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF {
        let marker = bytes[at + 1];
        // Fill bytes before a marker.
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // Start of scan, or end of image: no more metadata.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let Some(data) = bytes.get(at + 4..at + 2 + len) else { break };
        if len < 2 {
            break;
        }
        out.push(Segment { marker, data, at, end: at + 2 + len });
        at += 2 + len;
    }
    (out, at)
}

/// The segments (each its marker and length included) that write `exif`, `xmp` (split into
/// the standard packet and Extended XMP if too large) and `photoshop` (Photoshop's image
/// resource block, which carries the IIM: see `iptc::to_resources` and `iptc::with_iim`)
/// into a JPEG. Err if EXIF is too large for its segment.
pub fn jpeg_segments_of(
    exif: Option<&[u8]>,
    xmp: Option<&super::xmp::Xmp>,
    photoshop: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut segment = |marker: u8, parts: &[&[u8]]| -> Result<(), String> {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        if len > SEGMENT {
            return Err(format!("{len} bytes are more than a JPEG segment holds ({SEGMENT})"));
        }
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&((len + 2) as u16).to_be_bytes());
        for p in parts {
            out.extend_from_slice(p);
        }
        Ok(())
    };
    if let Some(exif) = exif {
        segment(0xE1, &[EXIF_HEADER, exif]).map_err(|e| format!("EXIF: {e}"))?;
    }
    if let Some(xmp) = xmp {
        let (standard, extended) = xmp.split(SEGMENT - XMP_HEADER.len());
        segment(0xE1, &[XMP_HEADER, standard.as_bytes()])?;
        if let Some((guid, extended)) = extended {
            let whole = (extended.len() as u32).to_be_bytes();
            for (i, chunk) in extended.as_bytes().chunks(EXTENDED_CHUNK).enumerate() {
                let offset = ((i * EXTENDED_CHUNK) as u32).to_be_bytes();
                segment(0xE1, &[EXTENDED_HEADER, guid.as_bytes(), &whole, &offset, chunk])?;
            }
        }
    }
    if let Some(block) = photoshop {
        // Photoshop's block split across segments if it must be, each with the header.
        let body = block.strip_prefix(iptc::PHOTOSHOP).unwrap_or(block);
        for chunk in body.chunks(SEGMENT - iptc::PHOTOSHOP.len()) {
            segment(0xED, &[iptc::PHOTOSHOP, chunk])?;
        }
    }
    Ok(out)
}

/// Photoshop's image resource block in a JPEG's APP13 segments, joined, their headers taken
/// off (empty if it has none).
pub fn photoshop_block(jpeg: &[u8]) -> Vec<u8> {
    let mut block = Vec::new();
    for (marker, data) in jpeg_segments(jpeg) {
        if marker == 0xED && data.starts_with(iptc::PHOTOSHOP) {
            block.extend_from_slice(&data[iptc::PHOTOSHOP.len()..]);
        }
    }
    block
}

/// Whether a JPEG segment (its marker and data) carries metadata this module writes, which
/// writing it anew replaces.
fn is_metadata(marker: u8, data: &[u8], parts: Parts) -> bool {
    match marker {
        0xE1 => {
            (parts.exif && data.starts_with(b"Exif\0"))
                || (parts.xmp && (data.starts_with(XMP_HEADER) || data.starts_with(EXTENDED_HEADER)))
        }
        0xED => parts.iptc && data.starts_with(iptc::PHOTOSHOP),
        _ => false,
    }
}

/// `jpeg` (a whole file, or its start: SOI and the segments after it) with its metadata
/// segments of `parts` replaced by `segments` (see [`jpeg_segments_of`]): put after its EXIF
/// if that is kept (which readers look for first), else after JFIF's APP0 if it comes first,
/// else right after SOI. None if `jpeg` is not a JPEG.
pub fn replace_jpeg(jpeg: &[u8], segments: &[u8], parts: Parts) -> Option<Vec<u8>> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = vec![0xFF, 0xD8];
    let (walked, rest) = jpeg_walk(jpeg);
    let kept_exif = walked.iter().position(|s| !parts.exif && s.marker == 0xE1 && s.data.starts_with(EXIF_HEADER));
    let jfif = walked.first().is_some_and(|s| s.marker == 0xE0 && s.data.starts_with(b"JFIF\0"));
    let after = kept_exif.map_or(usize::from(jfif), |i| i + 1);
    for (i, s) in walked.iter().enumerate() {
        if i == after {
            out.extend_from_slice(segments);
        }
        if !is_metadata(s.marker, s.data, parts) {
            out.extend_from_slice(&jpeg[s.at..s.end]);
        }
    }
    if after >= walked.len() {
        out.extend_from_slice(segments);
    }
    out.extend_from_slice(&jpeg[rest..]);
    Some(out)
}

/// A writer passing a JPEG through to `out` with its metadata segments replaced by
/// `segments`: it holds what comes until the segments before the image data are all there
/// (an encoder writes them first, and they are small), then writes them replaced, and passes
/// the rest straight on.
pub struct JpegWithMetadata<W: std::io::Write> {
    out: W,
    segments: Vec<u8>,
    /// What is held, until the header is whole; None once it has been written.
    held: Option<Vec<u8>>,
}

impl<W: std::io::Write> JpegWithMetadata<W> {
    pub fn new(out: W, segments: Vec<u8>) -> Self {
        JpegWithMetadata { out, segments, held: Some(Vec::new()) }
    }

    /// Where the segments before the image data end in `b`, if they are all there.
    fn header_end(b: &[u8]) -> Option<usize> {
        let mut at = 2;
        loop {
            if at + 2 > b.len() {
                return None;
            }
            let marker = b[at + 1];
            // An APPn or a comment: metadata's place; anything else begins the image's own.
            if !(0xE0..=0xEF).contains(&marker) && marker != 0xFE {
                return Some(at);
            }
            if at + 4 > b.len() {
                return None;
            }
            at += 2 + usize::from(u16::from_be_bytes([b[at + 2], b[at + 3]]));
        }
    }

    /// Writes what is held as it is, if the header never became whole (a JPEG too short).
    pub fn finish(mut self) -> std::io::Result<W> {
        if let Some(held) = self.held.take() {
            self.out.write_all(&held)?;
        }
        Ok(self.out)
    }
}

impl<W: std::io::Write> std::io::Write for JpegWithMetadata<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let Some(held) = self.held.as_mut() else { return self.out.write(buf) };
        held.extend_from_slice(buf);
        if let Some(end) = Self::header_end(held) {
            let held = self.held.take().expect("held");
            let header = replace_jpeg(&held[..end], &self.segments, Parts::ALL)
                .ok_or_else(|| std::io::Error::other("the encoder wrote no JPEG"))?;
            self.out.write_all(&header)?;
            self.out.write_all(&held[end..])?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const XMP_KEYWORD: &str = "XML:com.adobe.xmp";
/// The text chunks' keywords whose text is metadata read.
const READ_KEYWORDS: [&str; 6] = [
    XMP_KEYWORD,
    "Raw profile type exif",
    "Raw profile type APP1",
    "Raw profile type xmp",
    "Raw profile type iptc",
    "Raw profile type 8bim",
];

/// Which parts of a file's metadata writing replaces; those it does not are kept as the file
/// has them (what Opal Studio could not read among them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    pub exif: bool,
    pub xmp: bool,
    pub iptc: bool,
}

impl Parts {
    pub const ALL: Parts = Parts { exif: true, xmp: true, iptc: true };
}

/// A PNG's chunks: each one's type, data, and where it starts (its length field).
fn png_chunks(bytes: &[u8]) -> Vec<([u8; 4], &[u8], usize)> {
    let mut out = Vec::new();
    if !bytes.starts_with(PNG_SIGNATURE) {
        return out;
    }
    let mut at = PNG_SIGNATURE.len();
    while at + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let kind = [bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]];
        // Its data and its CRC both within the file.
        if (at + 12).saturating_add(len) > bytes.len() {
            break;
        }
        let data = &bytes[at + 8..at + 8 + len];
        out.push((kind, data, at));
        at += 12 + len;
        if &kind == b"IEND" {
            break;
        }
    }
    out
}

/// A text chunk's keyword and text (tEXt, zTXt, iTXt), its text inflated if compressed.
fn png_text(kind: &[u8; 4], data: &[u8]) -> Option<(String, Vec<u8>)> {
    let nul = data.iter().position(|&b| b == 0)?;
    let keyword: String = data[..nul].iter().map(|&c| char::from(c)).collect();
    // Other text is not metadata Opal Studio reads: not inflated for nothing.
    if !READ_KEYWORDS.contains(&keyword.as_str()) {
        return None;
    }
    let rest = &data[nul + 1..];
    let inflate = |b: &[u8]| {
        let mut out = Vec::new();
        // Text past 64 MB is no metadata Opal Studio keeps.
        flate2::read::ZlibDecoder::new(b).take(64 << 20).read_to_end(&mut out).ok().map(|_| out)
    };
    let text = match kind {
        b"tEXt" => rest.to_vec(),
        b"zTXt" => inflate(rest.get(1..)?)?,
        b"iTXt" => {
            let (compressed, rest) = (*rest.first()?, rest.get(2..)?);
            // The language tag and the translated keyword, each ending in a NUL.
            let lang = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[lang + 1..];
            let translated = rest.iter().position(|&b| b == 0)?;
            let text = &rest[translated + 1..];
            if compressed == 1 { inflate(text)? } else { text.to_vec() }
        }
        _ => return None,
    };
    Some((keyword, text))
}

/// ImageMagick's "Raw profile type" text: a line with the type, one with the length, and
/// the bytes in hex, broken into lines.
fn raw_profile(text: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(text).ok()?;
    let mut lines = text.trim_start().splitn(3, '\n');
    let _kind = lines.next()?;
    let len: usize = lines.next()?.trim().parse().ok()?;
    let hex: Vec<u8> = lines.next()?.bytes().filter(u8::is_ascii_hexdigit).collect();
    let digit = |c: u8| (c as char).to_digit(16).expect("a hex digit") as u8;
    let bytes: Vec<u8> = hex.chunks_exact(2).map(|p| digit(p[0]) << 4 | digit(p[1])).collect();
    (bytes.len() >= len).then(|| bytes[..len].to_vec())
}

/// The metadata of a PNG's chunks (`bytes`, the whole file).
pub fn read_png(bytes: &[u8]) -> Raw {
    let mut raw = Raw::default();
    for (kind, data, _) in png_chunks(bytes) {
        match &kind {
            b"eXIf" if raw.exif.is_none() => {
                raw.exif = Some(data.strip_prefix(EXIF_HEADER).unwrap_or(data).to_vec());
            }
            b"tEXt" | b"zTXt" | b"iTXt" => {
                let Some((keyword, text)) = png_text(&kind, data) else { continue };
                match keyword.as_str() {
                    XMP_KEYWORD if raw.xmp.is_none() => raw.xmp = Some(text),
                    "Raw profile type exif" | "Raw profile type APP1" if raw.exif.is_none() => {
                        if let Some(b) = raw_profile(&text) {
                            raw.exif = Some(b.strip_prefix(EXIF_HEADER).unwrap_or(&b).to_vec());
                        }
                    }
                    "Raw profile type xmp" if raw.xmp.is_none() => raw.xmp = raw_profile(&text),
                    "Raw profile type iptc" | "Raw profile type 8bim" if raw.iptc.is_none() => {
                        let Some(b) = raw_profile(&text) else { continue };
                        if b.starts_with(b"8BIM") {
                            if let Some((iim, changed)) = iptc::from_resources(&b) {
                                (raw.iptc, raw.iptc_changed) = (Some(iim), changed);
                            }
                        } else if b.first() == Some(&0x1C) {
                            raw.iptc = Some(b);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    raw
}

/// A PNG chunk of type `kind` holding `data`, its length and CRC included.
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
    out
}

/// The chunks (each its type and data) that write `exif`, `xmp` and `iptc` (the IIM, as
/// ImageMagick writes it) into a PNG: eXIf, an uncompressed iTXt, and a tEXt.
pub fn png_chunks_of(
    exif: Option<&[u8]>,
    xmp: Option<&super::xmp::Xmp>,
    iptc: Option<&[u8]>,
) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out = Vec::new();
    if let Some(exif) = exif {
        out.push((*b"eXIf", exif.to_vec()));
    }
    if let Some(xmp) = xmp {
        // Keyword, no compression, no language, no translated keyword.
        let mut data = XMP_KEYWORD.as_bytes().to_vec();
        data.extend_from_slice(&[0, 0, 0, 0, 0]);
        data.extend_from_slice(xmp.packet().as_bytes());
        out.push((*b"iTXt", data));
    }
    if let Some(iim) = iptc {
        out.push((*b"tEXt", raw_profile_data("iptc", iim)));
    }
    out
}

/// The data of a tEXt chunk holding `bytes` as ImageMagick's "Raw profile type" `kind` (see
/// [`raw_profile`]).
fn raw_profile_data(kind: &str, bytes: &[u8]) -> Vec<u8> {
    let mut text = format!("\n{kind}\n{:8}\n", bytes.len());
    for line in bytes.chunks(36) {
        for b in line {
            text.push_str(&format!("{b:02x}"));
        }
        text.push('\n');
    }
    let mut data = format!("Raw profile type {kind}").into_bytes();
    data.push(0);
    data.extend_from_slice(text.as_bytes());
    data
}

/// Photoshop's resources in a "Raw profile type 8bim" chunk (`kind`, `data`), if they hold an
/// IIM: the others, the IIM and its digest taken out (none, if that was all).
fn photoshop_without_iim(kind: &[u8; 4], data: &[u8]) -> Option<Vec<u8>> {
    // (Every text chunk begins with its keyword: no other is inflated or decoded for this.)
    if !data.starts_with(b"Raw profile type 8bim\0") {
        return None;
    }
    let (keyword, text) = png_text(kind, data)?;
    let block = raw_profile(&text).filter(|_| keyword == "Raw profile type 8bim")?;
    iptc::from_resources(&block)?;
    Some(iptc::with_iim(&block, None)[iptc::PHOTOSHOP.len()..].to_vec())
}

/// `chunks` as a file holds them, each with its length and CRC.
pub fn png_bytes(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    chunks.iter().flat_map(|(kind, data)| png_chunk(kind, data)).collect()
}

/// Whether a PNG chunk carries metadata this module writes, which writing it anew replaces.
fn png_is_metadata(kind: &[u8; 4], data: &[u8], parts: Parts) -> bool {
    match kind {
        b"eXIf" => parts.exif,
        b"tEXt" | b"zTXt" | b"iTXt" => match data.split(|&b| b == 0).next().unwrap_or_default() {
            b"Raw profile type exif" | b"Raw profile type APP1" => parts.exif,
            b"Raw profile type xmp" => parts.xmp,
            k if k == XMP_KEYWORD.as_bytes() => parts.xmp,
            b"Raw profile type iptc" => parts.iptc,
            // Photoshop's resources (paths), a color profile: not the parts written.
            _ => false,
        },
        _ => false,
    }
}

/// `png` (a whole file) with its metadata chunks replaced by `chunks` (see
/// [`png_chunks_of`]), put right after the header chunk (IHDR), before the image data as
/// eXIf must be. None if `png` is not a PNG.
pub fn replace_png(png: &[u8], chunks: &[u8], parts: Parts) -> Option<Vec<u8>> {
    let all = png_chunks(png);
    if all.first().is_none_or(|(k, ..)| k != b"IHDR") {
        return None;
    }
    let mut out = PNG_SIGNATURE.to_vec();
    let mut end = PNG_SIGNATURE.len();
    for (i, (kind, data, at)) in all.iter().enumerate() {
        let whole = &png[*at..at + 12 + data.len()];
        if !png_is_metadata(kind, data, parts) {
            match parts.iptc.then(|| photoshop_without_iim(kind, data)).flatten() {
                // Photoshop's resources that held the IIM, written in a chunk of its own: the
                // others kept.
                Some(others) if others.is_empty() => {}
                Some(others) => out.extend(png_chunk(b"tEXt", &raw_profile_data("8bim", &others))),
                None => out.extend_from_slice(whole),
            }
        }
        if i == 0 {
            out.extend_from_slice(chunks);
        }
        end = at + 12 + data.len();
    }
    out.extend_from_slice(&png[end..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::xmp::{DC, Xmp};

    /// A small JPEG, as the encoder writes one (with JFIF's APP0 first).
    fn jpeg() -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbImage::from_pixel(9, 7, image::Rgb([10, 200, 30]));
        image::codecs::jpeg::JpegEncoder::new(&mut out).encode_image(&img).unwrap();
        out
    }

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbaImage::from_pixel(9, 7, image::Rgba([10, 200, 30, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn sample() -> (Vec<u8>, Xmp, Vec<u8>) {
        let mut exif = crate::metadata::exif::Exif::new();
        exif.set(crate::metadata::exif::Ifd::Image, 0x010F, &crate::metadata::exif::Value::Ascii("Camera".into()));
        let mut xmp = Xmp::default();
        xmp.set_default_text(DC, "title", "Harbour");
        let mut iim = iptc::Iim::default();
        iim.set_texts(2, 5, &["Harbour".into()]);
        (exif.into_bytes(), xmp, iim.bytes())
    }

    #[test]
    fn a_jpeg_takes_its_metadata_after_jfif_and_gives_it_back_its_pixels_untouched() {
        let (exif, xmp, iim) = sample();
        let segments = jpeg_segments_of(Some(&exif), Some(&xmp), Some(&iptc::to_resources(&iim))).unwrap();
        let plain = jpeg();
        let with = replace_jpeg(&plain, &segments, Parts::ALL).unwrap();
        let raw = read_jpeg(&with);
        assert_eq!(raw.exif.as_deref(), Some(&exif[..]));
        assert_eq!(Xmp::read(raw.xmp.as_deref().unwrap()).unwrap().text(DC, "title").as_deref(), Some("Harbour"));
        assert_eq!((raw.iptc.as_deref(), raw.iptc_changed), (Some(&iim[..]), false));
        // JFIF first still; the pixels the same.
        assert_eq!(&with[2..4], &[0xFF, 0xE0]);
        let decoded = |b: &[u8]| image::load_from_memory(b).unwrap().to_rgb8();
        assert_eq!(decoded(&with), decoded(&plain));
        // Written again, replaced, not doubled; and none: gone.
        let again = replace_jpeg(&with, &segments, Parts::ALL).unwrap();
        assert_eq!(again, with);
        assert_eq!(replace_jpeg(&with, &[], Parts::ALL).unwrap(), plain);
        // Streamed through the writer, a few bytes at a time: the same.
        let mut w = JpegWithMetadata::new(Vec::new(), segments);
        for chunk in plain.chunks(7) {
            std::io::Write::write_all(&mut w, chunk).unwrap();
        }
        assert_eq!(w.finish().unwrap(), with);
    }

    #[test]
    fn extended_xmp_is_split_across_segments_and_joined_on_reading() {
        let mut xmp = Xmp::default();
        let many: Vec<String> = (0..12_000).map(|i| format!("keyword {i}")).collect();
        xmp.set_texts(DC, "subject", crate::metadata::xmp::ArrayKind::Bag, &many);
        let with = replace_jpeg(&jpeg(), &jpeg_segments_of(None, Some(&xmp), None).unwrap(), Parts::ALL).unwrap();
        let raw = read_jpeg(&with);
        let mut read = Xmp::read(raw.xmp.as_deref().unwrap()).unwrap();
        read.join(Xmp::read(raw.extended_xmp.as_deref().unwrap()).unwrap());
        assert_eq!(read.texts(DC, "subject").len(), 12_000);
        assert!(raw.extended_segments);
        // The standard packet naming another GUID (rewritten by a program that left the old
        // extended segments): those segments are nobody's, and not counted as unread.
        let guid = extended_guid(raw.xmp.as_deref().unwrap()).unwrap();
        let at = with.windows(guid.len()).position(|w| w == guid).unwrap();
        let mut orphaned = with.clone();
        orphaned[at..at + guid.len()].copy_from_slice(&[b'0'; 32]);
        let raw = read_jpeg(&orphaned);
        assert!(raw.xmp.is_some());
        assert_eq!((raw.extended_segments, raw.extended_xmp), (false, None));
    }

    #[test]
    fn a_png_takes_its_metadata_after_its_header_and_reads_imagemagicks_too() {
        let (exif, xmp, iim) = sample();
        let plain = png();
        let with =
            replace_png(&plain, &png_bytes(&png_chunks_of(Some(&exif), Some(&xmp), Some(&iim))), Parts::ALL).unwrap();
        let raw = read_png(&with);
        assert_eq!(raw.exif.as_deref(), Some(&exif[..]));
        assert!(Xmp::read(raw.xmp.as_deref().unwrap()).is_some());
        assert_eq!(raw.iptc.as_deref(), Some(&iim[..]));
        let decoded = |b: &[u8]| image::load_from_memory(b).unwrap().to_rgba8();
        assert_eq!(decoded(&with), decoded(&plain));
        assert_eq!(replace_png(&with, &[], Parts::ALL).unwrap(), plain);
        // ImageMagick's EXIF, compressed, with its header.
        let mut text = b"\nexif\n      14\n".to_vec();
        let mut payload = EXIF_HEADER.to_vec();
        payload.extend_from_slice(&exif[..8]);
        for b in &payload {
            text.extend_from_slice(format!("{b:02x}").as_bytes());
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &text).unwrap();
        let mut data = b"Raw profile type exif\0\0".to_vec();
        data.extend(z.finish().unwrap());
        let magick = replace_png(&plain, &png_chunk(b"zTXt", &data), Parts::ALL).unwrap();
        assert_eq!(read_png(&magick).exif.as_deref(), Some(&exif[..8]));
    }

    #[test]
    fn a_damaged_file_is_read_as_far_as_it_can_be_and_nothing_panics() {
        // An APP1 of "Exif" and its NUL alone.
        let plain = jpeg();
        let mut short = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x07];
        short.extend_from_slice(b"Exif\0");
        short.extend_from_slice(&plain[2..]);
        assert_eq!(read_jpeg(&short).exif, None);
        // A PNG cut off within a chunk's CRC: that chunk is none.
        let (exif, ..) = sample();
        let with = replace_png(&png(), &png_bytes(&png_chunks_of(Some(&exif), None, None)), Parts::ALL).unwrap();
        let at = with.windows(4).position(|w| w == b"eXIf").unwrap() - 4;
        let cut = &with[..at + 12 + exif.len() - 2];
        assert_eq!(read_png(cut).exif, None);
        assert!(replace_png(cut, &[], Parts::ALL).is_some());
    }

    /// What a PNG's "Raw profile type 8bim" chunk holds (Photoshop's resources), if it has one.
    fn eight_bim(png: &[u8]) -> Option<Vec<u8>> {
        png_chunks(png).into_iter().find_map(|(kind, data, _)| {
            let (keyword, text) = png_text(&kind, data)?;
            (keyword == "Raw profile type 8bim").then(|| raw_profile(&text)).flatten()
        })
    }

    #[test]
    fn the_iim_among_photoshops_resources_in_a_png_is_replaced_and_the_others_kept() {
        // Photoshop's resources as ImageMagick keeps them in a PNG: a path, and the IIM.
        let mut path = b"8BIM".to_vec();
        path.extend_from_slice(&2000u16.to_be_bytes());
        path.extend_from_slice(&[0, 0, 0, 0, 0, 4, 1, 2, 3, 4]);
        let mut old = iptc::Iim::default();
        old.set_texts(2, 5, &["Old".into()]);
        let block = iptc::with_iim(&path, Some(&old.bytes()))[iptc::PHOTOSHOP.len()..].to_vec();
        let chunk = png_bytes(&[(*b"tEXt", raw_profile_data("8bim", &block))]);
        let none = Parts { exif: false, xmp: false, iptc: false };
        let with = replace_png(&png(), &chunk, none).unwrap();
        assert_eq!(read_png(&with).iptc, Some(old.bytes()));
        let only_iim = Parts { iptc: true, ..none };
        // A new IIM is read back, not the old; and none, none.
        let mut new = iptc::Iim::default();
        new.set_texts(2, 5, &["New".into()]);
        let edited = replace_png(&with, &png_bytes(&png_chunks_of(None, None, Some(&new.bytes()))), only_iim).unwrap();
        let removed = replace_png(&with, &[], only_iim).unwrap();
        assert_eq!(read_png(&edited).iptc, Some(new.bytes()));
        assert_eq!(read_png(&removed).iptc, None);
        // The path kept in both; the IIM no longer among the resources.
        for (file, what) in [(&edited, "edited"), (&removed, "removed")] {
            let kept = eight_bim(file).unwrap_or_else(|| panic!("the resources {what} with the IIM"));
            assert_eq!(kept, path, "{what}");
        }
        // Resources holding only the IIM go with it.
        let alone = iptc::to_resources(&old.bytes())[iptc::PHOTOSHOP.len()..].to_vec();
        let lone = replace_png(&png(), &png_bytes(&[(*b"tEXt", raw_profile_data("8bim", &alone))]), none).unwrap();
        assert_eq!(eight_bim(&replace_png(&lone, &[], only_iim).unwrap()), None);
    }

    #[test]
    fn xmp_written_alone_goes_after_the_exif_kept_which_readers_look_for_first() {
        let (exif, xmp, _) = sample();
        let with_exif = replace_jpeg(&jpeg(), &jpeg_segments_of(Some(&exif), None, None).unwrap(), Parts::ALL).unwrap();
        let only_xmp = Parts { exif: false, xmp: true, iptc: false };
        let with_both = replace_jpeg(&with_exif, &jpeg_segments_of(None, Some(&xmp), None).unwrap(), only_xmp).unwrap();
        let app1: Vec<&[u8]> =
            jpeg_walk(&with_both).0.iter().filter(|s| s.marker == 0xE1).map(|s| &s.data[..4]).collect();
        assert_eq!(app1, [&b"Exif"[..], b"http"], "the XMP put before the EXIF");
        assert_eq!(read_jpeg(&with_both).exif.as_deref(), Some(&exif[..]));
    }
}
