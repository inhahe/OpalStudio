//! Metadata: what a JPEG's or PNG's file says about the image besides its pixels (EXIF, XMP,
//! IPTC IIM), read from it, kept with the document as it was read
//! ([`Metadata`](opal_core::document::Metadata)), shown, edited, and written on export with
//! what it says made true of the image exported, and as little of it as the user chooses.

pub mod capture;
pub mod container;
pub mod exif;
pub mod file;
pub mod iptc;
pub mod json;
pub mod tags;
pub mod xmp;

use exif::{Exif, Ifd, Value};
use opal_core::document::Metadata;
use std::sync::Arc;
use xmp::{ArrayKind, Name, Xmp};

/// Metadata as a file's segments or chunks hold it, before it is joined into a document's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
    /// A JPEG's Extended XMP, its chunks joined: the `x:xmpmeta` of the properties the
    /// standard packet had no room for.
    pub extended_xmp: Option<Vec<u8>>,
    pub iptc: Option<Vec<u8>>,
    /// Whether the IIM's digest said a program unaware of XMP changed it after the XMP was
    /// written.
    pub iptc_changed: bool,
}

impl Raw {
    /// As a document keeps it: what reads as EXIF, XMP (its Extended XMP joined) and IIM, each
    /// left out if it does not read as what it should be.
    pub fn into_metadata(self) -> Metadata {
        let exif = self.exif.filter(|b| Exif::read(b).is_some());
        let xmp = self.xmp.and_then(|standard| match self.extended_xmp.as_deref().and_then(Xmp::read) {
            // Joined, it is written anew.
            Some(extended) => {
                let mut x = Xmp::read(&standard)?;
                x.join(extended);
                Some(x.packet().into_bytes())
            }
            None => Xmp::read(&standard).map(|_| standard),
        });
        let iptc = self.iptc.filter(|b| iptc::Iim::read(b).is_some());
        Metadata {
            exif: exif.map(Arc::from),
            xmp: xmp.map(Arc::from),
            iptc: iptc.map(Arc::from),
            iptc_changed: self.iptc_changed,
        }
    }
}

/// The metadata of a file's bytes (a JPEG or PNG; anything else has none).
pub fn read(bytes: &[u8]) -> Metadata {
    let raw = if bytes.starts_with(&[0xFF, 0xD8]) {
        container::read_jpeg(bytes)
    } else if bytes.starts_with(b"\x89PNG") {
        container::read_png(bytes)
    } else {
        Raw::default()
    };
    raw.into_metadata()
}

/// What export writes of the image's metadata (and the preference's default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// All of it.
    All,
    /// All but where it was taken (GPS, and the place named) and what identifies the camera
    /// and its owner (serial numbers, the owner's name, the maker note, which holds serial
    /// numbers in most cameras').
    NotPlace,
    /// Only who made it and holds its copyright, and how to reach them.
    Copyright,
    /// None.
    Nothing,
}

impl Keep {
    pub const ALL: [Keep; 4] = [Keep::All, Keep::NotPlace, Keep::Copyright, Keep::Nothing];

    /// Its name, as preferences and scripts give it.
    pub fn name(self) -> &'static str {
        match self {
            Keep::All => "all",
            Keep::NotPlace => "no place",
            Keep::Copyright => "copyright",
            Keep::Nothing => "none",
        }
    }

    pub fn named(name: &str) -> Option<Keep> {
        Keep::ALL.into_iter().find(|k| k.name() == name)
    }

    /// What it is, as the export dialog lists it.
    pub fn label(self) -> &'static str {
        match self {
            Keep::All => "All",
            Keep::NotPlace => "All but the place and serial numbers",
            Keep::Copyright => "Copyright and contact only",
            Keep::Nothing => "None",
        }
    }
}

/// What is true of an image exported: its size, its resolution, and when.
#[derive(Clone, Debug)]
pub struct Exported {
    pub width: u32,
    pub height: u32,
    /// Pixels per inch.
    pub ppi: f64,
    /// Now, as EXIF writes a date ("YYYY:MM:DD HH:MM:SS") and XMP does (ISO 8601).
    pub exif_now: String,
    pub xmp_now: String,
    /// What wrote it ("Opal Studio 0.14.1").
    pub software: String,
}

