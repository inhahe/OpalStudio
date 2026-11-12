//! Metadata as scripts see it and change it: every EXIF tag (by name, or number if unknown),
//! IIM dataset and XMP property, as JSON, besides the description and capture File Info
//! shows.

use super::exif::{Exif, Ifd, Value};
use super::xmp::{self, ArrayKind, Name, Xmp};
use super::{Description, capture, description, iptc, sync, tags};
use opal_core::document::Metadata;
use serde_json::{Map, Value as Json, json};
use std::sync::Arc;

/// All of `meta`, as scripts read it: `description` (as File Info edits it), `capture` (as it
/// shows it, label by label), `exif` (IFD by IFD, tag by name), `iptc` (dataset by
/// "record:number", each a list of texts), and `xmp` (property by "prefix:name", or by
/// "{namespace}name" for one whose namespace has no usual prefix).
pub fn to_json(meta: &Metadata) -> Json {
    let d = description(meta);
    let capture: Map<String, Json> =
        capture::capture(meta).into_iter().map(|(label, value)| (label.to_owned(), Json::from(value))).collect();
    let mut exif = Map::new();
    if let Some(e) = meta.exif.as_deref().and_then(Exif::read) {
        for ifd in Ifd::ALL {
            let fields: Map<String, Json> = e
                .tags(ifd)
                .into_iter()
                .map(|(tag, v)| (tags::name(ifd, tag), exif_json(ifd, tag, &v, e.big_endian())))
                .collect();
            if !fields.is_empty() {
                exif.insert(ifd.name().to_owned(), Json::Object(fields));
            }
        }
    }
    let mut iim = Map::new();
    if let Some(i) = meta.iptc.as_deref().and_then(iptc::Iim::read) {
        for s in &i.sets {
            let key = format!("{}:{}", s.record, s.number);
            if !iim.contains_key(&key) {
                iim.insert(key, Json::from(i.texts(s.record, s.number)));
            }
        }
    }
    let mut x = Map::new();
    if let Some(p) = meta.xmp.as_deref().and_then(Xmp::read) {
        for (name, value) in &p.properties {
            x.insert(xmp_key(name), xmp_json(value));
        }
    }
    json!({
        "description": description_json(&d),
        "capture": capture,
        "exif": exif,
        "iptc": iim,
        "xmp": x,
    })
}

/// A description as scripts read and give it.
pub fn description_json(d: &Description) -> Json {
    json!({
        "title": d.title,
        "caption": d.caption,
        "creators": d.creators,
        "email": d.email,
        "website": d.website,
        "copyright": d.copyright,
        "copyright_status": match d.copyrighted { None => "unknown", Some(true) => "copyrighted", Some(false) => "public domain" },
        "copyright_url": d.copyright_url,
        "keywords": d.keywords,
    })
}

/// Whether tag `tag` of `ifd` holds text after an 8-byte code saying how it is written (EXIF
/// 2.32, 4.6.5): the user's comment, and GPS's processing method and area.
fn coded_text(ifd: Ifd, tag: u16) -> bool {
    matches!((ifd, tag), (Ifd::Exif, 0x9286) | (Ifd::Gps, 0x001B | 0x001C))
}

/// Such a tag's text, as its code says it is written: ASCII; Unicode (UCS-2, in the
/// structure's byte order, big-endian if `big`); or undefined (read as UTF-8). Its padding
/// taken off. None for another code (JIS), or bytes that are not what it says.
fn text_of_coded(b: &[u8], big: bool) -> Option<String> {
    let (code, text) = (b.get(..8)?, &b[8..]);
    let text = if code == b"ASCII\0\0\0" || code == [0; 8] {
        std::str::from_utf8(text).ok()?.to_owned()
    } else if code == b"UNICODE\0" {
        let units: Vec<u16> = text
            .chunks_exact(2)
            .map(|p| if big { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) })
            .collect();
        String::from_utf16(&units).ok()?
    } else {
        return None;
    };
    Some(text.trim_end_matches(['\0', ' ']).to_owned())
}

/// `text` as such a tag holds it (see [`text_of_coded`]): ASCII if it is, else Unicode.
fn coded(text: &str, big: bool) -> Vec<u8> {
    if text.is_ascii() {
        return [&b"ASCII\0\0\0"[..], text.as_bytes()].concat();
    }
    let mut out = b"UNICODE\0".to_vec();
    for u in text.encode_utf16() {
        out.extend_from_slice(&if big { u.to_be_bytes() } else { u.to_le_bytes() });
    }
    out
}

/// An EXIF value (tag `tag` of `ifd`, in a structure that is big-endian if `big`) as JSON:
/// text as a string (a coded text's too: see [`coded_text`]); one number as a number,
/// several as a list; a rational as its value (a fraction worked out); bytes (UNDEFINED,
/// BYTE) as a list of numbers, or text if they are printable ASCII (a version, "0230").
fn exif_json(ifd: Ifd, tag: u16, v: &Value, big: bool) -> Json {
    if let Some(t) = v.text() {
        return Json::from(t);
    }
    if let Value::Undefined(b) = v
        && coded_text(ifd, tag)
        && let Some(t) = text_of_coded(b, big)
    {
        return Json::from(t);
    }
    match v {
        Value::Undefined(b) | Value::Bytes(b)
            if !b.is_empty() && b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') =>
        {
            Json::from(String::from_utf8_lossy(b).into_owned())
        }
        Value::Undefined(b) | Value::Bytes(b) => Json::from(b.clone()),
        _ => {
            // Whole numbers (SHORT, LONG and their signed kinds) as integers; the rest (fractions,
            // floats) as numbers with a point.
            let whole = matches!(
                v,
                Value::Shorts(_) | Value::Longs(_) | Value::SBytes(_) | Value::SShorts(_) | Value::SLongs(_)
            );
            let n: Vec<Json> =
                v.numbers().into_iter().map(|x| if whole { Json::from(x as i64) } else { Json::from(x) }).collect();
            match &n[..] {
                [one] => one.clone(),
                _ => Json::from(n),
            }
        }
    }
}

