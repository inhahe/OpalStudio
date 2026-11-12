//! IPTC IIM: the datasets of the IPTC's Information Interchange Model (title, caption,
//! creator, copyright, keywords, place...), and Photoshop's image resource block, which
//! carries them in a JPEG with a digest telling whether they still match the XMP.

use md5::{Digest, Md5};

/// One dataset: its record and number (2:120 is record 2's caption), and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSet {
    pub record: u8,
    pub number: u8,
    pub data: Vec<u8>,
}

/// The datasets of an IIM block, in the order they lie.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Iim {
    pub sets: Vec<DataSet>,
}

/// 1:90, the coded character set; the escape sequence it holds for UTF-8.
const CHARSET: (u8, u8) = (1, 90);
const UTF8: &[u8] = b"\x1b%G";
/// 2:00, the record version, which comes first in record 2.
const RECORD_VERSION: (u8, u8) = (2, 0);

/// Whether record 2's dataset `number` holds text: all but its version (2:00) and the
/// rasterized caption and preview's (2:125, 2:200 to 2:202), which are binary.
fn is_text(number: u8) -> bool {
    !matches!(number, 0 | 125 | 200..=202)
}

impl Iim {
    /// Reads `bytes` as IIM datasets: None if they are not (a dataset not starting with its
    /// tag marker, or running past the end).
    pub fn read(bytes: &[u8]) -> Option<Iim> {
        let mut sets = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            // Padding at the end (Photoshop's).
            if bytes[at..].iter().all(|&b| b == 0) {
                break;
            }
            let head = bytes.get(at..at + 5)?;
            if head[0] != 0x1C {
                return None;
            }
            let (record, number) = (head[1], head[2]);
            let mut len = usize::from(u16::from_be_bytes([head[3], head[4]]));
            at += 5;
            // The extended form: the high bit set, the rest the length of the length.
            if len & 0x8000 != 0 {
                let n = len & 0x7FFF;
                if n > 4 {
                    return None;
                }
                len = bytes.get(at..at + n)?.iter().fold(0, |l, &b| (l << 8) | usize::from(b));
                at += n;
            }
            let data = bytes.get(at..at.checked_add(len)?)?.to_vec();
            at += len;
            sets.push(DataSet { record, number, data });
        }
        Some(Iim { sets })
    }

    /// Its bytes, as a file holds them.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for s in &self.sets {
            out.extend_from_slice(&[0x1C, s.record, s.number]);
            if s.data.len() < 0x8000 {
                out.extend_from_slice(&(s.data.len() as u16).to_be_bytes());
            } else {
                out.extend_from_slice(&0x8004u16.to_be_bytes());
                out.extend_from_slice(&(s.data.len() as u32).to_be_bytes());
            }
            out.extend_from_slice(&s.data);
        }
        out
    }

    /// Whether its text is UTF-8, as 1:90 says (else it is taken as Latin-1, unless it reads
    /// as UTF-8, as the Metadata Working Group advises).
    fn utf8(&self) -> bool {
        self.sets.iter().any(|s| (s.record, s.number) == CHARSET && s.data == UTF8)
    }

    /// The texts of dataset `record:number` (several, for a repeatable one: keywords,
    /// creators).
    pub fn texts(&self, record: u8, number: u8) -> Vec<String> {
        let utf8 = self.utf8();
        self.sets
            .iter()
            .filter(|s| (s.record, s.number) == (record, number))
            .map(|s| {
                let b = s.data.strip_suffix(&[0]).unwrap_or(&s.data);
                match std::str::from_utf8(b) {
                    Ok(t) => t.to_owned(),
                    Err(_) if utf8 => String::from_utf8_lossy(b).into_owned(),
                    Err(_) => b.iter().map(|&c| char::from(c)).collect(),
                }
            })
            .collect()
    }

    /// Makes dataset `record:number` hold `texts` (none: removed), where datasets of its
    /// number go: in the order of their numbers within their record. Text not in ASCII makes
    /// the block say it is UTF-8 (1:90), if it did not.
    pub fn set_texts(&mut self, record: u8, number: u8, texts: &[String]) {
        if texts.iter().any(|t| !t.is_ascii()) && !self.utf8() {
            // The texts there already, Latin-1 until now, are made UTF-8 too, or they would read
            // wrong once the block says it is UTF-8.
            for s in &mut self.sets {
                if s.record == 2 && is_text(s.number) && std::str::from_utf8(&s.data).is_err() {
                    s.data = s.data.iter().map(|&c| char::from(c)).collect::<String>().into_bytes();
                }
            }
            self.sets.retain(|s| (s.record, s.number) != CHARSET);
            self.insert(DataSet { record: CHARSET.0, number: CHARSET.1, data: UTF8.to_vec() });
        }
        let at = self.sets.iter().position(|s| (s.record, s.number) == (record, number));
        self.sets.retain(|s| (s.record, s.number) != (record, number));
        let new = texts.iter().map(|t| DataSet { record, number, data: t.as_bytes().to_vec() });
        match at {
            Some(at) => {
                let tail = self.sets.split_off(at);
                self.sets.extend(new);
                self.sets.extend(tail);
            }
            None => {
                // Record 2 begins with its version (4 being the one in use).
                if record == 2 && !self.sets.iter().any(|s| (s.record, s.number) == RECORD_VERSION) && !texts.is_empty()
                {
                    self.insert(DataSet { record: 2, number: 0, data: vec![0, 4] });
                }
                for s in new {
                    self.insert(s);
                }
            }
        }
    }

    /// Puts `set` after every dataset of a lower record, or of its record and a number no
    /// higher.
    fn insert(&mut self, set: DataSet) {
        let at =
            self.sets.iter().position(|s| (s.record, s.number) > (set.record, set.number)).unwrap_or(self.sets.len());
        self.sets.insert(at, set);
    }

    /// Removes every dataset `keep` refuses.
    pub fn retain(&mut self, keep: impl Fn(u8, u8) -> bool) {
        self.sets.retain(|s| keep(s.record, s.number));
    }

    /// Whether it holds anything but its bookkeeping (the character set, the record version).
    pub fn is_empty(&self) -> bool {
        self.sets.iter().all(|s| [CHARSET, RECORD_VERSION].contains(&(s.record, s.number)))
    }
}

