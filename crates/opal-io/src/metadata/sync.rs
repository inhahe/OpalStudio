//! What more than one of EXIF, IIM and XMP holds: IIM's datasets XMP holds too (IPTC Core's
//! mapping), and the three texts EXIF holds too (the Metadata Working Group's: the caption,
//! the creators, the copyright). A change a script makes to one is carried to the others; an
//! IIM a program unaware of XMP changed is carried to the XMP before anything is changed with
//! it: so that the IIM's digest, written with the IIM, which says the two agree, is true.

use super::exif::{Exif, Ifd, Value as ExifValue};
use super::iptc::Iim;
use super::xmp::{ArrayKind, DC, IPTC_CORE, Name, PHOTOSHOP, Xmp};
use super::{ARTIST, COPYRIGHT, DESCRIPTION};
use opal_core::document::Metadata;
use std::sync::Arc;

/// How XMP holds a dataset's texts.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// One text.
    Text,
    /// A language alternative: its default language's text.
    Alt,
    /// An array of texts.
    Array(ArrayKind),
    /// The intellectual genre: IIM's "IPTC:001:Current" (its source, number and name), XMP's
    /// name alone. Not carried back, the number being unknown there.
    Genre,
    /// Subject codes: IIM's "IPTC:01001000:..." (source, code, names), XMP's codes alone.
    /// Not carried back, the names being unknown there.
    Subjects,
}

/// Record 2's datasets XMP holds too: each one's number, its property and how it holds it.
/// (The date and time created, 2:55 and 2:60, are one property: see [`iim_date`].)
const MAPPED: &[(u8, &str, &str, Shape)] = &[
    (4, IPTC_CORE, "IntellectualGenre", Shape::Genre),
    (5, DC, "title", Shape::Alt),
    (10, PHOTOSHOP, "Urgency", Shape::Text),
    (12, IPTC_CORE, "SubjectCode", Shape::Subjects),
    (15, PHOTOSHOP, "Category", Shape::Text),
    (20, PHOTOSHOP, "SupplementalCategories", Shape::Array(ArrayKind::Bag)),
    (25, DC, "subject", Shape::Array(ArrayKind::Bag)),
    (40, PHOTOSHOP, "Instructions", Shape::Text),
    (80, DC, "creator", Shape::Array(ArrayKind::Seq)),
    (85, PHOTOSHOP, "AuthorsPosition", Shape::Text),
    (90, PHOTOSHOP, "City", Shape::Text),
    (92, IPTC_CORE, "Location", Shape::Text),
    (95, PHOTOSHOP, "State", Shape::Text),
    (100, IPTC_CORE, "CountryCode", Shape::Text),
    (101, PHOTOSHOP, "Country", Shape::Text),
    (103, PHOTOSHOP, "TransmissionReference", Shape::Text),
    (105, PHOTOSHOP, "Headline", Shape::Text),
    (110, PHOTOSHOP, "Credit", Shape::Text),
    (115, PHOTOSHOP, "Source", Shape::Text),
    (116, DC, "rights", Shape::Alt),
    (120, DC, "description", Shape::Alt),
    (122, PHOTOSHOP, "CaptionWriter", Shape::Text),
];

/// The date and time created, as XMP holds them (see [`iim_date`]).
const DATE_CREATED: &str = "DateCreated";

/// EXIF's texts (IFD0's) IIM and XMP hold too: each tag, its dataset, and its property (in
/// DC's namespace).
const IN_EXIF: [(u16, u8, &str); 3] =
    [(DESCRIPTION, 120, "description"), (ARTIST, 80, "creator"), (COPYRIGHT, 116, "rights")];

/// The property XMP holds dataset `n` of record 2 in, if it holds it.
fn property_of(n: u8) -> Option<(&'static str, &'static str)> {
    match n {
        55 | 60 => Some((PHOTOSHOP, DATE_CREATED)),
        _ => MAPPED.iter().find(|m| m.0 == n).map(|m| (m.1, m.2)),
    }
}

/// How XMP holds dataset `n` (one EXIF holds too).
fn shape_of(n: u8) -> Shape {
    MAPPED.iter().find(|m| m.0 == n).map_or(Shape::Text, |m| m.3)
}

/// Field `i` of IIM's "source:number:name..." text; the text itself if it is not so made.
fn field(text: &str, i: usize) -> String {
    match text.split(':').nth(i) {
        Some(f) if !f.is_empty() && text.contains(':') => f.to_owned(),
        _ => text.to_owned(),
    }
}