/// An XMP property's key: "prefix:name" for the usual prefixes, else "{namespace}name".
fn xmp_key(name: &Name) -> String {
    match xmp::usual_prefix(&name.ns) {
        Some(p) => format!("{p}:{}", name.local),
        None => format!("{{{}}}{}", name.ns, name.local),
    }
}

/// The name a key gives (see [`xmp_key`]).
fn xmp_name(key: &str) -> Option<Name> {
    let (ns, local) = match key.strip_prefix('{') {
        Some(rest) => rest.split_once('}')?,
        None => {
            let (prefix, local) = key.split_once(':')?;
            (xmp::namespace_of(prefix)?, local)
        }
    };
    // Written as an element's name, it must be one XML takes, or the packet would not read.
    let mut chars = local.chars();
    let name = chars.next().is_some_and(name_start) && chars.all(name_char);
    (name && !ns.is_empty()).then(|| Name::new(ns, local))
}

/// Whether `c` may begin an XML name (XML 1.0, fifth edition, but the colon, which a
/// namespace's prefix ends with).
fn name_start(c: char) -> bool {
    matches!(c,
        'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// Whether `c` may be in an XML name after its first character (see [`name_start`]).
fn name_char(c: char) -> bool {
    name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// An XMP value as JSON: text as a string; an alternative as {language: text}; another array
/// as a list; a structure as {field: value}; a qualified value as its value.
fn xmp_json(v: &xmp::Value) -> Json {
    match v {
        xmp::Value::Text { text, .. } | xmp::Value::Uri(text) | xmp::Value::Literal(text) => Json::from(text.clone()),
        xmp::Value::Array { kind: ArrayKind::Alt, items } => Json::Object(
            items
                .iter()
                .map(|i| match i {
                    xmp::Value::Text { text, lang } => {
                        (lang.clone().unwrap_or_else(|| "x-default".into()), Json::from(text.clone()))
                    }
                    other => ("x-default".into(), xmp_json(other)),
                })
                .collect(),
        ),
        xmp::Value::Array { items, .. } => Json::from(items.iter().map(xmp_json).collect::<Vec<_>>()),
        xmp::Value::Struct(fields) => Json::Object(fields.iter().map(|(n, v)| (xmp_key(n), xmp_json(v))).collect()),
        xmp::Value::Qualified { value, .. } => xmp_json(value),
    }
}

/// An XMP value from JSON given for property `name`: a string as text (as the default
/// language, for the properties that are alternatives: title, description, rights); a list
/// as an array (ordered for creators, unordered otherwise); an object as an alternative if
/// its keys are languages, else a structure.
fn xmp_value(name: &Name, v: &Json) -> Result<xmp::Value, String> {
    let alternative = name.ns == xmp::DC && ["title", "description", "rights"].contains(&name.local.as_str());
    Ok(match v {
        Json::String(s) if alternative => xmp::Value::Array {
            kind: ArrayKind::Alt,
            items: vec![xmp::Value::Text { text: s.clone(), lang: Some("x-default".into()) }],
        },
        Json::String(s) => xmp::Value::text(s.clone()),
        Json::Number(n) => xmp::Value::text(n.to_string()),
        Json::Bool(b) => xmp::Value::text(if *b { "True" } else { "False" }),
        Json::Array(items) => {
            let ordered = name.ns == xmp::DC && name.local == "creator";
            let items = items.iter().map(|i| xmp_value(&Name::new("", ""), i)).collect::<Result<Vec<_>, _>>()?;
            xmp::Value::Array { kind: if ordered { ArrayKind::Seq } else { ArrayKind::Bag }, items }
        }
        Json::Object(fields) if alternative || fields.keys().all(|k| looks_like_language(k)) => xmp::Value::Array {
            kind: ArrayKind::Alt,
            items: fields
                .iter()
                .map(|(lang, t)| {
                    let text = t.as_str().ok_or_else(|| format!("{}'s {lang} must be text", name.local))?;
                    Ok(xmp::Value::Text { text: text.to_owned(), lang: Some(lang.clone()) })
                })
                .collect::<Result<Vec<_>, String>>()?,
        },
        Json::Object(fields) => xmp::Value::Struct(
            fields
                .iter()
                .map(|(k, v)| {
                    let field =
                        xmp_name(k).ok_or_else(|| format!("{k} is no XMP name (prefix:name, or {{namespace}}name)"))?;
                    Ok((field.clone(), xmp_value(&field, v)?))
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Json::Null => return Err("null is not a value (to remove a property, give it in remove)".into()),
    })
}

/// Whether `k` reads as a language tag (x-default, en, en-US, zh-Hant-TW).
fn looks_like_language(k: &str) -> bool {
    k == "x-default"
        || (!k.is_empty()
            && k.split('-')
                .all(|part| (1..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())))
}

/// An EXIF value from JSON given for `tag` of `ifd`, of the TIFF type the tag has
/// (`existing`, if it is there), else the type EXIF gives it ([`tag_type`]), else as the JSON
/// reads: text as ASCII; a whole number as a SHORT (or a LONG past 65535); another number as
/// a RATIONAL; a list of numbers likewise each. So what the metadata query gives comes back as
/// it was: bytes (UNDEFINED, BYTE: a maker note, a version) given as a list of numbers, or as
/// text, stay bytes; a coded text given as text (see [`coded_text`]) is written with its code
/// (in a structure that is big-endian if `big`).
fn exif_value(ifd: Ifd, tag: u16, existing: Option<u16>, v: &Json, big: bool) -> Result<Value, String> {
    if let Json::String(s) = v
        && coded_text(ifd, tag)
    {
        return Ok(Value::Undefined(coded(s, big)));
    }
    let numbers = |v: &Json| -> Result<Vec<f64>, String> {
        match v {
            Json::Number(n) => Ok(vec![n.as_f64().unwrap_or(0.0)]),
            Json::Array(items) => items
                .iter()
                .map(|i| i.as_f64().ok_or_else(|| format!("{} takes numbers", tags::name(ifd, tag))))
                .collect(),
            _ => Err(format!("{} takes a number or a list of them", tags::name(ifd, tag))),
        }
    };
    let bytes = |v: &Json| -> Result<Vec<u8>, String> {
        match v {
            Json::String(s) => Ok(s.as_bytes().to_vec()),
            _ => numbers(v)?
                .into_iter()
                .map(|x| {
                    (x.fract() == 0.0 && (0.0..=255.0).contains(&x))
                        .then_some(x as u8)
                        .ok_or_else(|| format!("{} takes bytes (0 to 255) or text", tags::name(ifd, tag)))
                })
                .collect(),
        }
    };
    Ok(match (existing.or_else(|| tag_type(ifd, tag)), v) {
        (Some(7), v) => Value::Undefined(bytes(v)?),
        (Some(1), v) => Value::Bytes(bytes(v)?),
        (Some(129), Json::String(s)) => Value::Utf8(s.clone()),
        (_, Json::String(s)) => Value::Ascii(s.clone()),
        (Some(6), v) => Value::SBytes(numbers(v)?.into_iter().map(|x| x.clamp(-128.0, 127.0) as i8).collect()),
        (Some(8), v) => Value::SShorts(numbers(v)?.into_iter().map(|x| x.clamp(-32768.0, 32767.0) as i16).collect()),
        (Some(9), v) => Value::SLongs(numbers(v)?.into_iter().map(|x| x as i32).collect()),
        (Some(11), v) => Value::Floats(numbers(v)?.into_iter().map(|x| x as f32).collect()),
        (Some(12), v) => Value::Doubles(numbers(v)?),
        (Some(5), v) => Value::Rationals(numbers(v)?.into_iter().map(Value::rational).collect()),
        (Some(10), v) => Value::SRationals(numbers(v)?.into_iter().map(Value::srational).collect()),
        (Some(4), v) => Value::Longs(numbers(v)?.into_iter().map(|x| x.max(0.0) as u32).collect()),
        (Some(3), v) => Value::Shorts(numbers(v)?.into_iter().map(|x| x.clamp(0.0, 65535.0) as u16).collect()),
        (_, v) => {
            let n = numbers(v)?;
            if n.iter().all(|x| x.fract() == 0.0 && (0.0..=65535.0).contains(x)) {
                Value::Shorts(n.into_iter().map(|x| x as u16).collect())
            } else if n.iter().all(|x| x.fract() == 0.0 && *x >= 0.0) {
                Value::Longs(n.into_iter().map(|x| x as u32).collect())
            } else {
                Value::Rationals(n.into_iter().map(Value::rational).collect())
            }
        }
    })
}

/// The TIFF type EXIF gives a tag that is not text, where it fixes one: those of time,
/// aperture, distance and place that are fractions (RATIONAL, 5; SRATIONAL, 10, for those that
/// may be negative), counts that are LONG (4), and bytes (UNDEFINED, 7; BYTE, 1): versions, a
/// maker note, a comment, what a scene and its source were.
fn tag_type(ifd: Ifd, tag: u16) -> Option<u16> {
    match (ifd, tag) {
        (
            Ifd::Exif,
            0x9000 | 0xA000 | 0x9101 | 0x927C | 0x9286 | 0xA300 | 0xA301 | 0xA302 | 0x8828 | 0xA20C | 0xA40B,
        ) => Some(7),
        (Ifd::Gps, 0x001B | 0x001C) | (Ifd::Interop, 0x0002) => Some(7),
        (Ifd::Gps, 0x0000 | 0x0005) => Some(1),
        (Ifd::Image | Ifd::Thumbnail, 0x011A | 0x011B) => Some(5),
        (
            Ifd::Exif,
            0x829A | 0x829D | 0x9102 | 0x9202 | 0x9205 | 0x9206 | 0x920A | 0xA20B | 0xA20E | 0xA20F | 0xA215 | 0xA404
            | 0xA432 | 0xA500,
        ) => Some(5),
        (Ifd::Exif, 0x9201 | 0x9203 | 0x9204 | 0x9400 | 0x9405) => Some(10),
        (Ifd::Exif, 0xA002 | 0xA003) => Some(4),
        (
            Ifd::Gps,
            0x0002 | 0x0004 | 0x0006 | 0x0007 | 0x000B | 0x000D | 0x000F | 0x0011 | 0x0014 | 0x0016 | 0x0018 | 0x001A
            | 0x001F,
        ) => Some(5),
        _ => None,
    }
}

/// `meta` changed as `changes` (an object) says, as scripts change it: `description` (fields
/// as `to_json` gives them; those not given kept), `exif` ({ifd: {tag: value or null}}),
/// `iptc` ({"record:number": text, list of texts, or null}), `xmp` ({key: value or null}),
/// null removing what it names.
pub fn changed(original: &Metadata, changes: &Json) -> Result<Metadata, String> {
    let changes = changes.as_object().ok_or("the changes must be an object")?;
    for key in changes.keys() {
        if !["description", "exif", "iptc", "xmp"].contains(&key.as_str()) {
            return Err(format!("{key} is not one of description, exif, iptc and xmp"));
        }
    }
    // An IIM a program unaware of XMP changed since the XMP was written is carried to the XMP
    // first, so that what this changes is changed in an IIM and XMP that agree (see `sync`):
    // unless all it changes is EXIF that neither holds, which leaves them as they are.
    // (A tag as the change names it: in any case, or by its number.)
    let described = changes.get("exif").and_then(|e| e.get("image")).and_then(Json::as_object).is_some_and(|given| {
        given.keys().any(|k| tags::named(k, Some(Ifd::Image)).is_some_and(|(_, tag)| sync::held_elsewhere(tag)))
    });
    let carries = described || ["description", "iptc", "xmp"].iter().any(|k| changes.contains_key(*k));
    let mut meta = if carries { sync::reconciled(original) } else { original.clone() };
    let mut set = sync::Set::default();
    if let Some(d) = changes.get("description") {
        let mut desc = description(&meta);
        set_description(&mut desc, d)?;
        meta = super::with_description(&meta, &desc);
    }
    if let Some(e) = changes.get("exif") {
        let ifds = e.as_object().ok_or("exif must be an object of IFDs")?;
        let mut exif = meta.exif.as_deref().and_then(Exif::read).unwrap_or_default();
        let big = exif.big_endian();
        for (ifd_name, fields) in ifds {
            let ifd = Ifd::ALL
                .into_iter()
                .find(|i| i.name() == ifd_name)
                .ok_or_else(|| format!("{ifd_name} is not an IFD (image, exif, gps, interop, thumbnail)"))?;
            let fields = fields.as_object().ok_or_else(|| format!("exif's {ifd_name} must be an object of tags"))?;
            // The thumbnail's IFD takes IFD0's tags.
            let lookup = if ifd == Ifd::Thumbnail { Ifd::Image } else { ifd };
            for (name, v) in fields {
                let (_, tag) = tags::named(name, Some(lookup))
                    .ok_or_else(|| format!("{name} is not a tag of exif's {ifd_name}"))?;
                // A coded text given back as the query read it is left as it was, its code and
                // padding with it.
                let as_read = coded_text(ifd, tag)
                    && matches!((v, exif.get(ifd, tag)), (Json::String(s), Some(Value::Undefined(b)))
                        if text_of_coded(&b, big).as_deref() == Some(s.as_str()));
                let before = exif.get(ifd, tag);
                if v.is_null() {
                    exif.remove(ifd, tag);
                } else if !as_read {
                    let existing = exif.kind_of(ifd, tag);
                    exif.set(ifd, tag, &exif_value(ifd, tag, existing, v, big)?);
                }
                // Carried only if it changed: given back as it was, it says nothing new.
                if ifd == Ifd::Image && exif.get(ifd, tag) != before {
                    set.exif.push(tag);
                }
            }
        }
        exif.trim();
        meta.exif = (!exif.is_empty()).then(|| Arc::from(exif.into_bytes()));
    }
    if let Some(i) = changes.get("iptc") {
        let sets = i.as_object().ok_or("iptc must be an object of datasets")?;
        let mut iim = meta.iptc.as_deref().and_then(iptc::Iim::read).unwrap_or_default();
        for (key, v) in sets {
            let (r, n) = key
                .split_once(':')
                .and_then(|(r, n)| Some((r.parse::<u8>().ok()?, n.parse::<u8>().ok()?)))
                .ok_or_else(|| format!("{key} is no dataset (record:number, as 2:120)"))?;
            let texts: Vec<String> = match v {
                Json::Null => Vec::new(),
                Json::String(s) => vec![s.clone()],
                Json::Array(items) => items
                    .iter()
                    .map(|t| t.as_str().map(str::to_owned).ok_or_else(|| format!("iptc's {key} takes text")))
                    .collect::<Result<_, _>>()?,
                _ => return Err(format!("iptc's {key} takes text, a list of texts, or null")),
            };
            // The date and time created as IIM writes them, or neither IIM's readers nor XMP
            // could take them.
            let (well_formed, form): (fn(&[u8]) -> bool, &str) = match (r, n) {
                (2, 55) => (|b| b.len() == 8 && b.iter().all(u8::is_ascii_digit), "CCYYMMDD, as 20240131"),
                (2, 60) => (
                    |b| {
                        let zone =
                            b.len() == 11 && matches!(b[6], b'+' | b'-') && b[7..].iter().all(u8::is_ascii_digit);
                        (b.len() == 6 || zone) && b[..6].iter().all(u8::is_ascii_digit)
                    },
                    "HHMMSS and the zone, as 134501+0100",
                ),
                _ => (|_| true, ""),
            };
            if let Some(bad) = texts.iter().find(|t| !well_formed(t.as_bytes())) {
                return Err(format!("iptc's {key} takes {form}, not \"{bad}\""));
            }
            let before = iim.texts(r, n);
            iim.set_texts(r, n, &texts);
            if r == 2 && iim.texts(r, n) != before {
                set.iim.push(n);
            }
        }
        meta.iptc = (!iim.is_empty()).then(|| Arc::from(iim.bytes()));
    }
    if let Some(x) = changes.get("xmp") {
        let props = x.as_object().ok_or("xmp must be an object of properties")?;
        let mut packet = meta.xmp.as_deref().and_then(Xmp::read).unwrap_or_default();
        for (key, v) in props {
            let name =
                xmp_name(key).ok_or_else(|| format!("{key} is no XMP name (prefix:name, or {{namespace}}name)"))?;
            let before = packet.get(&name.ns, &name.local).cloned();
            if v.is_null() {
                packet.remove(&name.ns, &name.local);
            } else {
                packet.set(name.clone(), xmp_value(&name, v)?);
            }
            if packet.get(&name.ns, &name.local) != before.as_ref() {
                set.xmp.push(name);
            }
        }
        meta.xmp = (!packet.properties.is_empty()).then(|| Arc::from(packet.packet().into_bytes()));
    }
    // Each change carried to what holds the same elsewhere.
    Ok(as_it_was_where_unchanged(original, sync::carried(&meta, &set)))
}

/// `new` with each part that holds what `old`'s does given back `old`'s bytes (written anew,
/// XMP differs from a file's in its form; EXIF in where its values lie), so that a part a
/// change did not change is not taken, or written, as changed.
fn as_it_was_where_unchanged(old: &Metadata, mut new: Metadata) -> Metadata {
    let exif = |m: &Metadata| m.exif.as_deref().and_then(Exif::read).map(|e| Ifd::ALL.map(|ifd| e.tags(ifd)));
    if new.exif.is_some() == old.exif.is_some() && exif(&new) == exif(old) {
        new.exif.clone_from(&old.exif);
    }
    let xmp = |m: &Metadata| m.xmp.as_deref().and_then(Xmp::read).map(|x| x.properties);
    if new.xmp.is_some() == old.xmp.is_some() && xmp(&new) == xmp(old) {
        new.xmp.clone_from(&old.xmp);
    }
    let iim = |m: &Metadata| m.iptc.as_deref().and_then(iptc::Iim::read);
    if new.iptc.is_some() == old.iptc.is_some() && iim(&new) == iim(old) {
        new.iptc.clone_from(&old.iptc);
    }
    new
}

/// Changes `d` as `given` (an object of [`description_json`]'s fields) says.
fn set_description(d: &mut Description, given: &Json) -> Result<(), String> {
    let fields = given.as_object().ok_or("description must be an object")?;
    let text = |k: &str, v: &Json| v.as_str().map(str::to_owned).ok_or_else(|| format!("description's {k} takes text"));
    let texts = |k: &str, v: &Json| -> Result<Vec<String>, String> {
        match v {
            Json::String(s) => Ok(vec![s.clone()]),
            Json::Array(items) => items.iter().map(|i| text(k, i)).collect(),
            _ => Err(format!("description's {k} takes a list of texts")),
        }
    };
    for (k, v) in fields {
        match k.as_str() {
            "title" => d.title = text(k, v)?,
            "caption" => d.caption = text(k, v)?,
            "email" => d.email = text(k, v)?,
            "website" => d.website = text(k, v)?,
            "copyright" => d.copyright = text(k, v)?,
            "copyright_url" => d.copyright_url = text(k, v)?,
            "creators" => d.creators = texts(k, v)?,
            "keywords" => d.keywords = texts(k, v)?,
            "copyright_status" => {
                d.copyrighted = match v.as_str() {
                    Some("unknown") => None,
                    Some("copyrighted") => Some(true),
                    Some("public domain") => Some(false),
                    _ => return Err("description's copyright_status is copyrighted, public domain or unknown".into()),
                }
            }
            other => return Err(format!("{other} is not a field of the description")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_read_all_of_it_and_change_any_of_it() {
        let changes = json!({
            "description": { "title": "Harbour", "creators": ["Ann", "Bob"], "copyright_status": "copyrighted" },
            "exif": {
                "image": { "Make": "Canon", "Orientation": 1 },
                "exif": { "ExposureTime": 0.004, "DateTimeOriginal": "2024:05:06 14:03:22", "FNumber": 2.8 },
                "gps": { "GPSLatitudeRef": "N", "GPSLatitude": [48, 51, 29.6] }
            },
            "iptc": { "2:90": "Paris", "2:25": ["sea", "boat"] },
            "xmp": { "xmp:Rating": 4, "{http://example.com/mine/}Odd": "kept", "dc:rights": "© Ann" }
        });
        let meta = changed(&Metadata::default(), &changes).unwrap();
        let all = to_json(&meta);
        assert_eq!(all["description"]["title"], "Harbour");
        assert_eq!(all["description"]["creators"], json!(["Ann", "Bob"]));
        assert_eq!(all["description"]["copyright"], "© Ann");
        assert_eq!(all["exif"]["image"]["Make"], "Canon");
        assert_eq!(all["exif"]["exif"]["ExposureTime"], 0.004);
        assert_eq!(all["exif"]["exif"]["FNumber"], 2.8);
        assert_eq!(all["exif"]["gps"]["GPSLatitude"], json!([48.0, 51.0, 29.6]));
        assert_eq!(all["iptc"]["2:25"], json!(["sea", "boat"]));
        assert_eq!(all["xmp"]["xmp:Rating"], "4");
        assert_eq!(all["xmp"]["{http://example.com/mine/}Odd"], "kept");
        assert_eq!(all["capture"]["Shutter"], "1/250 s");
        assert_eq!(all["capture"]["Taken"], "2024-05-06 14:03:22");
        // Null removes; the shift of a capture time is a text set.
        let again = changed(
            &meta,
            &json!({ "exif": { "gps": { "GPSLatitude": null }, "exif": { "DateTimeOriginal": "2024:05:06 15:03:22" } },
                     "iptc": { "2:90": null }, "xmp": { "xmp:Rating": null } }),
        )
        .unwrap();
        let all = to_json(&again);
        assert!(all["exif"]["gps"].get("GPSLatitude").is_none() && all["iptc"].get("2:90").is_none());
        assert!(all["xmp"].get("xmp:Rating").is_none());
        assert_eq!(all["exif"]["exif"]["DateTimeOriginal"], "2024:05:06 15:03:22");
        // What cannot be: said.
        for bad in [
            json!({ "exif": { "image": { "NoSuchTag": 1 } } }),
            json!({ "exif": { "nowhere": {} } }),
            json!({ "iptc": { "two:five": "x" } }),
            json!({ "xmp": { "unknownprefix:x": "y" } }),
            json!({ "description": { "colour": "red" } }),
            json!({ "pixels": {} }),
        ] {
            assert!(changed(&meta, &bad).is_err(), "{bad} taken");
        }
    }

    #[test]
    fn what_the_query_gives_goes_back_as_it_was_and_names_xml_cannot_hold_are_refused() {
        let mut e = Exif::new();
        e.set(Ifd::Exif, 0x9000, &Value::Undefined(b"0232".to_vec()));
        e.set(Ifd::Exif, 0x927C, &Value::Undefined(vec![1, 2, 250]));
        e.set(Ifd::Gps, 0x0000, &Value::Bytes(vec![2, 3, 0, 0]));
        let meta = Metadata { exif: Some(Arc::from(e.into_bytes())), ..Metadata::default() };
        let all = to_json(&meta);
        let copied = changed(&Metadata::default(), &json!({ "exif": all["exif"] })).unwrap();
        let back = Exif::read(copied.exif.as_deref().unwrap()).unwrap();
        assert_eq!(back.get(Ifd::Exif, 0x9000), Some(Value::Undefined(b"0232".to_vec())));
        assert_eq!(back.get(Ifd::Exif, 0x927C), Some(Value::Undefined(vec![1, 2, 250])));
        assert_eq!(back.get(Ifd::Gps, 0x0000), Some(Value::Bytes(vec![2, 3, 0, 0])));
        // A tag no table knows keeps the type it has.
        let mut e = Exif::read(meta.exif.as_deref().unwrap()).unwrap();
        e.set(Ifd::Exif, 0xC000, &Value::Undefined(vec![7, 8, 9]));
        let odd = Metadata { exif: Some(Arc::from(e.into_bytes())), ..Metadata::default() };
        let again = changed(&odd, &json!({ "exif": to_json(&odd)["exif"] })).unwrap();
        assert_eq!(
            Exif::read(again.exif.as_deref().unwrap()).unwrap().get(Ifd::Exif, 0xC000),
            Some(Value::Undefined(vec![7, 8, 9]))
        );
        assert!(changed(&meta, &json!({ "xmp": { "dc:café": "x" } })).is_ok(), "a letter XML takes refused");
        for bad in ["dc:my title", "dc:1st", "{}x", "dc:", "dc:x²", "dc:ªb", "dc:½"] {
            assert!(changed(&meta, &json!({ "xmp": { bad: "x" } })).is_err(), "{bad} taken");
        }
        // An IIM title set alone reaches XMP, which a reader takes first.
        let titled = changed(&meta, &json!({ "xmp": { "dc:title": "Harbour" } })).unwrap();
        let retitled = changed(&titled, &json!({ "iptc": { "2:5": "Quay" } })).unwrap();
        let x = Xmp::read(retitled.xmp.as_deref().unwrap()).unwrap();
        assert_eq!(x.text(xmp::DC, "title").as_deref(), Some("Quay"));
        assert_eq!(to_json(&retitled)["description"]["title"], "Quay");
    }

    #[test]
    fn a_change_to_what_another_holds_too_is_carried_there_unless_it_was_changed_too() {
        // A photograph as a program keeping them in step left it: the title, caption and city
        // in both IIM and XMP, the caption in EXIF too.
        let start = changed(
            &Metadata::default(),
            &json!({
                "exif": { "image": { "ImageDescription": "Boats" } },
                "iptc": { "2:5": "Harbour", "2:120": "Boats", "2:90": "Paris" },
            }),
        )
        .unwrap();
        let all = to_json(&start);
        assert_eq!(all["xmp"]["dc:title"], json!({ "x-default": "Harbour" }));
        assert_eq!(all["xmp"]["dc:description"], json!({ "x-default": "Boats" }));
        assert_eq!(all["xmp"]["photoshop:City"], "Paris");
        // A dataset removed is removed from XMP too, not brought back from it.
        let untitled = changed(&start, &json!({ "iptc": { "2:5": null } })).unwrap();
        let all = to_json(&untitled);
        assert!(all["xmp"].get("dc:title").is_none(), "{all}");
        assert_eq!(all["description"]["title"], "");
        // Any dataset XMP holds is carried, not only the description's.
        let moved = changed(&start, &json!({ "iptc": { "2:90": "Lyon" } })).unwrap();
        assert_eq!(to_json(&moved)["xmp"]["photoshop:City"], "Lyon");
        // EXIF's caption changed beside a keyword: kept, and carried to the IIM and XMP.
        let both = changed(
            &start,
            &json!({ "exif": { "image": { "ImageDescription": "New caption" } }, "iptc": { "2:25": ["boat"] } }),
        )
        .unwrap();
        let all = to_json(&both);
        assert_eq!(all["exif"]["image"]["ImageDescription"], "New caption");
        assert_eq!(all["iptc"]["2:120"], json!(["New caption"]));
        assert_eq!(all["description"]["caption"], "New caption");
        assert_eq!(all["xmp"]["dc:subject"], json!(["boat"]));
        // EXIF's and IIM's captions changed at once: each keeps its own.
        let captions = changed(
            &start,
            &json!({ "exif": { "image": { "ImageDescription": "Mine" } }, "iptc": { "2:120": "Theirs" } }),
        )
        .unwrap();
        let all = to_json(&captions);
        assert_eq!(all["exif"]["image"]["ImageDescription"], "Mine");
        assert_eq!(all["iptc"]["2:120"], json!(["Theirs"]));
        // Both changed at once: each keeps what it was given (and readers take XMP's).
        let apart = changed(&start, &json!({ "iptc": { "2:5": "Quay" }, "xmp": { "dc:title": "Pier" } })).unwrap();
        let all = to_json(&apart);
        assert_eq!(all["iptc"]["2:5"], json!(["Quay"]));
        assert_eq!(all["description"]["title"], "Pier");
        // The date and time created, one property in XMP.
        let dated = changed(&start, &json!({ "iptc": { "2:55": "20240131", "2:60": "134501+0100" } })).unwrap();
        assert_eq!(to_json(&dated)["xmp"]["photoshop:DateCreated"], "2024-01-31T13:45:01+01:00");
        let redated = changed(&dated, &json!({ "xmp": { "photoshop:DateCreated": "2023-07-04T09:30Z" } })).unwrap();
        let all = to_json(&redated);
        assert_eq!(all["iptc"]["2:55"], json!(["20230704"]));
        assert_eq!(all["iptc"]["2:60"], json!(["093000+0000"]));
    }

    #[test]
    fn xmp_changed_where_a_program_unaware_of_it_changed_the_iim_since_keeps_what_the_iim_says() {
        let start = changed(&Metadata::default(), &json!({ "iptc": { "2:5": "Old", "2:90": "Paris" } })).unwrap();
        // Then a program unaware of XMP changed the IIM's city (its digest no longer agrees).
        let mut iim = iptc::Iim::read(start.iptc.as_deref().unwrap()).unwrap();
        iim.set_texts(2, 90, &["Rome".into()]);
        let stale = Metadata { iptc: Some(Arc::from(iim.bytes())), iptc_changed: true, ..start };
        let retitled = changed(&stale, &json!({ "xmp": { "dc:title": "New" } })).unwrap();
        assert!(!retitled.iptc_changed, "the IIM and XMP do not agree");
        let all = to_json(&retitled);
        assert_eq!(all["description"]["title"], "New");
        assert_eq!(all["iptc"]["2:5"], json!(["New"]));
        assert_eq!(all["xmp"]["photoshop:City"], "Rome", "the IIM's newer city lost");
        // An export of it, likewise: the IIM's city, which a reader would have taken.
        let out = crate::metadata::Exported {
            width: 1,
            height: 1,
            ppi: 72.0,
            exif_now: "2024:01:01 00:00:00".into(),
            exif_zone: "+00:00".into(),
            xmp_now: "2024-01-01T00:00:00Z".into(),
            software: "Opal Studio".into(),
        };
        let prepared = crate::metadata::for_export(&stale, crate::metadata::Keep::All, &out);
        assert_eq!(prepared.xmp.unwrap().text(xmp::PHOTOSHOP, "City").as_deref(), Some("Rome"));
    }

    #[test]
    fn a_comment_is_written_with_its_character_code_and_read_without_it() {
        for (text, code, rest) in [
            ("Shot at dawn", &b"ASCII\0\0\0"[..], b"Shot at dawn".to_vec()),
            ("Café", b"UNICODE\0", b"C\0a\0f\0\xe9\0".to_vec()),
        ] {
            let meta = changed(
                &Metadata::default(),
                &json!({ "exif": { "exif": { "UserComment": text }, "gps": { "GPSProcessingMethod": text } } }),
            )
            .unwrap();
            let e = Exif::read(meta.exif.as_deref().unwrap()).unwrap();
            for (ifd, tag) in [(Ifd::Exif, 0x9286), (Ifd::Gps, 0x001B)] {
                assert_eq!(e.get(ifd, tag), Some(Value::Undefined([code, &rest[..]].concat())), "{text}");
            }
            let all = to_json(&meta);
            assert_eq!(all["exif"]["exif"]["UserComment"], text);
            assert_eq!(all["exif"]["gps"]["GPSProcessingMethod"], text);
        }
    }

    #[test]
    fn a_change_that_changes_nothing_leaves_the_bytes_as_they_were() {
        // XMP as another program writes it: no wrapper, no padding.
        let packet = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Harbour</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let meta = Metadata { xmp: Some(Arc::from(packet.as_bytes())), ..Metadata::default() };
        for same in [json!({ "description": { "title": "Harbour" } }), json!({ "xmp": { "dc:title": "Harbour" } })] {
            assert!(changed(&meta, &same).unwrap() == meta, "{same} changed it");
        }
        assert!(changed(&meta, &json!({ "description": { "title": "Quay" } })).unwrap() != meta);
    }

    /// What `start` holds, its IIM's city since changed to "Rome" by a program unaware of XMP
    /// (its digest no longer agreeing); XMP's city "Paris".
    fn stale() -> Metadata {
        let start = changed(
            &Metadata::default(),
            &json!({ "exif": { "image": { "Artist": "Ann", "Make": "Canon" } }, "iptc": { "2:5": "Old", "2:90": "Paris" } }),
        )
        .unwrap();
        let mut iim = iptc::Iim::read(start.iptc.as_deref().unwrap()).unwrap();
        iim.set_texts(2, 90, &["Rome".into()]);
        Metadata { iptc: Some(Arc::from(iim.bytes())), iptc_changed: true, ..start }
    }

    fn city(meta: &Metadata) -> Option<String> {
        Xmp::read(meta.xmp.as_deref()?)?.text(xmp::PHOTOSHOP, "City")
    }

    #[test]
    fn what_describes_the_image_written_where_the_iim_is_newer_carries_the_iim_first() {
        // As File Info writes it: the caption changed.
        let meta = stale();
        let mut d = description(&meta);
        d.caption = "Boats".into();
        let described = crate::metadata::with_description(&meta, &d);
        assert!(!described.iptc_changed);
        assert_eq!(city(&described).as_deref(), Some("Rome"), "the IIM's newer city lost");
        // A described EXIF tag named as the change may name it: in any case, or by its number.
        for name in ["artist", "0x013B"] {
            let set = changed(&meta, &json!({ "exif": { "image": { name: "Bob" } } })).unwrap();
            assert_eq!(city(&set).as_deref(), Some("Rome"), "{name}: the IIM's newer city lost");
            assert_eq!(to_json(&set)["iptc"]["2:80"], json!(["Bob"]), "{name}");
        }
    }

    #[test]
    fn a_time_set_alone_leaves_xmp_s_date_and_a_date_not_written_as_iim_writes_it_is_refused() {
        let dated =
            changed(&Metadata::default(), &json!({ "xmp": { "photoshop:DateCreated": "2024-01-31T10:00:00+01:00" } }))
                .unwrap();
        // (No IIM yet, so the date was carried nowhere: one made by the time set alone.)
        let timed = changed(&dated, &json!({ "iptc": { "2:60": "120000+0000" } })).unwrap();
        assert_eq!(to_json(&timed)["xmp"]["photoshop:DateCreated"], "2024-01-31T10:00:00+01:00");
        for (key, bad) in [("2:55", "2024-01-31"), ("2:55", "202401"), ("2:60", "12:00"), ("2:60", "120000 0000")] {
            let refused = changed(&dated, &json!({ "iptc": { key: bad } }));
            assert!(refused.is_err(), "{key} {bad} taken");
        }
        assert!(changed(&dated, &json!({ "iptc": { "2:55": "20240131", "2:60": "120000" } })).is_ok());
    }

    #[test]
    fn exif_s_last_described_tags_removed_are_removed_where_they_were_carried() {
        let only = changed(
            &Metadata::default(),
            &json!({ "exif": { "image": { "Artist": "Ann", "Copyright": "(c) Ann" } }, "iptc": { "2:5": "Harbour" } }),
        )
        .unwrap();
        let all = to_json(&only);
        assert_eq!((all["iptc"]["2:80"].clone(), all["xmp"]["dc:creator"].clone()), (json!(["Ann"]), json!(["Ann"])));
        let gone = changed(&only, &json!({ "exif": { "image": { "Artist": null, "Copyright": null } } })).unwrap();
        assert!(gone.exif.is_none(), "EXIF left");
        let all = to_json(&gone);
        assert!(all["iptc"].get("2:80").is_none() && all["xmp"].get("dc:creator").is_none(), "{all}");
        assert!(all["iptc"].get("2:116").is_none() && all["xmp"].get("dc:rights").is_none(), "{all}");
    }

    #[test]
    fn a_comment_given_back_as_the_query_read_it_stays_as_it_was() {
        // A camera's: no code (eight zeros) and a long run of padding.
        let mut e = Exif::new();
        e.set(Ifd::Exif, 0x9286, &Value::Undefined([&[0u8; 8][..], &[0u8; 64]].concat()));
        let meta = Metadata { exif: Some(Arc::from(e.into_bytes())), ..Metadata::default() };
        assert_eq!(to_json(&meta)["exif"]["exif"]["UserComment"], "");
        let back = changed(&meta, &json!({ "exif": to_json(&meta)["exif"] })).unwrap();
        assert!(back == meta, "the comment's bytes changed");
    }

    #[test]
    fn a_part_given_back_unchanged_carries_nothing_and_a_removal_that_empties_the_iim_is_carried() {
        // A camera's caption in EXIF; the user's in XMP.
        let mut e = Exif::new();
        e.set(Ifd::Image, 0x010E, &Value::Ascii("OLYMPUS DIGITAL CAMERA".into()));
        let mut x = Xmp::default();
        x.set_default_text(xmp::DC, "description", "Boats at dusk");
        let meta = Metadata {
            exif: Some(Arc::from(e.into_bytes())),
            xmp: Some(Arc::from(x.packet().into_bytes())),
            ..Metadata::default()
        };
        let back = changed(&meta, &json!({ "exif": to_json(&meta)["exif"] })).unwrap();
        assert_eq!(to_json(&back)["xmp"]["dc:description"], json!({ "x-default": "Boats at dusk" }));
        // Likewise an IIM's title, and an XMP's, given back as they were.
        let mut iim = iptc::Iim::default();
        iim.set_texts(2, 5, &["Quay".into()]);
        let mut x = Xmp::default();
        x.set_default_text(xmp::DC, "title", "Pier");
        let apart = Metadata {
            iptc: Some(Arc::from(iim.bytes())),
            xmp: Some(Arc::from(x.packet().into_bytes())),
            ..Metadata::default()
        };
        let back = changed(&apart, &json!({ "iptc": to_json(&apart)["iptc"] })).unwrap();
        assert_eq!(to_json(&back)["xmp"]["dc:title"], json!({ "x-default": "Pier" }), "the IIM's title carried");
        let back = changed(&apart, &json!({ "xmp": to_json(&apart)["xmp"] })).unwrap();
        assert_eq!(to_json(&back)["iptc"]["2:5"], json!(["Quay"]), "the XMP's title carried");
        // An IIM of a title alone, removed: the XMP's title goes too.
        let titled = changed(&Metadata::default(), &json!({ "iptc": { "2:5": "Harbour" } })).unwrap();
        let untitled = changed(&titled, &json!({ "iptc": { "2:5": null } })).unwrap();
        assert!(untitled.iptc.is_none(), "the IIM left");
        assert!(to_json(&untitled)["xmp"].get("dc:title").is_none(), "the XMP's title left");
    }
}
