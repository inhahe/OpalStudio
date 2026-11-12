//! Metadata as scripts see it and change it: every EXIF tag (by name, or number if unknown),
//! IIM dataset and XMP property, as JSON, besides the description and capture File Info
//! shows.

use super::exif::{Exif, Ifd, Value};
use super::xmp::{self, ArrayKind, Name, Xmp};
use super::{Description, capture, description, iptc, tags};
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
            let fields: Map<String, Json> =
                e.tags(ifd).into_iter().map(|(tag, v)| (tags::name(ifd, tag), exif_json(&v))).collect();
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

/// An EXIF value as JSON: text as a string; one number as a number, several as a list; a
/// rational as its value (a fraction worked out); bytes (UNDEFINED, BYTE) as a list of numbers,
/// or text if they are printable ASCII (a version, "0230").
fn exif_json(v: &Value) -> Json {
    if let Some(t) = v.text() {
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
    if let Some(rest) = key.strip_prefix('{') {
        let (ns, local) = rest.split_once('}')?;
        return (!local.is_empty()).then(|| Name::new(ns, local));
    }
    let (prefix, local) = key.split_once(':')?;
    let ns = xmp::namespace_of(prefix)?;
    (!local.is_empty()).then(|| Name::new(ns, local))
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

/// An EXIF value from JSON given for `tag` of `ifd`: text as ASCII; a whole number as a SHORT
/// (or a LONG past 65535); another number as a RATIONAL; a list of numbers likewise each.
/// Tags whose type EXIF fixes otherwise are written as their type is ([`tag_type`]).
fn exif_value(ifd: Ifd, tag: u16, v: &Json) -> Result<Value, String> {
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
    let rational = |x: f64| -> [u32; 2] {
        if x.fract() == 0.0 && (0.0..f64::from(u32::MAX)).contains(&x) {
            [x as u32, 1]
        } else {
            let d = 10_000u32;
            [(x * f64::from(d)).round().clamp(0.0, f64::from(u32::MAX)) as u32, d]
        }
    };
    let srational = |x: f64| -> [i32; 2] {
        if x.fract() == 0.0 && x.abs() < f64::from(i32::MAX) {
            [x as i32, 1]
        } else {
            [(x * 10_000.0).round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32, 10_000]
        }
    };
    Ok(match (tag_type(ifd, tag), v) {
        (_, Json::String(s)) => Value::Ascii(s.clone()),
        (Some(5), v) => Value::Rationals(numbers(v)?.into_iter().map(rational).collect()),
        (Some(10), v) => Value::SRationals(numbers(v)?.into_iter().map(srational).collect()),
        (Some(4), v) => Value::Longs(numbers(v)?.into_iter().map(|x| x.max(0.0) as u32).collect()),
        (Some(3), v) => Value::Shorts(numbers(v)?.into_iter().map(|x| x.clamp(0.0, 65535.0) as u16).collect()),
        (_, v) => {
            let n = numbers(v)?;
            if n.iter().all(|x| x.fract() == 0.0 && (0.0..=65535.0).contains(x)) {
                Value::Shorts(n.into_iter().map(|x| x as u16).collect())
            } else if n.iter().all(|x| x.fract() == 0.0 && *x >= 0.0) {
                Value::Longs(n.into_iter().map(|x| x as u32).collect())
            } else {
                Value::Rationals(n.into_iter().map(rational).collect())
            }
        }
    })
}

/// The TIFF type EXIF gives a tag that is not text, where it fixes one: those of time,
/// aperture, distance and place that are fractions (RATIONAL, 5; SRATIONAL, 10, for those that
/// may be negative), and counts that are LONG (4).
fn tag_type(ifd: Ifd, tag: u16) -> Option<u16> {
    match (ifd, tag) {
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
pub fn changed(meta: &Metadata, changes: &Json) -> Result<Metadata, String> {
    let changes = changes.as_object().ok_or("the changes must be an object")?;
    for key in changes.keys() {
        if !["description", "exif", "iptc", "xmp"].contains(&key.as_str()) {
            return Err(format!("{key} is not one of description, exif, iptc and xmp"));
        }
    }
    let mut meta = meta.clone();
    if let Some(d) = changes.get("description") {
        let mut desc = description(&meta);
        set_description(&mut desc, d)?;
        meta = super::with_description(&meta, &desc);
    }
    if let Some(e) = changes.get("exif") {
        let ifds = e.as_object().ok_or("exif must be an object of IFDs")?;
        let mut exif = meta.exif.as_deref().and_then(Exif::read).unwrap_or_default();
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
                if v.is_null() {
                    exif.remove(ifd, tag);
                } else {
                    exif.set(ifd, tag, &exif_value(ifd, tag, v)?);
                }
            }
        }
        exif.trim();
        meta.exif = Some(Arc::from(exif.into_bytes()));
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
            iim.set_texts(r, n, &texts);
        }
        meta.iptc = (!iim.is_empty()).then(|| Arc::from(iim.bytes()));
        meta.iptc_changed = false;
    }
    if let Some(x) = changes.get("xmp") {
        let props = x.as_object().ok_or("xmp must be an object of properties")?;
        let mut packet = meta.xmp.as_deref().and_then(Xmp::read).unwrap_or_default();
        for (key, v) in props {
            let name =
                xmp_name(key).ok_or_else(|| format!("{key} is no XMP name (prefix:name, or {{namespace}}name)"))?;
            if v.is_null() {
                packet.remove(&name.ns, &name.local);
            } else {
                let value = xmp_value(&name, v)?;
                packet.set(name, value);
            }
        }
        meta.xmp = (!packet.properties.is_empty()).then(|| Arc::from(packet.packet().into_bytes()));
    }
    Ok(meta)
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
}
