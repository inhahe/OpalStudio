//! How a photograph was taken, as File Info shows it: read from EXIF (XMP's copies where EXIF
//! has none) and put as a photographer reads it.

use super::exif::{Exif, Ifd, Value};
use super::xmp::{self, Xmp};
use opal_core::document::Metadata;

/// What File Info shows of the capture: a label and its text, for each thing known, in the
/// order shown.
pub fn capture(meta: &Metadata) -> Vec<(&'static str, String)> {
    let e = meta.exif.as_deref().and_then(Exif::read);
    let x = meta.xmp.as_deref().and_then(Xmp::read).unwrap_or_default();
    let get = |ifd: Ifd, tag: u16| e.as_ref().and_then(|e| e.get(ifd, tag));
    let text = |ifd: Ifd, tag: u16| {
        get(ifd, tag).and_then(|v| v.text().map(|t| t.trim().to_owned())).filter(|t| !t.is_empty())
    };
    let number = |ifd: Ifd, tag: u16| get(ifd, tag).and_then(|v| v.number());
    let mut out = Vec::new();
    let mut add = |label, value: Option<String>| {
        if let Some(v) = value {
            out.push((label, v));
        }
    };
    add("Camera", camera(text(Ifd::Image, 0x010F), text(Ifd::Image, 0x0110)));
    let lens =
        text(Ifd::Exif, 0xA434).or_else(|| x.text(xmp::AUX, "Lens")).or_else(|| x.text(xmp::EXIF_EX, "LensModel"));
    add("Lens", lens);
    add("Shutter", number(Ifd::Exif, 0x829A).map(shutter));
    add("Aperture", number(Ifd::Exif, 0x829D).filter(|&f| f > 0.0).map(|f| format!("f/{}", decimal(f, 1))));
    add("ISO", number(Ifd::Exif, 0x8827).map(|i| format!("ISO {}", i.round())));
    add(
        "Exposure compensation",
        number(Ifd::Exif, 0x9204).map(|ev| {
            let ev = decimal(ev, 1);
            if ev.starts_with('-') || ev == "0" { format!("{ev} EV") } else { format!("+{ev} EV") }
        }),
    );
    add(
        "Focal length",
        number(Ifd::Exif, 0x920A).filter(|&f| f > 0.0).map(|f| {
            match number(Ifd::Exif, 0xA405).filter(|&f35| f35 > 0.0 && (f35 - f).abs() > 0.5) {
                Some(f35) => format!("{} mm ({} mm in 35 mm)", decimal(f, 1), f35.round()),
                None => format!("{} mm", decimal(f, 1)),
            }
        }),
    );
    add(
        "Flash",
        number(Ifd::Exif, 0x9209).map(|f| if f as u32 & 1 == 1 { "Fired" } else { "Did not fire" }.to_owned()),
    );
    add("Program", number(Ifd::Exif, 0x8822).and_then(|p| program(p as u32)).map(str::to_owned));
    add("Metering", number(Ifd::Exif, 0x9207).and_then(|m| metering(m as u32)).map(str::to_owned));
    add("White balance", number(Ifd::Exif, 0xA403).map(|w| if w == 0.0 { "Auto" } else { "Manual" }.to_owned()));
    let taken = text(Ifd::Exif, 0x9003).map(|d| {
        let sub = text(Ifd::Exif, 0x9291).map(|s| format!(".{s}")).unwrap_or_default();
        let zone = text(Ifd::Exif, 0x9011).map(|z| format!(" {z}")).unwrap_or_default();
        format!("{}{sub}{zone}", exif_date(&d))
    });
    let taken = taken.or_else(|| {
        x.text(xmp::EXIF, "DateTimeOriginal")
            .or_else(|| x.text(xmp::PHOTOSHOP, "DateCreated"))
            .map(|d| d.replace('T', " "))
    });
    add("Taken", taken);
    add("Place", place(e.as_ref()));
    add("Software", text(Ifd::Image, 0x0131).or_else(|| x.text(xmp::XMP, "CreatorTool")));
    out
}

/// The camera's name: its model, with its maker first unless the model names it already.
fn camera(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (Some(make), Some(model)) => {
            let first = make.split_whitespace().next().unwrap_or_default().to_lowercase();
            if model.to_lowercase().starts_with(&first) { Some(model) } else { Some(format!("{make} {model}")) }
        }
        (make, model) => model.or(make),
    }
}

/// An exposure time as cameras show it: 1/250 s, 0.4 s, 2 s.
fn shutter(t: f64) -> String {
    if t > 0.0 && t < 0.3 { format!("1/{} s", (1.0 / t).round()) } else { format!("{} s", decimal(t, 1)) }
}

/// `v` with at most `places` decimals, without trailing zeros.
fn decimal(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

/// An EXIF date ("2024:05:06 14:03:22") as one reads it ("2024-05-06 14:03:22").
fn exif_date(d: &str) -> String {
    match d.split_once(' ') {
        Some((date, time)) => format!("{} {time}", date.replace(':', "-")),
        None => d.replace(':', "-"),
    }
}

fn program(p: u32) -> Option<&'static str> {
    Some(match p {
        1 => "Manual",
        2 => "Program",
        3 => "Aperture priority",
        4 => "Shutter priority",
        5 => "Creative",
        6 => "Action",
        7 => "Portrait",
        8 => "Landscape",
        _ => return None,
    })
}

fn metering(m: u32) -> Option<&'static str> {
    Some(match m {
        1 => "Average",
        2 => "Center-weighted",
        3 => "Spot",
        4 => "Multi-spot",
        5 => "Pattern",
        6 => "Partial",
        _ => return None,
    })
}

/// The latitude and longitude GPS says, in degrees (south and west negative), and the
/// altitude in meters (below sea level negative), each if it says.
pub fn gps(e: &Exif) -> (Option<[f64; 2]>, Option<f64>) {
    let degrees = |v: Option<Value>| {
        let n = v?.numbers();
        Some(n.first()? + n.get(1).unwrap_or(&0.0) / 60.0 + n.get(2).unwrap_or(&0.0) / 3600.0)
    };
    let sign = |tag: u16, negative: &str| {
        if e.get(Ifd::Gps, tag)
            .and_then(|v| v.text().map(str::to_owned))
            .is_some_and(|r| r.eq_ignore_ascii_case(negative))
        {
            -1.0
        } else {
            1.0
        }
    };
    let lat = degrees(e.get(Ifd::Gps, 2)).map(|d| d * sign(1, "S"));
    let lon = degrees(e.get(Ifd::Gps, 4)).map(|d| d * sign(3, "W"));
    let altitude = e.get(Ifd::Gps, 6).and_then(|v| v.number()).map(|a| {
        let below = e.get(Ifd::Gps, 5).and_then(|v| v.number()) == Some(1.0);
        if below { -a } else { a }
    });
    (lat.zip(lon).map(|(a, b)| [a, b]), altitude)
}

/// Where it was taken, as one reads it: "48.858370° N, 2.294481° E, 35 m".
fn place(e: Option<&Exif>) -> Option<String> {
    let (at, altitude) = gps(e?);
    let [lat, lon] = at?;
    let mut s = format!(
        "{:.6}° {}, {:.6}° {}",
        lat.abs(),
        if lat < 0.0 { "S" } else { "N" },
        lon.abs(),
        if lon < 0.0 { "W" } else { "E" }
    );
    if let Some(a) = altitude {
        s.push_str(&format!(", {} m", decimal(a, 1)));
    }
    Some(s)
}
