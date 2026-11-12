//! EXIF: a TIFF structure of IFDs (image file directories), each a list of tagged values,
//! read and edited in place.
//!
//! The bytes are kept as they are and only grow. A value that fits where it was is written
//! there; one that does not goes at the end, its entry pointed there; an IFD that gains an
//! entry is written anew at the end and pointed to. What an edit leaves unreferenced is
//! zeroed, so that nothing removed survives in the bytes. So offsets counting from the start
//! of the structure that Opal Studio does not know of (a maker note's, which most cameras'
//! use) stay right through any edit, which writing the structure anew would break.

use std::collections::HashSet;

/// An IFD of the structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ifd {
    /// IFD0: the image's own (its description, the camera's make and model, the resolution).
    Image,
    /// The Exif IFD (pointed to by IFD0's tag 0x8769): how the photograph was taken.
    Exif,
    /// The GPS IFD (IFD0's 0x8825): where.
    Gps,
    /// The interoperability IFD (the Exif IFD's 0xA005).
    Interop,
    /// IFD1, the next after IFD0: the embedded thumbnail.
    Thumbnail,
}

impl Ifd {
    pub const ALL: [Ifd; 5] = [Ifd::Image, Ifd::Exif, Ifd::Gps, Ifd::Interop, Ifd::Thumbnail];

    /// Where the pointer to it is: the IFD and tag holding its offset; none for IFD0 (the
    /// header's) and IFD1 (IFD0's next).
    fn pointer(self) -> Option<(Ifd, u16)> {
        match self {
            Ifd::Exif => Some((Ifd::Image, EXIF_POINTER)),
            Ifd::Gps => Some((Ifd::Image, GPS_POINTER)),
            Ifd::Interop => Some((Ifd::Exif, INTEROP_POINTER)),
            Ifd::Image | Ifd::Thumbnail => None,
        }
    }

    /// Its name, as scripts give it.
    pub fn name(self) -> &'static str {
        match self {
            Ifd::Image => "image",
            Ifd::Exif => "exif",
            Ifd::Gps => "gps",
            Ifd::Interop => "interop",
            Ifd::Thumbnail => "thumbnail",
        }
    }
}

pub const EXIF_POINTER: u16 = 0x8769;
pub const GPS_POINTER: u16 = 0x8825;
pub const INTEROP_POINTER: u16 = 0xA005;
/// IFD1's thumbnail: where its JPEG starts, and how long it is.
const THUMBNAIL_AT: u16 = 0x0201;
const THUMBNAIL_LENGTH: u16 = 0x0202;
/// The tags whose value is the offset of an IFD: their values are the structure's, not data.
const POINTERS: [u16; 3] = [EXIF_POINTER, GPS_POINTER, INTEROP_POINTER];

/// A tag's value, by its TIFF type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// BYTE (1).
    Bytes(Vec<u8>),
    /// ASCII (2): text ending in a NUL. Read as UTF-8 where it is (as most writers now
    /// write it), else as Latin-1; written as UTF-8.
    Ascii(String),
    /// SHORT (3).
    Shorts(Vec<u16>),
    /// LONG (4).
    Longs(Vec<u32>),
    /// RATIONAL (5): numerator and denominator.
    Rationals(Vec<[u32; 2]>),
    /// SBYTE (6).
    SBytes(Vec<i8>),
    /// UNDEFINED (7): bytes whose meaning the tag says.
    Undefined(Vec<u8>),
    /// SSHORT (8).
    SShorts(Vec<i16>),
    /// SLONG (9).
    SLongs(Vec<i32>),
    /// SRATIONAL (10).
    SRationals(Vec<[i32; 2]>),
    /// FLOAT (11).
    Floats(Vec<f32>),
    /// DOUBLE (12).
    Doubles(Vec<f64>),
    /// UTF-8 (129, EXIF 3.0): text ending in a NUL.
    Utf8(String),
}

impl Value {
    /// Its TIFF type's number.
    pub fn kind(&self) -> u16 {
        match self {
            Value::Bytes(_) => 1,
            Value::Ascii(_) => 2,
            Value::Shorts(_) => 3,
            Value::Longs(_) => 4,
            Value::Rationals(_) => 5,
            Value::SBytes(_) => 6,
            Value::Undefined(_) => 7,
            Value::SShorts(_) => 8,
            Value::SLongs(_) => 9,
            Value::SRationals(_) => 10,
            Value::Floats(_) => 11,
            Value::Doubles(_) => 12,
            Value::Utf8(_) => 129,
        }
    }