/// The header of Photoshop's image resource block in a JPEG's APP13 segments.
pub const PHOTOSHOP: &[u8] = b"Photoshop 3.0\0";
/// Its resources: the IIM, and the MD5 digest of the IIM as the program that last wrote the
/// XMP left it.
const IIM_RESOURCE: u16 = 0x0404;
const DIGEST_RESOURCE: u16 = 0x0425;

/// What Photoshop's image resource block (`block`, its header taken off) holds of IPTC: the
/// IIM's bytes, and whether its digest says a program unaware of XMP has changed it since the
/// XMP was written (true if there is a digest and it is not the IIM's).
pub fn from_resources(block: &[u8]) -> Option<(Vec<u8>, bool)> {
    let (mut iim, mut digest) = (None, None);
    for r in resources(block) {
        match r.id {
            IIM_RESOURCE if r.signature == *b"8BIM" => iim = Some(r.data.to_vec()),
            DIGEST_RESOURCE if r.signature == *b"8BIM" => digest = Some(r.data.to_vec()),
            _ => {}
        }
    }
    let iim = iim?;
    let changed = digest.is_some_and(|d| d[..] != Md5::digest(&iim)[..]);
    Some((iim, changed))
}

/// Photoshop's image resource block `block` (its header taken off) with its IIM and digest
/// made `iim` (none: left out), every other resource kept as it was (a clipping path, say):
/// the header included, what a JPEG's APP13 carries.
pub fn with_iim(block: &[u8], iim: Option<&[u8]>) -> Vec<u8> {
    let mut out = PHOTOSHOP.to_vec();
    for r in resources(block) {
        if r.signature != *b"8BIM" || (r.id != IIM_RESOURCE && r.id != DIGEST_RESOURCE) {
            out.extend_from_slice(r.whole);
            // A last resource whose padding the file left off.
            if r.whole.len() % 2 == 1 {
                out.push(0);
            }
        }
    }
    if let Some(iim) = iim {
        out.extend_from_slice(&to_resources(iim)[PHOTOSHOP.len()..]);
    }
    out
}