/// Dataset `n`'s texts (record 2's), as XMP holds them (see [`Shape`]).
fn iim_texts(iim: &Iim, n: u8, shape: Shape) -> Vec<String> {
    let texts = iim.texts(2, n).into_iter().filter(|t| !t.trim().is_empty());
    match shape {
        Shape::Text | Shape::Alt => texts.take(1).collect(),
        Shape::Genre => texts.take(1).map(|t| field(&t, 2)).collect(),
        Shape::Subjects => texts.map(|t| field(&t, 1)).collect(),
        Shape::Array(_) => texts.collect(),
    }
}

/// Makes XMP's `ns`:`local` hold `texts` as `shape` has it (none: removed).
fn set_xmp(x: &mut Xmp, ns: &str, local: &str, shape: Shape, texts: &[String]) {
    let first = texts.first().map_or("", String::as_str);
    match shape {
        Shape::Text | Shape::Genre => x.set_text(ns, local, first),
        Shape::Alt => x.set_default_text(ns, local, first),
        Shape::Array(kind) => x.set_texts(ns, local, kind, texts),
        Shape::Subjects => x.set_texts(ns, local, ArrayKind::Bag, texts),
    }
}

/// XMP's `ns`:`local` as IIM's texts, if `shape` is carried back.
fn xmp_texts(x: &Xmp, ns: &str, local: &str, shape: Shape) -> Option<Vec<String>> {
    match shape {
        Shape::Text | Shape::Alt => Some(x.text(ns, local).into_iter().collect()),
        Shape::Array(_) => Some(x.texts(ns, local)),
        Shape::Genre | Shape::Subjects => None,
    }
}

/// Whether `b` is a date as IIM writes it (2:55): "CCYYMMDD", 00 for a month or day not known.
pub fn is_iim_date(b: &[u8]) -> bool {
    b.len() == 8 && b.iter().all(u8::is_ascii_digit)
}

/// Whether `b` is a time as IIM writes it (2:60): "HHMMSS", and its zone, "±HHMM" (which some
/// writers leave off).
pub fn is_iim_time(b: &[u8]) -> bool {
    let zone = b.len() == 11 && matches!(b[6], b'+' | b'-') && b[7..].iter().all(u8::is_ascii_digit);
    (b.len() == 6 || zone) && b[..6].iter().all(u8::is_ascii_digit)
}

/// IIM's date and time created (2:55 and 2:60: see [`is_iim_date`], [`is_iim_time`]) as
/// XMP's (photoshop:DateCreated: "CCYY-MM-DDTHH:MM:SS±HH:MM", as much of it as is known; a
/// time not as IIM writes it left off). None if there is no date, or it is not one.
fn iim_date(iim: &Iim) -> Option<String> {
    let date = iim.texts(2, 55).into_iter().next()?;
    let date = date.trim();
    if !is_iim_date(date.as_bytes()) {
        return None;
    }
    let (year, month, day) = (&date[..4], &date[4..6], &date[6..]);
    let mut out = year.to_owned();
    if month != "00" {
        out = format!("{out}-{month}");
        if day != "00" {
            out = format!("{out}-{day}");
            let time = iim.texts(2, 60).into_iter().next().unwrap_or_default();
            let t = time.trim().as_bytes();
            if is_iim_time(t) {
                let s = |a: usize, b: usize| String::from_utf8_lossy(&t[a..b]).into_owned();
                out = format!("{out}T{}:{}:{}", s(0, 2), s(2, 4), s(4, 6));
                if t.len() == 11 {
                    out = format!("{out}{}{}:{}", s(6, 7), s(7, 9), s(9, 11));
                }
            }
        }
    }
    Some(out)
}

