//! The local date and time, as metadata writes it.

/// Now, local: as EXIF writes a date ("2024:05:06 14:03:22", local time) and its zone
/// ("+02:00"), and as XMP does ("2024-05-06T14:03:22+02:00").
pub fn now() -> (String, String, String) {
    let (local, offset_minutes) = local_now();
    let [y, mo, d, h, mi, s] = local;
    let exif = format!("{y:04}:{mo:02}:{d:02} {h:02}:{mi:02}:{s:02}");
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let off = offset_minutes.unsigned_abs();
    let zone = format!("{sign}{:02}:{:02}", off / 60, off % 60);
    let xmp = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{zone}");
    (exif, zone, xmp)
}

/// The local time (year, month, day, hour, minute, second) and its offset from UTC in
/// minutes.
#[cfg(windows)]
fn local_now() -> ([i64; 6], i64) {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // SAFETY: both only fill in the structure they return.
    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let fields = |t: &windows::Win32::Foundation::SYSTEMTIME| {
        [t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond].map(i64::from)
    };
    let (l, u) = (fields(&local), fields(&utc));
    // The offset, to the nearest quarter hour (the two were read a moment apart).
    let diff = seconds(l) - seconds(u);
    let offset = ((diff as f64 / 900.0).round() as i64) * 15;
    (l, offset)
}

#[cfg(not(windows))]
fn local_now() -> ([i64; 6], i64) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    (civil(secs), 0)
}

/// Seconds since 1970 of a civil date and time (proleptic Gregorian, as UTC).
fn seconds([y, mo, d, h, mi, s]: [i64; 6]) -> i64 {
    // Howard Hinnant's days_from_civil.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + h * 3600 + mi * 60 + s
}

/// The civil date and time of `secs` since 1970 (UTC).
#[cfg_attr(windows, allow(dead_code))]
fn civil(secs: i64) -> [i64; 6] {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    [y, mo, d, rest / 3600, rest % 3600 / 60, rest % 60]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_and_seconds_agree() {
        for secs in [0, 951_782_400, 1_715_004_202, -86_400, 4_102_444_800] {
            assert_eq!(seconds(civil(secs)), secs);
        }
        assert_eq!(civil(1_715_004_202), [2024, 5, 6, 14, 3, 22]);
        // Leap day.
        assert_eq!(civil(951_782_400), [2000, 2, 29, 0, 0, 0]);
        let (exif, zone, xmp) = now();
        assert_eq!((zone.len(), &zone[3..4]), (6, ":"));
        assert_eq!((exif.len(), &exif[4..5], &exif[10..11]), (19, ":", " "));
        assert!(xmp.len() == 25 && &xmp[10..11] == "T", "{xmp}");
    }
}