/// Metadata made ready to write: each part's bytes or packet, none where nothing is left.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Xmp>,
    pub iptc: Option<Vec<u8>>,
}

impl Prepared {
    /// The JPEG segments it is written as; EXIF left out (said in `notes`) if, past all
    /// trimming, it is too large for its segment.
    pub fn jpeg_segments(&self, notes: &mut Vec<String>) -> Vec<u8> {
        let photoshop = self.iptc.as_deref().map(iptc::to_resources);
        match container::jpeg_segments_of(self.exif.as_deref(), self.xmp.as_ref(), photoshop.as_deref()) {
            Ok(s) => s,
            Err(why) => {
                notes.push(format!("The photograph's EXIF was left out of the JPEG: {why}."));
                container::jpeg_segments_of(None, self.xmp.as_ref(), photoshop.as_deref()).unwrap_or_default()
            }
        }
    }

    /// The PNG chunks it is written as (each its type and data).
    pub fn png_chunks(&self) -> Vec<([u8; 4], Vec<u8>)> {
        container::png_chunks_of(self.exif.as_deref(), self.xmp.as_ref(), self.iptc.as_deref())
    }

    pub fn is_empty(&self) -> bool {
        self.exif.is_none() && self.xmp.is_none() && self.iptc.is_none()
    }
}

// EXIF's tags that export changes or leaves out.
const ORIENTATION: u16 = 0x0112;
const X_RESOLUTION: u16 = 0x011A;
const Y_RESOLUTION: u16 = 0x011B;
const RESOLUTION_UNIT: u16 = 0x0128;
const SOFTWARE: u16 = 0x0131;
const DATE_TIME: u16 = 0x0132;
const IMAGE_WIDTH: u16 = 0x0100;
const IMAGE_LENGTH: u16 = 0x0101;
const DESCRIPTION: u16 = 0x010E;
const ARTIST: u16 = 0x013B;
const COPYRIGHT: u16 = 0x8298;
const PIXEL_X: u16 = 0xA002;
const PIXEL_Y: u16 = 0xA003;
const MAKER_NOTE: u16 = 0x927C;
/// What identifies the camera, the lens, or their owner.
const IDENTIFYING: [u16; 4] = [0xA430, 0xA431, 0xA435, MAKER_NOTE];

/// XMP's properties that place the image: in IPTC Core and Photoshop's schemas, and the
/// IPTC extension's locations.
fn places(name: &Name) -> bool {
    (name.ns == xmp::EXIF && name.local.starts_with("GPS"))
        || (name.ns == xmp::PHOTOSHOP && ["City", "State", "Country"].contains(&name.local.as_str()))
        || (name.ns == xmp::IPTC_CORE && ["Location", "CountryCode"].contains(&name.local.as_str()))
        || (name.ns == xmp::IPTC_EXT && ["LocationCreated", "LocationShown"].contains(&name.local.as_str()))
}

/// XMP's properties that identify the camera, the lens or their owner.
fn identifies(name: &Name) -> bool {
    (name.ns == xmp::AUX
        && ["SerialNumber", "LensSerialNumber", "OwnerName", "ImageNumber"].contains(&name.local.as_str()))
        || (name.ns == xmp::EXIF_EX
            && ["BodySerialNumber", "LensSerialNumber", "CameraOwnerName"].contains(&name.local.as_str()))
}

/// XMP's properties that say who made the image and holds its copyright, and how to reach
/// them.
fn copyright_and_contact(name: &Name) -> bool {
    (name.ns == xmp::DC && ["creator", "rights"].contains(&name.local.as_str()))
        || name.ns == xmp::XMP_RIGHTS
        || (name.ns == xmp::IPTC_CORE && name.local == "CreatorContactInfo")
        || (name.ns == xmp::PHOTOSHOP && ["Credit", "Source"].contains(&name.local.as_str()))
}

/// IIM's datasets that place the image: sublocation, city, state, country and its code.
const IIM_PLACES: [u8; 5] = [90, 92, 95, 100, 101];
/// IIM's that say who made it and holds its copyright: by-line, its title, credit, source,
/// copyright notice, contact.
const IIM_COPYRIGHT: [u8; 6] = [80, 85, 110, 115, 116, 118];