/// One of Photoshop's image resources: its signature ("8BIM", or another program's own), its
/// identifier, its data, and the whole of it as it lies (its padding included).
struct Resource<'a> {
    signature: [u8; 4],
    id: u16,
    data: &'a [u8],
    whole: &'a [u8],
}

/// The resources of Photoshop's image resource block `block` (its header taken off), in
/// order, as far as they can be read.
fn resources(block: &[u8]) -> Vec<Resource<'_>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 12 <= block.len() {
        let signature = [block[at], block[at + 1], block[at + 2], block[at + 3]];
        // A signature is four printable characters ("8BIM", "MeSa", "PHUT"...).
        if !signature.iter().all(|c| c.is_ascii_graphic()) {
            break;
        }
        let id = u16::from_be_bytes([block[at + 4], block[at + 5]]);
        // A Pascal string padded to an even length, its length byte included.
        let name = usize::from(block[at + 6]);
        let name_end = at + 6 + ((name + 1 + 1) & !1);
        let Some(size) = block.get(name_end..name_end + 4) else { break };
        let size = u32::from_be_bytes([size[0], size[1], size[2], size[3]]) as usize;
        let start = name_end + 4;
        let Some(data) = block.get(start..start.saturating_add(size)) else { break };
        let end = (start + ((size + 1) & !1)).min(block.len());
        out.push(Resource { signature, id, data, whole: &block[at..end] });
        at = end;
    }
    out
}