    /// How many values of its type it is (the entry's count): text with its NUL.
    fn count(&self) -> usize {
        match self {
            Value::Bytes(v) | Value::Undefined(v) => v.len(),
            Value::Ascii(s) | Value::Utf8(s) => s.len() + 1,
            Value::Shorts(v) => v.len(),
            Value::Longs(v) => v.len(),
            Value::Rationals(v) => v.len(),
            Value::SBytes(v) => v.len(),
            Value::SShorts(v) => v.len(),
            Value::SLongs(v) => v.len(),
            Value::SRationals(v) => v.len(),
            Value::Floats(v) => v.len(),
            Value::Doubles(v) => v.len(),
        }
    }

    /// Its bytes, in the byte order `big` says (big-endian if true).
    fn encode(&self, big: bool) -> Vec<u8> {
        fn each<T: Copy, const N: usize>(v: &[T], big: bool, f: impl Fn(T) -> ([u8; N], [u8; N])) -> Vec<u8> {
            v.iter().flat_map(|&x| if big { f(x).0 } else { f(x).1 }).collect()
        }
        match self {
            Value::Bytes(v) | Value::Undefined(v) => v.clone(),
            Value::SBytes(v) => v.iter().map(|&b| b as u8).collect(),
            Value::Ascii(s) | Value::Utf8(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            Value::Shorts(v) => each(v, big, |x: u16| (x.to_be_bytes(), x.to_le_bytes())),
            Value::SShorts(v) => each(v, big, |x: i16| (x.to_be_bytes(), x.to_le_bytes())),
            Value::Longs(v) => each(v, big, |x: u32| (x.to_be_bytes(), x.to_le_bytes())),
            Value::SLongs(v) => each(v, big, |x: i32| (x.to_be_bytes(), x.to_le_bytes())),
            Value::Floats(v) => each(v, big, |x: f32| (x.to_be_bytes(), x.to_le_bytes())),
            Value::Doubles(v) => each(v, big, |x: f64| (x.to_be_bytes(), x.to_le_bytes())),
            Value::Rationals(v) => {
                let flat: Vec<u32> = v.iter().flatten().copied().collect();
                each(&flat, big, |x: u32| (x.to_be_bytes(), x.to_le_bytes()))
            }
            Value::SRationals(v) => {
                let flat: Vec<i32> = v.iter().flatten().copied().collect();
                each(&flat, big, |x: i32| (x.to_be_bytes(), x.to_le_bytes()))
            }
        }
    }

    /// The first of its numbers, as a float, if it is numbers.
    pub fn number(&self) -> Option<f64> {
        self.numbers().into_iter().next()
    }

    /// Its numbers, as floats (a rational divided out; one with a zero denominator left out).
    pub fn numbers(&self) -> Vec<f64> {
        match self {
            Value::Bytes(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::Shorts(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::Longs(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::SBytes(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::SShorts(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::SLongs(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::Floats(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Value::Doubles(v) => v.clone(),
            Value::Rationals(v) => {
                v.iter().filter(|[_, d]| *d != 0).map(|&[n, d]| f64::from(n) / f64::from(d)).collect()
            }
            Value::SRationals(v) => {
                v.iter().filter(|[_, d]| *d != 0).map(|&[n, d]| f64::from(n) / f64::from(d)).collect()
            }
            Value::Ascii(_) | Value::Utf8(_) | Value::Undefined(_) => Vec::new(),
        }
    }

    /// Its text, if it is text.
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Ascii(s) | Value::Utf8(s) => Some(s),
            _ => None,
        }
    }
}

/// The bytes a value of TIFF type `kind` takes, if the type is known.
fn type_size(kind: u16) -> Option<usize> {
    Some(match kind {
        1 | 2 | 6 | 7 | 129 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// An entry of an IFD, as it lies in the bytes.
#[derive(Clone, Copy, Debug)]
struct Entry {
    /// Where the entry's 12 bytes are.
    at: usize,
    tag: u16,
    kind: u16,
    count: u32,
    /// Where its value's bytes are: in the entry itself if they fit in 4, else at the
    /// offset it holds. None if its type is unknown, or its value lies past the end.
    value_at: Option<usize>,
    /// How many bytes its value takes.
    size: usize,
}

impl Entry {
    /// Whether its value lies outside the entry (at an offset).
    fn out_of_line(&self) -> bool {
        self.size > 4
    }
}

/// An EXIF structure (what follows "Exif\0\0" in a JPEG's APP1 segment, or fills a PNG's
/// eXIf chunk).
#[derive(Clone, Debug, PartialEq)]
pub struct Exif {
    bytes: Vec<u8>,
    /// Big-endian ("MM"), or little ("II").
    big: bool,
}

/// The most entries an IFD is taken to have: more is a damaged file.
const MOST_ENTRIES: usize = 4096;

impl Exif {
    /// Reads `bytes` as an EXIF structure: None if it is not one (its header wrong, or IFD0
    /// past its end).
    pub fn read(bytes: &[u8]) -> Option<Exif> {
        let big = match bytes.get(0..2)? {
            b"MM" => true,
            b"II" => false,
            _ => return None,
        };
        let exif = Exif { bytes: bytes.to_vec(), big };
        (exif.u16_at(2)? == 42 && exif.ifd_at(Ifd::Image).is_some()).then_some(exif)
    }

    /// A structure with an empty IFD0 and nothing else, little-endian.
    pub fn new() -> Exif {
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        // No entries, no next IFD.
        bytes.extend_from_slice(&[0; 6]);
        Exif { bytes, big: false }
    }

    /// Its bytes, as a file holds them.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }

    fn put_u16(&mut self, at: usize, v: u16) {
        let b = if self.big { v.to_be_bytes() } else { v.to_le_bytes() };
        self.bytes[at..at + 2].copy_from_slice(&b);
    }

    fn put_u32(&mut self, at: usize, v: u32) {
        let b = if self.big { v.to_be_bytes() } else { v.to_le_bytes() };
        self.bytes[at..at + 4].copy_from_slice(&b);
    }

    /// Where IFD `ifd` is, if the structure has it (and it lies within the bytes).
    fn ifd_at(&self, ifd: Ifd) -> Option<usize> {
        let at = match ifd {
            Ifd::Image => self.u32_at(4)? as usize,
            Ifd::Thumbnail => {
                let image = self.ifd_at(Ifd::Image)?;
                let n = usize::from(self.u16_at(image)?);
                self.u32_at(image + 2 + n * 12)? as usize
            }
            _ => {
                let (parent, tag) = ifd.pointer().expect("a pointed-to IFD");
                let e = self.entry(parent, tag)?;
                self.u32_at(e.at + 8)? as usize
            }
        };
        // An offset of 0 is none; one must leave room for the count.
        (at >= 8 && at + 2 <= self.bytes.len()).then_some(at)
    }

    /// The entries of IFD `ifd` (none if it has none), in the order they lie.
    fn entries(&self, ifd: Ifd) -> Vec<Entry> {
        let Some(at) = self.ifd_at(ifd) else { return Vec::new() };
        let n = usize::from(self.u16_at(at).unwrap_or(0)).min(MOST_ENTRIES);
        (0..n).map_while(|i| self.entry_at(at + 2 + i * 12)).collect()
    }

    /// The entry whose 12 bytes are at `at`.
    fn entry_at(&self, at: usize) -> Option<Entry> {
        let (tag, kind, count) = (self.u16_at(at)?, self.u16_at(at + 2)?, self.u32_at(at + 4)?);
        let size = type_size(kind).and_then(|s| s.checked_mul(count as usize));
        let value_at = size.and_then(|size| {
            let start = if size <= 4 { at + 8 } else { self.u32_at(at + 8)? as usize };
            (start.checked_add(size)? <= self.bytes.len()).then_some(start)
        });
        Some(Entry { at, tag, kind, count, value_at, size: size.unwrap_or(0) })
    }

    fn entry(&self, ifd: Ifd, tag: u16) -> Option<Entry> {
        self.entries(ifd).into_iter().find(|e| e.tag == tag)
    }

    /// The value of tag `tag` in `ifd`, if it is there and of a type known.
    pub fn get(&self, ifd: Ifd, tag: u16) -> Option<Value> {
        self.value(&self.entry(ifd, tag)?)
    }

    /// Every tag of `ifd` with its value (those of a type unknown, or lying past the end,
    /// left out), but the pointers to other IFDs, which are the structure's.
    pub fn tags(&self, ifd: Ifd) -> Vec<(u16, Value)> {
        self.entries(ifd)
            .iter()
            .filter(|e| !POINTERS.contains(&e.tag))
            .filter_map(|e| Some((e.tag, self.value(e)?)))
            .collect()
    }

    fn value(&self, e: &Entry) -> Option<Value> {
        let at = e.value_at?;
        let b = &self.bytes[at..at + e.size];
        let n = e.count as usize;
        let u16s = || (0..n).map(|i| self.u16_at(at + i * 2).unwrap_or(0));
        let u32s = |k: usize| (0..k).map(|i| self.u32_at(at + i * 4).unwrap_or(0)).collect::<Vec<u32>>();
        let text = |b: &[u8]| {
            let b = b.split(|&c| c == 0).next().unwrap_or_default();
            String::from_utf8(b.to_vec()).unwrap_or_else(|_| b.iter().map(|&c| char::from(c)).collect())
        };
        Some(match e.kind {
            1 => Value::Bytes(b.to_vec()),
            2 => Value::Ascii(text(b)),
            3 => Value::Shorts(u16s().collect()),
            4 | 13 => Value::Longs(u32s(n)),
            5 => Value::Rationals(u32s(n * 2).chunks_exact(2).map(|p| [p[0], p[1]]).collect()),
            6 => Value::SBytes(b.iter().map(|&x| x as i8).collect()),
            7 => Value::Undefined(b.to_vec()),
            8 => Value::SShorts(u16s().map(|x| x as i16).collect()),
            9 => Value::SLongs(u32s(n).into_iter().map(|x| x as i32).collect()),
            10 => Value::SRationals(u32s(n * 2).chunks_exact(2).map(|p| [p[0] as i32, p[1] as i32]).collect()),
            11 => Value::Floats(u32s(n).into_iter().map(f32::from_bits).collect()),
            12 => Value::Doubles(
                (0..n)
                    .map(|i| {
                        let (hi, lo) = (self.u32_at(at + i * 8).unwrap_or(0), self.u32_at(at + i * 8 + 4).unwrap_or(0));
                        let bits = if self.big {
                            (u64::from(hi) << 32) | u64::from(lo)
                        } else {
                            (u64::from(lo) << 32) | u64::from(hi)
                        };
                        f64::from_bits(bits)
                    })
                    .collect(),
            ),
            129 => Value::Utf8(text(b)),
            _ => return None,
        })
    }

    /// Appends `bytes` at the end, on an even offset as TIFF asks; where they went.
    fn append(&mut self, bytes: &[u8]) -> usize {
        if self.bytes.len() % 2 == 1 {
            self.bytes.push(0);
        }
        let at = self.bytes.len();
        self.bytes.extend_from_slice(bytes);
        at
    }

    /// Zeroes `len` bytes from `at` (what an edit has left unreferenced).
    fn zero(&mut self, at: usize, len: usize) {
        let end = (at + len).min(self.bytes.len());
        if at < end {
            self.bytes[at..end].fill(0);
        }
    }

    /// Sets tag `tag` of `ifd` to `value`, making the tag, and the IFD (pointed to from its
    /// parent, made too if need be), if they are not there. The thumbnail's IFD is not made.
    pub fn set(&mut self, ifd: Ifd, tag: u16, value: &Value) {
        if self.ifd_at(ifd).is_none() {
            if ifd == Ifd::Thumbnail {
                return;
            }
            self.make_ifd(ifd);
        }
        let encoded = value.encode(self.big);
        match self.entry(ifd, tag) {
            Some(e) => self.rewrite(e, value.kind(), value.count(), &encoded),
            None => self.add_entry(ifd, tag, value.kind(), value.count(), &encoded),
        }
    }

    /// Gives entry `e` the value `encoded` (of type `kind`, `count` of them): where its old
    /// value was if it fits there, else at the end; the old value's bytes zeroed.
    fn rewrite(&mut self, e: Entry, kind: u16, count: usize, encoded: &[u8]) {
        let old = e.value_at.filter(|_| e.out_of_line()).map(|at| (at, e.size));
        self.put_u16(e.at + 2, kind);
        self.put_u32(e.at + 4, count as u32);
        if encoded.len() <= 4 {
            if let Some((at, size)) = old {
                self.zero(at, size);
            }
            let mut inline = [0u8; 4];
            inline[..encoded.len()].copy_from_slice(encoded);
            self.bytes[e.at + 8..e.at + 12].copy_from_slice(&inline);
        } else if let Some((at, size)) = old.filter(|&(_, size)| encoded.len() <= size) {
            self.zero(at, size);
            self.bytes[at..at + encoded.len()].copy_from_slice(encoded);
        } else {
            if let Some((at, size)) = old {
                self.zero(at, size);
            }
            let at = self.append(encoded);
            self.put_u32(e.at + 8, at as u32);
        }
    }

    /// Adds tag `tag` to `ifd` (which is there): the IFD written anew at the end with it,
    /// in the order of their tags as TIFF asks, and pointed to; the old one zeroed.
    fn add_entry(&mut self, ifd: Ifd, tag: u16, kind: u16, count: usize, encoded: &[u8]) {
        let old_at = self.ifd_at(ifd).expect("there");
        let entries = self.entries(ifd);
        let next = self.u32_at(old_at + 2 + entries.len() * 12).unwrap_or(0);
        // The value first, if it lies outside its entry.
        let value_field = if encoded.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..encoded.len()].copy_from_slice(encoded);
            inline
        } else {
            let at = self.append(encoded) as u32;
            if self.big { at.to_be_bytes() } else { at.to_le_bytes() }
        };
        let mut raw: Vec<(u16, [u8; 12])> =
            entries.iter().map(|e| (e.tag, self.bytes[e.at..e.at + 12].try_into().expect("12 bytes"))).collect();
        let mut new = [0u8; 12];
        let (t, k, c) = if self.big {
            (tag.to_be_bytes(), kind.to_be_bytes(), (count as u32).to_be_bytes())
        } else {
            (tag.to_le_bytes(), kind.to_le_bytes(), (count as u32).to_le_bytes())
        };
        new[0..2].copy_from_slice(&t);
        new[2..4].copy_from_slice(&k);
        new[4..8].copy_from_slice(&c);
        new[8..12].copy_from_slice(&value_field);
        raw.push((tag, new));
        raw.sort_by_key(|&(tag, _)| tag);
        self.write_ifd(ifd, old_at, entries.len(), &raw.into_iter().map(|(_, e)| e).collect::<Vec<_>>(), next);
    }

    /// Writes IFD `ifd` anew at the end with `entries` (12 bytes each, values in place) and
    /// `next`, points to it, and zeroes the old one (`old_count` entries at `old_at`).
    fn write_ifd(&mut self, ifd: Ifd, old_at: usize, old_count: usize, entries: &[[u8; 12]], next: u32) {
        let mut b = Vec::with_capacity(2 + entries.len() * 12 + 4);
        let n = entries.len() as u16;
        b.extend_from_slice(&if self.big { n.to_be_bytes() } else { n.to_le_bytes() });
        for e in entries {
            b.extend_from_slice(e);
        }
        b.extend_from_slice(&if self.big { next.to_be_bytes() } else { next.to_le_bytes() });
        let at = self.append(&b) as u32;
        self.point_to(ifd, at);
        self.zero(old_at, 2 + old_count * 12 + 4);
    }

    /// Makes `ifd` point at `at` from where its pointer is.
    fn point_to(&mut self, ifd: Ifd, at: u32) {
        match ifd {
            Ifd::Image => self.put_u32(4, at),
            Ifd::Thumbnail => {
                let image = self.ifd_at(Ifd::Image).expect("IFD0");
                let n = usize::from(self.u16_at(image).unwrap_or(0));
                self.put_u32(image + 2 + n * 12, at);
            }
            _ => {
                let (parent, tag) = ifd.pointer().expect("a pointed-to IFD");
                match self.entry(parent, tag) {
                    Some(e) => {
                        self.put_u16(e.at + 2, 4);
                        self.put_u32(e.at + 4, 1);
                        self.put_u32(e.at + 8, at);
                    }
                    None => self.set(parent, tag, &Value::Longs(vec![at])),
                }
            }
        }
    }

    /// Makes `ifd`, empty, and points to it (making its parent if need be).
    fn make_ifd(&mut self, ifd: Ifd) {
        let at = self.append(&[0; 6]) as u32;
        if let Some((parent, tag)) = ifd.pointer() {
            self.set(parent, tag, &Value::Longs(vec![at]));
        } else {
            self.point_to(ifd, at);
        }
    }

    /// Removes tag `tag` from `ifd`, if it is there: the entries after it moved up in place,
    /// its value zeroed. A pointer removed takes its IFD with it (see
    /// [`remove_ifd`](Self::remove_ifd)).
    pub fn remove(&mut self, ifd: Ifd, tag: u16) {
        if let Some(pointed) = Ifd::ALL.into_iter().find(|i| i.pointer() == Some((ifd, tag))) {
            return self.remove_ifd(pointed);
        }
        self.remove_entry(ifd, tag);
    }

    fn remove_entry(&mut self, ifd: Ifd, tag: u16) {
        let Some(at) = self.ifd_at(ifd) else { return };
        let entries = self.entries(ifd);
        let Some(i) = entries.iter().position(|e| e.tag == tag) else { return };
        let e = entries[i];
        if let (Some(v), true) = (e.value_at, e.out_of_line()) {
            self.zero(v, e.size);
        }
        // The entries after it, and the next IFD's offset, up by one entry (unless the IFD
        // is cut short, which is left as it is).
        let n = entries.len();
        let from = at + 2 + (i + 1) * 12;
        let end = at + 2 + n * 12 + 4;
        if end > self.bytes.len() {
            return;
        }
        self.bytes.copy_within(from..end, from - 12);
        self.zero(end - 12, 12);
        self.put_u16(at, (n - 1) as u16);
    }

    /// Removes IFD `ifd`: its entries' values (and IFDs it points to, and the thumbnail's
    /// image) zeroed, the IFD itself zeroed, and the pointer to it removed (IFD0's next set
    /// to none, for the thumbnail's). IFD0 cannot be removed: it is emptied instead.
    pub fn remove_ifd(&mut self, ifd: Ifd) {
        let Some(at) = self.ifd_at(ifd) else { return };
        for child in Ifd::ALL.into_iter().filter(|c| c.pointer().is_some_and(|(p, _)| p == ifd)) {
            self.remove_ifd(child);
        }
        let entries = self.entries(ifd);
        if ifd == Ifd::Thumbnail {
            let at = self.get(ifd, THUMBNAIL_AT).and_then(|v| v.number());
            let len = self.get(ifd, THUMBNAIL_LENGTH).and_then(|v| v.number());
            if let (Some(at), Some(len)) = (at, len) {
                self.zero(at as usize, len as usize);
            }
        }
        for e in &entries {
            if let (Some(v), true) = (e.value_at, e.out_of_line()) {
                self.zero(v, e.size);
            }
        }
        let next = self.u32_at(at + 2 + entries.len() * 12).unwrap_or(0);
        match ifd {
            Ifd::Image => {
                // Emptied: no entries, its next kept.
                self.zero(at, 2 + entries.len() * 12 + 4);
                self.put_u16(at, 0);
                self.put_u32(at + 2, next);
            }
            Ifd::Thumbnail => {
                self.zero(at, 2 + entries.len() * 12 + 4);
                self.point_to(Ifd::Thumbnail, 0);
            }
            _ => {
                self.zero(at, 2 + entries.len() * 12 + 4);
                let (parent, tag) = ifd.pointer().expect("a pointed-to IFD");
                self.remove_entry(parent, tag);
            }
        }
    }

    /// Drops the bytes past the last that anything known refers to (zeroed by edits: the
    /// thumbnail's image, removed, usually lies last). What a maker note refers to is not
    /// known, so nothing is dropped from before the end of anything known.
    pub fn trim(&mut self) {
        let mut end = 8;
        let mut seen = HashSet::new();
        for ifd in Ifd::ALL {
            let Some(at) = self.ifd_at(ifd) else { continue };
            if !seen.insert(at) {
                continue;
            }
            let entries = self.entries(ifd);
            end = end.max(at + 2 + entries.len() * 12 + 4);
            for e in &entries {
                if let Some(v) = e.value_at {
                    end = end.max(v + e.size);
                }
            }
            if ifd == Ifd::Thumbnail {
                let at = self.get(ifd, THUMBNAIL_AT).and_then(|v| v.number());
                let len = self.get(ifd, THUMBNAIL_LENGTH).and_then(|v| v.number());
                if let (Some(at), Some(len)) = (at, len) {
                    end = end.max(at as usize + len as usize);
                }
            }
        }
        // Past the end, only zeros: what an edit left.
        if end < self.bytes.len() && self.bytes[end..].iter().all(|&b| b == 0) {
            self.bytes.truncate(end);
        }
    }
}

impl Default for Exif {
    fn default() -> Exif {
        Exif::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(s: &str) -> Value {
        Value::Ascii(s.to_owned())
    }

    #[test]
    fn values_are_set_read_and_made_where_they_go() {
        let mut e = Exif::new();
        e.set(Ifd::Image, 0x010F, &ascii("Canon"));
        e.set(Ifd::Image, 0x0112, &Value::Shorts(vec![6]));
        // Into the Exif IFD, made and pointed to from IFD0.
        e.set(Ifd::Exif, 0x829A, &Value::Rationals(vec![[1, 250]]));
        e.set(Ifd::Exif, 0x9003, &ascii("2024:05:06 14:03:22"));
        let read = Exif::read(e.bytes()).unwrap();
        assert_eq!(read.get(Ifd::Image, 0x010F), Some(ascii("Canon")));
        assert_eq!(read.get(Ifd::Image, 0x0112), Some(Value::Shorts(vec![6])));
        assert_eq!(read.get(Ifd::Exif, 0x829A).and_then(|v| v.number()), Some(0.004));
        // Its tags in the order of their numbers, the pointer not among them.
        let tags: Vec<u16> = read.tags(Ifd::Image).into_iter().map(|(t, _)| t).collect();
        assert_eq!(tags, [0x010F, 0x0112]);
        assert!(read.entries(Ifd::Image).windows(2).all(|w| w[0].tag < w[1].tag));
    }

    #[test]
    fn a_value_shorter_is_written_where_it_was_and_a_longer_one_at_the_end_the_old_zeroed() {
        let mut e = Exif::new();
        e.set(Ifd::Image, 0x010E, &ascii("A long description of it"));
        let len = e.bytes().len();
        e.set(Ifd::Image, 0x010E, &ascii("Shorter"));
        assert_eq!(e.bytes().len(), len, "not written in place");
        assert_eq!(e.get(Ifd::Image, 0x010E), Some(ascii("Shorter")));
        // Nothing of the old text past the new one's end.
        assert!(!e.bytes().windows(5).any(|w| w == b"of it"), "the old text's tail left");
        e.set(Ifd::Image, 0x010E, &ascii("Now longer than any before it was"));
        assert!(e.bytes().len() > len);
        assert_eq!(e.get(Ifd::Image, 0x010E), Some(ascii("Now longer than any before it was")));
        assert!(!e.bytes().windows(7).any(|w| w == b"Shorter"));
    }

    #[test]
    fn what_is_removed_leaves_nothing_behind() {
        let mut e = Exif::new();
        e.set(Ifd::Image, 0x010F, &ascii("Canon"));
        e.set(Ifd::Gps, 0x0001, &ascii("N"));
        e.set(Ifd::Gps, 0x0002, &Value::Rationals(vec![[48, 1], [51, 1], [2959, 100]]));
        e.set(Ifd::Exif, 0xA431, &ascii("SERIAL-12345"));
        e.set(Ifd::Exif, 0x829D, &Value::Rationals(vec![[28, 10]]));
        e.remove_ifd(Ifd::Gps);
        e.remove(Ifd::Exif, 0xA431);
        assert!(e.get(Ifd::Gps, 0x0001).is_none() && e.get(Ifd::Image, GPS_POINTER).is_none());
        assert!(e.get(Ifd::Exif, 0xA431).is_none());
        assert!(e.get(Ifd::Exif, 0x829D).is_some(), "another tag lost");
        assert!(!e.bytes().windows(6).any(|w| w == b"SERIAL"), "the serial number left in the bytes");
        let latitude = 2959u32.to_le_bytes();
        assert!(!e.bytes().windows(4).any(|w| w == latitude), "the latitude left in the bytes");
        assert_eq!(Exif::read(e.bytes()).unwrap().get(Ifd::Image, 0x010F), Some(ascii("Canon")));
    }

    /// A little-endian structure with a maker note holding an offset (from the structure's
    /// start, as Canon's do) to data of its own, and a thumbnail last.
    fn with_maker_note() -> (Exif, usize, usize) {
        let mut e = Exif::new();
        e.set(Ifd::Image, 0x010F, &ascii("Canon"));
        e.set(Ifd::Exif, 0x829A, &Value::Rationals(vec![[1, 60]]));
        // The maker's data, then the note pointing at it.
        let data_at = e.append(b"MAKER-DATA");
        let mut note = b"NOTE".to_vec();
        note.extend_from_slice(&(data_at as u32).to_le_bytes());
        e.set(Ifd::Exif, MAKER_NOTE_TAG, &Value::Undefined(note));
        let note_at = e.entry(Ifd::Exif, MAKER_NOTE_TAG).unwrap().value_at.unwrap();
        // IFD1 with its JPEG.
        let thumb_ifd = e.append(&[0; 6]);
        e.point_to(Ifd::Thumbnail, thumb_ifd as u32);
        let jpeg_at = e.append(&[0xFF, 0xD8, 1, 2, 3, 4, 0xFF, 0xD9]);
        e.set(Ifd::Thumbnail, THUMBNAIL_AT, &Value::Longs(vec![jpeg_at as u32]));
        e.set(Ifd::Thumbnail, THUMBNAIL_LENGTH, &Value::Longs(vec![8]));
        (e, note_at, data_at)
    }

    const MAKER_NOTE_TAG: u16 = 0x927C;

    #[test]
    fn a_maker_notes_offsets_stay_right_through_edits() {
        let (mut e, note_at, data_at) = with_maker_note();
        let note = e.get(Ifd::Exif, MAKER_NOTE_TAG).unwrap();
        // Tags added to its IFD and to IFD0 (both written anew), one changed, one removed.
        e.set(Ifd::Exif, 0xA002, &Value::Longs(vec![4000]));
        e.set(Ifd::Exif, 0x9003, &ascii("2024:05:06 14:03:22"));
        e.set(Ifd::Image, 0x0131, &ascii("Opal Studio"));
        e.set(Ifd::Image, 0x010F, &ascii("Canon Inc. and a longer name"));
        e.remove(Ifd::Exif, 0x829A);
        let e = Exif::read(e.bytes()).unwrap();
        assert_eq!(e.entry(Ifd::Exif, MAKER_NOTE_TAG).unwrap().value_at, Some(note_at), "the note moved");
        assert_eq!(e.get(Ifd::Exif, MAKER_NOTE_TAG), Some(note));
        assert_eq!(&e.bytes()[data_at..data_at + 10], b"MAKER-DATA", "what the note points at moved");
        assert_eq!(e.get(Ifd::Exif, 0xA002), Some(Value::Longs(vec![4000])));
        assert_eq!(e.get(Ifd::Image, 0x010F), Some(ascii("Canon Inc. and a longer name")));
    }

    #[test]
    fn the_thumbnail_removed_is_zeroed_and_trimmed_off_the_end() {
        let (mut e, ..) = with_maker_note();
        assert!(e.ifd_at(Ifd::Thumbnail).is_some());
        let before = e.bytes().len();
        e.remove_ifd(Ifd::Thumbnail);
        assert!(e.ifd_at(Ifd::Thumbnail).is_none());
        assert!(!e.bytes().windows(2).any(|w| w == [0xFF, 0xD8]), "the thumbnail's JPEG left");
        e.trim();
        assert!(e.bytes().len() < before, "nothing trimmed");
        assert_eq!(e.get(Ifd::Image, 0x010F), Some(ascii("Canon")));
    }

    #[test]
    fn a_big_endian_structure_reads_and_takes_edits_in_its_own_order() {
        // "MM", 42, IFD0 at 8: one entry, Orientation (SHORT) = 3.
        let mut b = b"MM\x00\x2a\x00\x00\x00\x08".to_vec();
        b.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
        let mut e = Exif::read(&b).unwrap();
        assert_eq!(e.get(Ifd::Image, 0x0112), Some(Value::Shorts(vec![3])));
        e.set(Ifd::Image, 0x011A, &Value::Rationals(vec![[300, 1]]));
        let e = Exif::read(e.bytes()).unwrap();
        assert_eq!(e.get(Ifd::Image, 0x011A).and_then(|v| v.number()), Some(300.0));
        assert_eq!(e.get(Ifd::Image, 0x0112), Some(Value::Shorts(vec![3])));
        assert!(Exif::read(b"II\x2a\x00\xff\xff\x00\x00").is_none(), "IFD0 past the end");
        assert!(Exif::read(b"XX").is_none());
    }
}