/// `meta` made ready for an image exported as `out` says, keeping what `keep` keeps: the
/// orientation upright (the pixels are), the size and resolution the exported ones, the
/// modification date now, the thumbnails (which show the image as it was) gone.
pub fn for_export(meta: &Metadata, keep: Keep, out: &Exported) -> Prepared {
    if keep == Keep::Nothing {
        return Prepared::default();
    }
    let mut prepared = Prepared::default();
    // EXIF.
    let exif = meta.exif.as_deref().and_then(Exif::read);
    if let Some(mut e) = match keep {
        Keep::Copyright => exif.map(|e| {
            let mut fresh = Exif::new();
            for tag in [ARTIST, COPYRIGHT] {
                if let Some(v) = e.get(Ifd::Image, tag) {
                    fresh.set(Ifd::Image, tag, &v);
                }
            }
            fresh
        }),
        _ => exif,
    } {
        e.remove_ifd(Ifd::Thumbnail);
        if keep == Keep::NotPlace {
            e.remove_ifd(Ifd::Gps);
            for tag in IDENTIFYING {
                e.remove(Ifd::Exif, tag);
            }
        }
        if keep != Keep::Copyright {
            let upright = Value::Shorts(vec![1]);
            if e.get(Ifd::Image, ORIENTATION).is_some() {
                e.set(Ifd::Image, ORIENTATION, &upright);
            }
            let ppi = rational(out.ppi);
            e.set(Ifd::Image, X_RESOLUTION, &ppi);
            e.set(Ifd::Image, Y_RESOLUTION, &ppi);
            e.set(Ifd::Image, RESOLUTION_UNIT, &Value::Shorts(vec![2]));
            for (tag, v) in [(IMAGE_WIDTH, out.width), (IMAGE_LENGTH, out.height)] {
                if e.get(Ifd::Image, tag).is_some() {
                    e.set(Ifd::Image, tag, &Value::Longs(vec![v]));
                }
            }
            for (tag, v) in [(PIXEL_X, out.width), (PIXEL_Y, out.height)] {
                if e.get(Ifd::Exif, tag).is_some() {
                    e.set(Ifd::Exif, tag, &Value::Longs(vec![v]));
                }
            }
            e.set(Ifd::Image, DATE_TIME, &Value::Ascii(out.exif_now.clone()));
            e.set(Ifd::Image, SOFTWARE, &Value::Ascii(out.software.clone()));
        }
        e.trim();
        prepared.exif = Some(e.into_bytes());
    }
    // XMP.
    if let Some(mut x) = meta.xmp.as_deref().and_then(Xmp::read) {
        // Thumbnails, and the history of what was done in another program, are left behind
        // with the image they were of.
        x.remove(xmp::XMP, "Thumbnails");
        x.remove(xmp::XMP_NOTE, "HasExtendedXMP");
        match keep {
            Keep::NotPlace => x.retain(|n| !places(n) && !identifies(n)),
            Keep::Copyright => x.retain(copyright_and_contact),
            _ => {}
        }
        if keep != Keep::Copyright {
            for (ns, local) in [(xmp::TIFF, "Orientation")] {
                if x.get(ns, local).is_some() {
                    x.set_text(ns, local, "1");
                }
            }
            for (ns, local, v) in [
                (xmp::EXIF, "PixelXDimension", out.width),
                (xmp::EXIF, "PixelYDimension", out.height),
                (xmp::TIFF, "ImageWidth", out.width),
                (xmp::TIFF, "ImageLength", out.height),
            ] {
                if x.get(ns, local).is_some() {
                    x.set_text(ns, local, &v.to_string());
                }
            }
            for (ns, local) in [(xmp::TIFF, "XResolution"), (xmp::TIFF, "YResolution")] {
                if x.get(ns, local).is_some() {
                    x.set_text(ns, local, &format!("{}/10000", (out.ppi * 10000.0).round() as u64));
                }
            }
            if x.get(xmp::TIFF, "ResolutionUnit").is_some() {
                x.set_text(xmp::TIFF, "ResolutionUnit", "2");
            }
            x.set_text(xmp::XMP, "ModifyDate", &out.xmp_now);
            x.set_text(xmp::XMP, "MetadataDate", &out.xmp_now);
        }
        if !x.properties.is_empty() {
            prepared.xmp = Some(x);
        }
    }
    // IIM.
    if let Some(mut iim) = meta.iptc.as_deref().and_then(iptc::Iim::read) {
        match keep {
            Keep::NotPlace => iim.retain(|r, n| !(r == 2 && IIM_PLACES.contains(&n))),
            Keep::Copyright => iim.retain(|r, n| r == 1 || (r == 2 && (n == 0 || IIM_COPYRIGHT.contains(&n)))),
            _ => {}
        }
        if !iim.is_empty() {
            prepared.iptc = Some(iim.bytes());
        }
    }
    prepared
}