/// Photoshop's image resource block (its header included) holding `iim` and its digest: what
/// a JPEG's APP13 carries.
pub fn to_resources(iim: &[u8]) -> Vec<u8> {
    let mut out = PHOTOSHOP.to_vec();
    let mut resource = |id: u16, data: &[u8]| {
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(&id.to_be_bytes());
        // An empty name, padded.
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
    };
    resource(IIM_RESOURCE, iim);
    resource(DIGEST_RESOURCE, &Md5::digest(iim));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datasets_are_read_and_written_back_alike_in_their_order() {
        let mut iim = Iim::default();
        iim.set_texts(2, 120, &["A caption".into()]);
        iim.set_texts(2, 25, &["sea".into(), "boat".into()]);
        iim.set_texts(2, 5, &["Title".into()]);
        let order: Vec<(u8, u8)> = iim.sets.iter().map(|s| (s.record, s.number)).collect();
        assert_eq!(order, [(2, 0), (2, 5), (2, 25), (2, 25), (2, 120)]);
        let again = Iim::read(&iim.bytes()).unwrap();
        assert_eq!(again, iim);
        assert_eq!(again.texts(2, 25), ["sea", "boat"]);
        // Text past ASCII: said to be UTF-8, in record 1, before record 2.
        let mut iim = again;
        iim.set_texts(2, 80, &["Zoë Ångström".into()]);
        assert_eq!((iim.sets[0].record, iim.sets[0].number), CHARSET);
        assert_eq!(Iim::read(&iim.bytes()).unwrap().texts(2, 80), ["Zoë Ångström"]);
        // Replaced where it was; removed by none.
        iim.set_texts(2, 25, &["harbour".into()]);
        assert_eq!(iim.texts(2, 25), ["harbour"]);
        iim.set_texts(2, 25, &[]);
        assert!(iim.texts(2, 25).is_empty());
    }

    #[test]
    fn a_long_dataset_takes_the_extended_length_and_latin_1_is_read_as_such() {
        let long = "x".repeat(40_000);
        let iim = Iim { sets: vec![DataSet { record: 2, number: 120, data: long.clone().into_bytes() }] };
        assert_eq!(Iim::read(&iim.bytes()).unwrap().texts(2, 120), [long]);
        // No 1:90, and not UTF-8: Latin-1.
        let latin = Iim { sets: vec![DataSet { record: 2, number: 80, data: vec![b'Z', b'o', 0xEB] }] };
        assert_eq!(Iim::read(&latin.bytes()).unwrap().texts(2, 80), ["Zoë"]);
        assert!(Iim::read(&[0x1C, 2, 5, 0, 9, b'x']).is_none(), "a dataset running past the end");
    }

    #[test]
    fn photoshops_block_carries_the_iim_and_a_digest_that_tells_of_changes() {
        let mut iim = Iim::default();
        iim.set_texts(2, 5, &["Title".into()]);
        let block = to_resources(&iim.bytes());
        assert!(block.starts_with(PHOTOSHOP));
        let (read, changed) = from_resources(&block[PHOTOSHOP.len()..]).unwrap();
        assert_eq!((read, changed), (iim.bytes(), false));
        // The IIM changed without the digest: a program unaware of XMP changed it.
        let mut stale = block.clone();
        let at = stale.windows(5).position(|w| w == b"Title").unwrap();
        stale[at] = b'B';
        assert!(from_resources(&stale[PHOTOSHOP.len()..]).unwrap().1);
        // Another resource (a path, 0x07D0) kept when the IIM is replaced, and when it goes.
        let mut with_path = block.clone();
        with_path.extend_from_slice(b"8BIM\x07\xd0\x00\x00\x00\x00\x00\x03abc\x00");
        let other = Iim { sets: vec![DataSet { record: 2, number: 5, data: b"Quay".to_vec() }] };
        let replaced = with_iim(&with_path[PHOTOSHOP.len()..], Some(&other.bytes()));
        assert!(replaced.windows(3).any(|w| w == b"abc"), "the path lost");
        assert_eq!(from_resources(&replaced[PHOTOSHOP.len()..]).unwrap(), (other.bytes(), false));
        let gone = with_iim(&with_path[PHOTOSHOP.len()..], None);
        assert!(gone.windows(3).any(|w| w == b"abc") && from_resources(&gone[PHOTOSHOP.len()..]).is_none());
    }

    #[test]
    fn latin_1_texts_are_made_utf_8_when_the_block_comes_to_say_it_is() {
        // Written by an older program: Latin-1, no 1:90.
        let mut iim = Iim {
            sets: vec![
                DataSet { record: 2, number: 0, data: vec![0, 4] },
                DataSet { record: 2, number: 90, data: vec![b'M', 0xFC, b'n', b'c', b'h', b'e', b'n'] },
            ],
        };
        iim.set_texts(2, 116, &["© Hans".into()]);
        let read = Iim::read(&iim.bytes()).unwrap();
        assert_eq!(read.texts(2, 90), ["München"]);
        assert_eq!(read.texts(2, 116), ["© Hans"]);
        assert_eq!(read.sets.iter().find(|s| s.number == 0).map(|s| s.data.clone()), Some(vec![0, 4]));
    }

    #[test]
    fn another_programs_resources_are_read_past_and_kept() {
        let mut iim = Iim::default();
        iim.set_texts(2, 5, &["Title".into()]);
        // A resource of another signature first, then Photoshop's.
        let mut block = b"MeSa\x03\xe8\x00\x00\x00\x00\x00\x02ok".to_vec();
        block.extend_from_slice(&to_resources(&iim.bytes())[PHOTOSHOP.len()..]);
        assert_eq!(from_resources(&block).map(|(b, _)| b), Some(iim.bytes()));
        let kept = with_iim(&block, None);
        assert!(kept.windows(4).any(|w| w == b"MeSa"), "the other program's resource lost");
    }
}
