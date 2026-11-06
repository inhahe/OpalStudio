//! Units of length for sizes and positions, and the resolution that relates them to pixels.
//!
//! A document's size is always kept in pixels; its resolution (pixels per inch) says how
//! large those pixels are when printed. Every other unit is converted through it.

/// The resolution a document has unless told otherwise, in pixels per inch: what most
/// programs assume for a file that does not say.
pub const DEFAULT_PPI: f64 = 72.0;
/// The lowest resolution accepted, in pixels per inch (a pixel 2.5 metres across).
pub const MIN_PPI: f64 = 0.01;
/// The highest resolution accepted, in pixels per inch (a pixel 25 nanometres across).
pub const MAX_PPI: f64 = 1_000_000.0;

/// Whether `ppi` is a resolution a document may have: finite and from [`MIN_PPI`] to
/// [`MAX_PPI`].
pub fn valid_ppi(ppi: f64) -> bool {
    (MIN_PPI..=MAX_PPI).contains(&ppi)
}

/// A unit of length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Px,
    In,
    Cm,
    Mm,
    Pt,
    Pc,
    Ft,
    Yd,
    M,
}

impl Unit {
    /// Every unit, in the order menus list them: pixels, then the typographic and imperial
    /// units by size, then the metric ones.
    pub const ALL: [Unit; 9] =
        [Unit::Px, Unit::Pt, Unit::Pc, Unit::In, Unit::Ft, Unit::Yd, Unit::Mm, Unit::Cm, Unit::M];

    /// The unit's short name, as written after a number and in scripts and settings.
    pub const fn id(self) -> &'static str {
        match self {
            Unit::Px => "px",
            Unit::In => "in",
            Unit::Cm => "cm",
            Unit::Mm => "mm",
            Unit::Pt => "pt",
            Unit::Pc => "pc",
            Unit::Ft => "ft",
            Unit::Yd => "yd",
            Unit::M => "m",
        }
    }

    /// The unit's name in menus.
    pub const fn name(self) -> &'static str {
        match self {
            Unit::Px => "Pixels",
            Unit::In => "Inches",
            Unit::Cm => "Centimeters",
            Unit::Mm => "Millimeters",
            Unit::Pt => "Points",
            Unit::Pc => "Picas",
            Unit::Ft => "Feet",
            Unit::Yd => "Yards",
            Unit::M => "Meters",
        }
    }

    /// The unit named `name`: its [`id`](Unit::id), or its name singular or plural (in
    /// either spelling of the metric ones), in any case.
    pub fn from_name(name: &str) -> Option<Unit> {
        let name = name.trim().to_ascii_lowercase();
        let singular = name.strip_suffix('s').filter(|s| !s.is_empty()).unwrap_or(&name);
        Unit::ALL.into_iter().find(|u| {
            let full = u.name().to_ascii_lowercase();
            let full = full.strip_suffix('s').unwrap_or(&full);
            let british = full.replace("meter", "metre");
            name == u.id()
                || singular == u.id()
                || singular == full
                || singular == british
                || (*u == Unit::In && singular == "inch")
                || (*u == Unit::Ft && singular == "foot")
        })
    }

    /// Inches in one of this unit, or `None` for pixels, whose size is the resolution's.
    fn inches(self) -> Option<f64> {
        match self {
            Unit::Px => None,
            Unit::In => Some(1.0),
            Unit::Cm => Some(1.0 / 2.54),
            Unit::Mm => Some(1.0 / 25.4),
            Unit::Pt => Some(1.0 / 72.0),
            Unit::Pc => Some(1.0 / 6.0),
            Unit::Ft => Some(12.0),
            Unit::Yd => Some(36.0),
            Unit::M => Some(1.0 / 0.0254),
        }
    }

    /// `value` of this unit in pixels, at `ppi` pixels per inch.
    pub fn to_px(self, value: f64, ppi: f64) -> f64 {
        self.inches().map_or(value, |i| value * i * ppi)
    }

    /// `px` pixels in this unit, at `ppi` pixels per inch.
    pub fn from_px(self, px: f64, ppi: f64) -> f64 {
        self.inches().map_or(px, |i| px / (i * ppi))
    }

    /// How many decimal places a length in this unit needs, at `ppi`, for a change of one
    /// pixel to change what is shown: none for pixels, and from none to six for the others.
    pub fn decimals(self, ppi: f64) -> usize {
        let pixel = self.from_px(1.0, ppi);
        // Converted lengths are not exact: a pixel of 0.1 must not ask for two places.
        (-pixel.log10() - 1e-9).ceil().clamp(0.0, 6.0) as usize
    }
}

impl std::fmt::Display for Unit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// Reads a length typed as a number with or without a unit after it ("12", "2.5 cm",
/// "3in", "1 inch"): the number, and the unit if one was given.
pub fn parse_length(text: &str) -> Option<(f64, Option<Unit>)> {
    let text = text.trim();
    let split = text
        .char_indices()
        .find(|&(i, c)| c.is_ascii_alphabetic() && !(matches!(c, 'e' | 'E') && is_exponent(text, i)))
        .map_or(text.len(), |(i, _)| i);
    let (number, unit) = text.split_at(split);
    let value: f64 = number.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
    if unit.trim().is_empty() {
        return Some((value, None));
    }
    Some((value, Some(Unit::from_name(unit)?)))
}