/// `ppi` as an EXIF rational, exact to a ten-thousandth.
fn rational(ppi: f64) -> Value {
    let (n, d) = if ppi.fract() == 0.0 && ppi < f64::from(u32::MAX) {
        (ppi as u32, 1)
    } else {
        ((ppi * 10000.0).round().min(f64::from(u32::MAX)) as u32, 10000)
    };
    Value::Rationals(vec![[n, d]])
}

/// What describes the image, as File Info edits it: read as the Metadata Working Group's
/// guidelines say, from XMP, then IIM (first, if its digest says it changed since), then
/// EXIF; written to all three.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Description {
    pub title: String,
    pub caption: String,
    pub creators: Vec<String>,
    pub email: String,
    pub website: String,
    pub copyright: String,
    /// Whether it is copyrighted (true), in the public domain (false), or not said.
    pub copyrighted: Option<bool>,
    /// A web page saying how it may be used.
    pub copyright_url: String,
    pub keywords: Vec<String>,
}

const CONTACT: &str = "CreatorContactInfo";

fn contact_field(x: &Xmp, field: &str) -> Option<String> {
    x.get(xmp::IPTC_CORE, CONTACT)?.field(&Name::new(xmp::IPTC_CORE, field))?.as_text().map(str::to_owned)
}

/// What describes the image `meta` is of (see [`Description`]).
pub fn description(meta: &Metadata) -> Description {
    let x = meta.xmp.as_deref().and_then(Xmp::read).unwrap_or_default();
    let iim = meta.iptc.as_deref().and_then(iptc::Iim::read).unwrap_or_default();
    let e = meta.exif.as_deref().and_then(Exif::read);
    let exif_text = |tag| e.as_ref().and_then(|e| e.get(Ifd::Image, tag)).and_then(|v| v.text().map(str::to_owned));
    let iim_text = |n| iim.texts(2, n).into_iter().next().filter(|t| !t.is_empty());
    let iim_texts = |n| iim.texts(2, n);
    // One text: XMP's, IIM's or EXIF's, IIM's first if a program unaware of XMP changed it.
    let one = |from_xmp: Option<String>, from_iim: Option<String>, from_exif: Option<String>| {
        let (a, b) = if meta.iptc_changed { (from_iim, from_xmp) } else { (from_xmp, from_iim) };
        a.or(b).or(from_exif.filter(|t| !t.trim().is_empty())).unwrap_or_default()
    };
    let many = |from_xmp: Vec<String>, from_iim: Vec<String>| {
        let (a, b) = if meta.iptc_changed { (from_iim, from_xmp) } else { (from_xmp, from_iim) };
        if a.is_empty() { b } else { a }
    };
    let mut creators = many(x.texts(xmp::DC, "creator"), iim_texts(80));
    if creators.is_empty()
        && let Some(artist) = exif_text(ARTIST).filter(|a| !a.trim().is_empty())
    {
        // EXIF's Artist names several separated by semicolons.
        creators = artist.split(';').map(|a| a.trim().to_owned()).filter(|a| !a.is_empty()).collect();
    }
    Description {
        title: one(x.text(xmp::DC, "title"), iim_text(5), None),
        caption: one(x.text(xmp::DC, "description"), iim_text(120), exif_text(DESCRIPTION)),
        creators,
        email: contact_field(&x, "CiEmailWork").unwrap_or_default(),
        website: contact_field(&x, "CiUrlWork").unwrap_or_default(),
        copyright: one(x.text(xmp::DC, "rights"), iim_text(116), exif_text(COPYRIGHT)),
        copyrighted: x.text(xmp::XMP_RIGHTS, "Marked").map(|m| m.eq_ignore_ascii_case("true")),
        copyright_url: x.text(xmp::XMP_RIGHTS, "WebStatement").unwrap_or_default(),
        keywords: many(x.texts(xmp::DC, "subject"), iim_texts(25)),
    }
}