/// XMP's date created (see [`iim_date`]) as IIM's date, and its time if it gives one with
/// its zone (IIM's always has one). None if it is not a date.
fn date_of_xmp(text: &str) -> Option<(String, Option<String>)> {
    let digits = |p: &str, len: usize| p.len() == len && p.bytes().all(|c| c.is_ascii_digit());
    let (date, time) = match text.trim().split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (text.trim(), None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    let (year, month, day) = match parts[..] {
        [y] if digits(y, 4) => (y, "00", "00"),
        [y, m] if digits(y, 4) && digits(m, 2) => (y, m, "00"),
        [y, m, d] if digits(y, 4) && digits(m, 2) && digits(d, 2) => (y, m, d),
        _ => return None,
    };
    let time = time.and_then(|t| {
        // "HH:MM", ":SS" and a fraction maybe, then the zone: "Z", or "±HH:MM".
        let at = t.find(['Z', '+', '-'])?;
        let (clock, zone) = t.split_at(at);
        let zone = if zone == "Z" { "+0000".to_owned() } else { zone.replace(':', "") };
        if !(zone.len() == 5 && digits(&zone[1..], 4)) {
            return None;
        }
        let (h, m, s) = match clock.split(':').collect::<Vec<_>>()[..] {
            [h, m] => (h, m, "00"),
            [h, m, s] => (h, m, s.split('.').next().unwrap_or_default()),
            _ => return None,
        };
        (digits(h, 2) && digits(m, 2) && digits(s, 2)).then(|| format!("{h}{m}{s}{zone}"))
    });
    Some((format!("{year}{month}{day}"), time))
}

/// `x` made to say what `iim` says of every dataset of record 2 XMP holds too, where it says
/// anything: the IIM a program unaware of XMP changed since the XMP was written being the
/// newer, as its digest says (and as the guidelines have readers take it).
pub(super) fn xmp_from_iim(iim: &Iim, x: &mut Xmp) {
    iim_to_xmp(iim, x, |_| true, false);
}

/// `x` made to say what `iim` says of the datasets of record 2 that `numbers` picks (those XMP
/// holds too): where the IIM has one, what it says; where it has none, removed if `removing`
/// (a script removed it), kept if not (an IIM a program unaware of XMP changed says nothing
/// of what it does not have). The date created goes from XMP only with IIM's date: not for a
/// time set alone, which XMP has nowhere to put.
fn iim_to_xmp(iim: &Iim, x: &mut Xmp, numbers: impl Fn(u8) -> bool, removing: bool) {
    for &(n, ns, local, shape) in MAPPED {
        if numbers(n) {
            let texts = iim_texts(iim, n, shape);
            if !texts.is_empty() || removing {
                set_xmp(x, ns, local, shape, &texts);
            }
        }
    }
    if numbers(55) || numbers(60) {
        match iim_date(iim) {
            Some(date) => x.set_text(PHOTOSHOP, DATE_CREATED, &date),
            None if removing && numbers(55) && iim.texts(2, 55).is_empty() => x.remove(PHOTOSHOP, DATE_CREATED),
            None => {}
        }
    }
}

/// `iim` made to say what `x` says of the properties `names` picks that it holds too (and
/// that are carried back: see [`Shape`]); a date XMP holds that is none, removed.
fn xmp_to_iim(x: &Xmp, iim: &mut Iim, names: impl Fn(&str, &str) -> bool) {
    for &(n, ns, local, shape) in MAPPED {
        if names(ns, local)
            && let Some(texts) = xmp_texts(x, ns, local, shape)
        {
            iim.set_texts(2, n, &texts);
        }
    }
    if names(PHOTOSHOP, DATE_CREATED) {
        let (date, time) = match x.text(PHOTOSHOP, DATE_CREATED).and_then(|d| date_of_xmp(&d)) {
            Some((date, time)) => (vec![date], time.into_iter().collect()),
            None => (Vec::new(), Vec::new()),
        };
        iim.set_texts(2, 55, &date);
        iim.set_texts(2, 60, &time);
    }
}

/// EXIF's `tag` as IIM's and XMP's texts: Artist names several, separated by semicolons.
fn exif_texts(e: &Exif, tag: u16) -> Vec<String> {
    let text = e.get(Ifd::Image, tag).and_then(|v| v.text().map(str::to_owned)).unwrap_or_default();
    if tag == ARTIST {
        text.split(';').map(|a| a.trim().to_owned()).filter(|a| !a.is_empty()).collect()
    } else {
        (!text.trim().is_empty()).then_some(text).into_iter().collect()
    }
}

/// Makes EXIF's `tag` say `texts` (several joined by semicolons; none removes it), if it has
/// the tag: EXIF is given these only where it had them, as `with_description` gives them.
fn set_exif(e: &mut Exif, tag: u16, texts: &[String]) {
    if e.get(Ifd::Image, tag).is_none() {
        return;
    }
    match texts.join("; ") {
        t if t.is_empty() => e.remove(Ifd::Image, tag),
        t => e.set(Ifd::Image, tag, &ExifValue::Ascii(t)),
    }
}

/// What a script's change set itself: IFD0's tags, record 2's datasets, XMP's properties.
#[derive(Debug, Default)]
pub struct Set {
    pub exif: Vec<u16>,
    pub iim: Vec<u8>,
    pub xmp: Vec<Name>,
}

/// `meta`, which a script has changed as `set` says, each change carried to what holds the
/// same elsewhere, unless the script changed that too. Where two changes carry to the same
/// place, XMP's goes over IIM's, and IIM's over EXIF's (as readers prefer them, the IIM's
/// digest agreeing). An IIM is not made where there is none, nor EXIF's tags where it has
/// none (see [`set_exif`]); the XMP is made if need be.
pub fn carried(meta: &Metadata, set: &Set) -> Metadata {
    let exif0 = meta.exif.as_deref().and_then(Exif::read);
    let iim0 = meta.iptc.as_deref().and_then(Iim::read);
    let xmp0 = meta.xmp.as_deref().and_then(Xmp::read).unwrap_or_default();
    let (mut exif, mut iim, mut x) = (exif0.clone(), iim0.clone(), xmp0.clone());
    let xmp_set = |ns: &str, local: &str| set.xmp.iter().any(|n| n.ns == ns && n.local == local);
    let iim_set = |n: u8| set.iim.contains(&n);
    // From EXIF (a removal too, though it took EXIF's last tag with it).
    {
        let from = exif0.clone().unwrap_or_default();
        for (tag, n, local) in IN_EXIF.into_iter().filter(|(tag, ..)| set.exif.contains(tag)) {
            let texts = exif_texts(&from, tag);
            if !xmp_set(DC, local) {
                set_xmp(&mut x, DC, local, shape_of(n), &texts);
            }
            if let Some(i) = iim.as_mut().filter(|_| !iim_set(n)) {
                i.set_texts(2, n, &texts);
            }
        }
    }
    // From the IIM (a removal too, though it took the IIM's last dataset with it).
    if !set.iim.is_empty() {
        let from = &iim0.clone().unwrap_or_default();
        let picked = |n: u8| iim_set(n) && !property_of(n).is_some_and(|(ns, local)| xmp_set(ns, local));
        iim_to_xmp(from, &mut x, picked, true);
        if let Some(e) = &mut exif {
            for (tag, n, _) in IN_EXIF.into_iter().filter(|&(tag, n, _)| iim_set(n) && !set.exif.contains(&tag)) {
                set_exif(e, tag, &iim_texts(from, n, shape_of(n)));
            }
        }
    }
    // From XMP.
    if let Some(i) = &mut iim {
        let picked = |ns: &str, local: &str| {
            xmp_set(ns, local)
                && !MAPPED.iter().any(|m| (m.1, m.2) == (ns, local) && iim_set(m.0))
                && !((ns, local) == (PHOTOSHOP, DATE_CREATED) && (iim_set(55) || iim_set(60)))
        };
        xmp_to_iim(&x, i, picked);
    }
    if let Some(e) = &mut exif {
        for (tag, n, local) in
            IN_EXIF.into_iter().filter(|&(tag, _, local)| xmp_set(DC, local) && !set.exif.contains(&tag))
        {
            set_exif(e, tag, &xmp_texts(&x, DC, local, shape_of(n)).unwrap_or_default());
        }
    }
    let mut out = meta.clone();
    if exif != exif0 {
        out.exif = exif.map(|e| Arc::from(e.into_bytes()));
    }
    if iim != iim0 {
        out.iptc = iim.filter(|i| !i.is_empty()).map(|i| Arc::from(i.bytes()));
    }
    if x != xmp0 {
        out.xmp = (!x.properties.is_empty()).then(|| Arc::from(x.packet().into_bytes()));
    }
    out
}

/// `meta`, its IIM carried to its XMP if a program unaware of XMP changed the IIM since the
/// XMP was written (as its digest says): whatever the IIM says, the XMP is made to say too
/// (and EXIF's texts, with what describes the image), as the Metadata Working Group's
/// guidelines have readers take it. Both then agree, as the digest written with the IIM says
/// (see `with_description`, which does it).
pub fn reconciled(meta: &Metadata) -> Metadata {
    if !meta.iptc_changed {
        return meta.clone();
    }
    super::with_description(meta, &super::description(meta))
}

/// Whether EXIF's IFD0 tag `tag` is one IIM and XMP hold too (see the module).
pub fn held_elsewhere(tag: u16) -> bool {
    IN_EXIF.iter().any(|&(t, ..)| t == tag)
}