/// Whether the `e` at byte `i` of `text` is a number's exponent ("1e3"), not the start of
/// a unit: a digit before it, and a digit (after an optional sign) after it.
fn is_exponent(text: &str, i: usize) -> bool {
    let before = text[..i].chars().next_back().is_some_and(|c| c.is_ascii_digit() || c == '.');
    let rest = text[i + 1..].trim_start_matches(['+', '-']);
    before && rest.starts_with(|c: char| c.is_ascii_digit())
}

/// A unit of resolution: pixels per inch or per centimeter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolutionUnit {
    PerInch,
    PerCm,
}

impl ResolutionUnit {
    pub const ALL: [ResolutionUnit; 2] = [ResolutionUnit::PerInch, ResolutionUnit::PerCm];

    /// The unit's short name, as in scripts and settings.
    pub const fn id(self) -> &'static str {
        match self {
            ResolutionUnit::PerInch => "ppi",
            ResolutionUnit::PerCm => "ppcm",
        }
    }

    /// The unit's name in menus.
    pub const fn name(self) -> &'static str {
        match self {
            ResolutionUnit::PerInch => "Pixels/inch",
            ResolutionUnit::PerCm => "Pixels/cm",
        }
    }

    pub fn from_id(id: &str) -> Option<ResolutionUnit> {
        ResolutionUnit::ALL.into_iter().find(|u| u.id() == id)
    }

    /// `ppi` pixels per inch in this unit.
    pub fn from_ppi(self, ppi: f64) -> f64 {
        match self {
            ResolutionUnit::PerInch => ppi,
            ResolutionUnit::PerCm => ppi / 2.54,
        }
    }

    /// `value` of this unit in pixels per inch.
    pub fn to_ppi(self, value: f64) -> f64 {
        match self {
            ResolutionUnit::PerInch => value,
            ResolutionUnit::PerCm => value * 2.54,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_convert_both_ways() {
        let cases = [
            (Unit::Px, 300.0, 300.0),
            (Unit::In, 1.0, 300.0),
            (Unit::Cm, 2.54, 300.0),
            (Unit::Mm, 25.4, 300.0),
            (Unit::Pt, 72.0, 300.0),
            (Unit::Pc, 6.0, 300.0),
            (Unit::Ft, 1.0, 3600.0),
            (Unit::Yd, 1.0, 10_800.0),
            (Unit::M, 0.0254, 300.0),
        ];
        for (unit, value, px) in cases {
            assert!((unit.to_px(value, 300.0) - px).abs() < 1e-9, "{unit}");
            assert!((unit.from_px(px, 300.0) - value).abs() < 1e-12, "{unit}");
        }
        // Pixels ignore the resolution.
        assert_eq!(Unit::Px.to_px(5.0, 72.0), 5.0);
    }

    #[test]
    fn decimals_resolve_a_pixel() {
        assert_eq!(Unit::Px.decimals(300.0), 0);
        // A point is a pixel at 72 ppi, and a tenth of 0.24 px at 300.
        assert_eq!(Unit::Pt.decimals(72.0), 0);
        assert_eq!(Unit::Pt.decimals(300.0), 1);
        // A pixel at 300 ppi is 0.00333 in, at 72 ppi 0.0139 in.
        assert_eq!(Unit::In.decimals(300.0), 3);
        assert_eq!(Unit::In.decimals(72.0), 2);
        // A pixel at 254 ppi is exactly 0.1 mm.
        assert_eq!(Unit::Mm.decimals(254.0), 1);
        assert_eq!(Unit::M.decimals(MAX_PPI), 6);
    }

    #[test]
    fn lengths_are_read_with_or_without_a_unit() {
        assert_eq!(parse_length("12"), Some((12.0, None)));
        assert_eq!(parse_length(" 2.5 cm "), Some((2.5, Some(Unit::Cm))));
        assert_eq!(parse_length("3in"), Some((3.0, Some(Unit::In))));
        assert_eq!(parse_length("1 Inch"), Some((1.0, Some(Unit::In))));
        assert_eq!(parse_length("2 inches"), Some((2.0, Some(Unit::In))));
        assert_eq!(parse_length("4 feet"), Some((4.0, Some(Unit::Ft))));
        assert_eq!(parse_length("1 foot"), Some((1.0, Some(Unit::Ft))));
        assert_eq!(parse_length("10 millimetres"), Some((10.0, Some(Unit::Mm))));
        assert_eq!(parse_length("-3 px"), Some((-3.0, Some(Unit::Px))));
        assert_eq!(parse_length("1e2 pt"), Some((100.0, Some(Unit::Pt))));
        assert_eq!(parse_length("2 m"), Some((2.0, Some(Unit::M))));
        assert_eq!(parse_length("5 furlongs"), None);
        assert_eq!(parse_length("cm"), None);
        assert_eq!(parse_length("inf"), None);
        assert_eq!(parse_length(""), None);
    }

    #[test]
    fn every_unit_is_found_by_its_id_and_name() {
        for u in Unit::ALL {
            assert_eq!(Unit::from_name(u.id()), Some(u));
            assert_eq!(Unit::from_name(u.name()), Some(u));
        }
    }

    #[test]
    fn resolutions_are_bounded() {
        assert!(valid_ppi(DEFAULT_PPI) && valid_ppi(MIN_PPI) && valid_ppi(MAX_PPI));
        assert!(!valid_ppi(0.0) && !valid_ppi(f64::NAN) && !valid_ppi(f64::INFINITY) && !valid_ppi(2e6));
        assert!((ResolutionUnit::PerCm.from_ppi(254.0) - 100.0).abs() < 1e-12);
        assert_eq!(ResolutionUnit::PerCm.to_ppi(100.0), 254.0);
    }
}