/// `meta` describing the image as `d` says: XMP (made if there is none) given every field;
/// the IIM, if there is one, given those it has (its digest worked out again on writing);
/// EXIF's description, artist and copyright, where it has them.
pub fn with_description(meta: &Metadata, d: &Description) -> Metadata {
    let mut x = meta.xmp.as_deref().and_then(Xmp::read).unwrap_or_default();
    x.set_default_text(xmp::DC, "title", &d.title);
    x.set_default_text(xmp::DC, "description", &d.caption);
    x.set_texts(xmp::DC, "creator", ArrayKind::Seq, &d.creators);
    x.set_field(xmp::IPTC_CORE, CONTACT, &Name::new(xmp::IPTC_CORE, "CiEmailWork"), &d.email);
    x.set_field(xmp::IPTC_CORE, CONTACT, &Name::new(xmp::IPTC_CORE, "CiUrlWork"), &d.website);
    x.set_default_text(xmp::DC, "rights", &d.copyright);
    match d.copyrighted {
        Some(c) => x.set_text(xmp::XMP_RIGHTS, "Marked", if c { "True" } else { "False" }),
        None => x.remove(xmp::XMP_RIGHTS, "Marked"),
    }
    x.set_text(xmp::XMP_RIGHTS, "WebStatement", &d.copyright_url);
    x.set_texts(xmp::DC, "subject", ArrayKind::Bag, &d.keywords);
    let iptc = meta.iptc.as_deref().and_then(iptc::Iim::read).map(|mut iim| {
        let one = |t: &str| if t.is_empty() { Vec::new() } else { vec![t.to_owned()] };
        iim.set_texts(2, 5, &one(&d.title));
        iim.set_texts(2, 120, &one(&d.caption));
        iim.set_texts(2, 80, &d.creators);
        iim.set_texts(2, 116, &one(&d.copyright));
        iim.set_texts(2, 25, &d.keywords);
        Arc::from(iim.bytes())
    });
    let exif = meta.exif.as_deref().and_then(Exif::read).map(|mut e| {
        for (tag, text) in
            [(DESCRIPTION, d.caption.clone()), (ARTIST, d.creators.join("; ")), (COPYRIGHT, d.copyright.clone())]
        {
            if e.get(Ifd::Image, tag).is_some() {
                if text.is_empty() {
                    e.remove(Ifd::Image, tag);
                } else {
                    e.set(Ifd::Image, tag, &Value::Ascii(text));
                }
            }
        }
        Arc::from(e.into_bytes())
    });
    Metadata {
        exif: exif.or_else(|| meta.exif.clone()),
        xmp: (!x.properties.is_empty()).then(|| Arc::from(x.packet().into_bytes())),
        iptc: iptc.or_else(|| meta.iptc.clone()),
        // Written together, they agree.
        iptc_changed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photograph's metadata as a camera, then a photo manager, would leave it: EXIF with
    /// GPS, a serial number and a maker note; XMP with a place, a serial number and a
    /// thumbnail; an IIM with a city.
    fn photograph() -> Metadata {
        let mut e = Exif::new();
        e.set(Ifd::Image, 0x010F, &Value::Ascii("Canon".into()));
        e.set(Ifd::Image, ORIENTATION, &Value::Shorts(vec![6]));
        e.set(Ifd::Image, ARTIST, &Value::Ascii("Ann; Bob".into()));
        e.set(Ifd::Image, COPYRIGHT, &Value::Ascii("(c) Ann".into()));
        e.set(Ifd::Exif, 0x829A, &Value::Rationals(vec![[1, 250]]));
        e.set(Ifd::Exif, PIXEL_X, &Value::Longs(vec![6000]));
        e.set(Ifd::Exif, PIXEL_Y, &Value::Longs(vec![4000]));
        e.set(Ifd::Exif, 0xA431, &Value::Ascii("SERIAL-1".into()));
        e.set(Ifd::Exif, MAKER_NOTE, &Value::Undefined(b"the maker's own".to_vec()));
        e.set(Ifd::Gps, 0x0001, &Value::Ascii("N".into()));
        e.set(Ifd::Gps, 0x0002, &Value::Rationals(vec![[48, 1], [51, 1], [30, 1]]));
        let mut x = Xmp::default();
        x.set_default_text(xmp::DC, "title", "Harbour");
        x.set_texts(xmp::DC, "creator", ArrayKind::Seq, &["Ann".into()]);
        x.set_default_text(xmp::DC, "rights", "© Ann");
        x.set_text(xmp::TIFF, "Orientation", "6");
        x.set_text(xmp::EXIF, "PixelXDimension", "6000");
        x.set_text(xmp::EXIF, "GPSLatitude", "48,51.5N");
        x.set_text(xmp::PHOTOSHOP, "City", "Paris");
        x.set_text(xmp::AUX, "SerialNumber", "SERIAL-1");
        x.set_text(xmp::XMP, "Thumbnails", "old");
        let mut iim = iptc::Iim::default();
        iim.set_texts(2, 5, &["Harbour".into()]);
        iim.set_texts(2, 90, &["Paris".into()]);
        iim.set_texts(2, 116, &["© Ann".into()]);
        Metadata {
            exif: Some(Arc::from(e.into_bytes())),
            xmp: Some(Arc::from(x.packet().into_bytes())),
            iptc: Some(Arc::from(iim.bytes())),
            iptc_changed: false,
        }
    }

    fn exported() -> Exported {
        Exported {
            width: 1200,
            height: 800,
            ppi: 300.0,
            exif_now: "2026:11:12 10:00:00".into(),
            xmp_now: "2026-11-12T10:00:00+01:00".into(),
            software: "Opal Studio 1.0".into(),
        }
    }

    #[test]
    fn all_of_it_goes_out_made_true_of_the_image_exported() {
        let p = for_export(&photograph(), Keep::All, &exported());
        let e = Exif::read(p.exif.as_deref().unwrap()).unwrap();
        assert_eq!(e.get(Ifd::Image, ORIENTATION), Some(Value::Shorts(vec![1])));
        assert_eq!(e.get(Ifd::Exif, PIXEL_X), Some(Value::Longs(vec![1200])));
        assert_eq!(e.get(Ifd::Image, X_RESOLUTION).and_then(|v| v.number()), Some(300.0));
        assert_eq!(e.get(Ifd::Image, DATE_TIME), Some(Value::Ascii("2026:11:12 10:00:00".into())));
        assert!(e.get(Ifd::Gps, 0x0002).is_some() && e.get(Ifd::Exif, MAKER_NOTE).is_some());
        let x = p.xmp.unwrap();
        assert_eq!(x.text(xmp::TIFF, "Orientation").as_deref(), Some("1"));
        assert_eq!(x.text(xmp::EXIF, "PixelXDimension").as_deref(), Some("1200"));
        assert_eq!(x.text(xmp::XMP, "ModifyDate").as_deref(), Some("2026-11-12T10:00:00+01:00"));
        assert!(x.get(xmp::XMP, "Thumbnails").is_none(), "a thumbnail of the image as it was");
        assert_eq!(iptc::Iim::read(p.iptc.as_deref().unwrap()).unwrap().texts(2, 90), ["Paris"]);
    }

    #[test]
    fn the_place_and_serial_numbers_are_left_behind_and_the_rest_goes() {
        let p = for_export(&photograph(), Keep::NotPlace, &exported());
        let e = Exif::read(p.exif.as_deref().unwrap()).unwrap();
        assert!(e.get(Ifd::Gps, 0x0002).is_none() && e.get(Ifd::Image, exif::GPS_POINTER).is_none());
        assert!(e.get(Ifd::Exif, 0xA431).is_none() && e.get(Ifd::Exif, MAKER_NOTE).is_none());
        assert!(!e.bytes().windows(8).any(|w| w == b"SERIAL-1"), "the serial number left in the bytes");
        assert_eq!(e.get(Ifd::Image, 0x010F), Some(Value::Ascii("Canon".into())));
        assert!(e.get(Ifd::Exif, 0x829A).is_some(), "the exposure lost");
        let x = p.xmp.unwrap();
        for (ns, local) in [(xmp::EXIF, "GPSLatitude"), (xmp::PHOTOSHOP, "City"), (xmp::AUX, "SerialNumber")] {
            assert!(x.get(ns, local).is_none(), "{local} left");
        }
        assert_eq!(x.text(xmp::DC, "title").as_deref(), Some("Harbour"));
        let iim = iptc::Iim::read(p.iptc.as_deref().unwrap()).unwrap();
        assert!(iim.texts(2, 90).is_empty() && iim.texts(2, 5) == ["Harbour"]);
    }

    #[test]
    fn copyright_and_contact_alone_or_nothing() {
        let p = for_export(&photograph(), Keep::Copyright, &exported());
        let e = Exif::read(p.exif.as_deref().unwrap()).unwrap();
        let tags: Vec<u16> = e.tags(Ifd::Image).into_iter().map(|(t, _)| t).collect();
        assert_eq!(tags, [ARTIST, COPYRIGHT]);
        assert!(e.get(Ifd::Exif, 0x829A).is_none());
        let x = p.xmp.unwrap();
        let names: Vec<&str> = x.properties.iter().map(|(n, _)| n.local.as_str()).collect();
        assert_eq!(names, ["creator", "rights"]);
        let iim = iptc::Iim::read(p.iptc.as_deref().unwrap()).unwrap();
        assert!(iim.texts(2, 5).is_empty() && iim.texts(2, 116) == ["© Ann"]);
        assert!(for_export(&photograph(), Keep::Nothing, &exported()).is_empty());
        assert!(for_export(&Metadata::default(), Keep::All, &exported()).is_empty(), "made up for an image with none");
    }

    #[test]
    fn the_description_reads_as_the_guidelines_say_and_is_written_to_all_three() {
        let meta = photograph();
        let d = description(&meta);
        assert_eq!((d.title.as_str(), d.copyright.as_str()), ("Harbour", "© Ann"));
        // XMP's creator first; EXIF's Artist (two of them) only where neither has any.
        assert_eq!(d.creators, ["Ann"]);
        let only_exif = Metadata { exif: meta.exif.clone(), ..Metadata::default() };
        assert_eq!(description(&only_exif).creators, ["Ann", "Bob"]);
        // A program unaware of XMP changed the IIM since: its title first.
        let mut iim = iptc::Iim::read(meta.iptc.as_deref().unwrap()).unwrap();
        iim.set_texts(2, 5, &["Quay".into()]);
        let changed = Metadata { iptc: Some(Arc::from(iim.bytes())), iptc_changed: true, ..meta.clone() };
        assert_eq!(description(&changed).title, "Quay");
        // Written: XMP, the IIM, and EXIF's artist and copyright, all saying the same.
        let new = Description {
            title: "Quay at dusk".into(),
            caption: "Boats".into(),
            creators: vec!["Zoë".into()],
            copyright: "© Zoë".into(),
            copyrighted: Some(true),
            keywords: vec!["sea".into(), "boat".into()],
            ..d.clone()
        };
        let written = with_description(&meta, &new);
        assert_eq!(description(&written), new);
        let iim = iptc::Iim::read(written.iptc.as_deref().unwrap()).unwrap();
        assert_eq!((iim.texts(2, 5), iim.texts(2, 25)), (vec!["Quay at dusk".to_owned()], new.keywords.clone()));
        let e = Exif::read(written.exif.as_deref().unwrap()).unwrap();
        assert_eq!(e.get(Ifd::Image, COPYRIGHT), Some(Value::Ascii("© Zoë".into())));
        assert_eq!(e.get(Ifd::Image, ARTIST), Some(Value::Ascii("Zoë".into())));
        // EXIF's description not there to begin with: not made.
        assert!(e.get(Ifd::Image, DESCRIPTION).is_none());
        // None to begin with: XMP made, nothing else.
        let fresh = with_description(&Metadata::default(), &new);
        assert!(fresh.xmp.is_some() && fresh.exif.is_none() && fresh.iptc.is_none());
        assert_eq!(description(&fresh), new);
    }
}
