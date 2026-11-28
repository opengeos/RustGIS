//! Display units for measurements and distance controls. Inputs stay in metres or square metres.
use rustgis_engine::geom;

/// Keep nonzero measurements visible when rounding to whole units would erase them.
fn whole_or_small(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude > 0.0 && magnitude <= 0.5 {
        if magnitude < 0.0001 {
            format!("{value:.1e}")
        } else {
            // Two significant figures, with at most five decimal places in this range.
            let precision = (1.0 - magnitude.log10().floor()) as usize;
            format!("{value:.precision$}")
        }
    } else {
        format!("{value:.0}")
    }
}

/// Format a distance supplied in metres, using feet/miles or the metric formatter.
/// Sub-foot values whose magnitude would round to zero retain significant digits.
pub fn distance(m: f64, imperial: bool) -> String {
    if !imperial || !m.is_finite() {
        return geom::format_distance(m);
    }
    let ft = m / geom::METRES_PER_FOOT;
    if ft.abs().round() < 5280.0 {
        crate::i18n::fmt("{distance} ft", &[("distance", &whole_or_small(ft))])
    } else {
        crate::i18n::fmt("{distance} mi", &[("distance", &format!("{:.1}", m / geom::METRES_PER_MILE))])
    }
}

/// Parse an optional positive distance. Bare numbers use metres or feet; suffixes let the
/// user enter kilometres or miles too. Command distances remain metres in both settings.
pub fn parse_distance(text: &str, imperial: bool) -> Result<Option<f64>, String> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Ok(None);
    }
    let (number, factor) = [("km", 1000.0), ("mi", geom::METRES_PER_MILE), ("ft", geom::METRES_PER_FOOT), ("m", 1.0)]
        .into_iter()
        .find_map(|(suffix, factor)| text.strip_suffix(suffix).map(|n| (n.trim(), factor)))
        .unwrap_or((&text, if imperial { geom::METRES_PER_FOOT } else { 1.0 }));
    let value = number
        .parse::<f64>()
        .ok()
        .map(|n| n * factor)
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or_else(|| tl!("Enter a positive distance in m, km, ft or mi.").to_string())?;
    Ok(Some(value))
}

/// Format an elevation supplied in metres; imperial vertical measurements always use feet.
pub fn elevation(m: f64, imperial: bool) -> String {
    if imperial {
        crate::i18n::fmt("{distance} ft", &[("distance", &whole_or_small(m / geom::METRES_PER_FOOT))])
    } else {
        crate::i18n::fmt("{distance} m", &[("distance", &rustgis_engine::map::symbology::fmt_num(m))])
    }
}

/// Format square metres as square feet, acres or square miles, or use metric units.
/// Sub-square-foot values whose magnitude would round to zero retain significant digits.
pub fn area(m2: f64, imperial: bool) -> String {
    if !imperial || !m2.is_finite() {
        return geom::format_area(m2);
    }
    if m2.abs() < geom::SQUARE_METRES_PER_ACRE {
        crate::i18n::fmt("{area} ft²", &[("area", &whole_or_small(m2 / geom::SQUARE_METRES_PER_SQUARE_FOOT))])
    } else if m2.abs() < geom::SQUARE_METRES_PER_SQUARE_MILE {
        crate::i18n::fmt("{area} acres", &[("area", &format!("{:.1}", m2 / geom::SQUARE_METRES_PER_ACRE))])
    } else {
        crate::i18n::fmt("{area} mi²", &[("area", &format!("{:.1}", m2 / geom::SQUARE_METRES_PER_SQUARE_MILE))])
    }
}

/// Choose a readable scale-bar length in the display units and return its length in metres.
pub fn scale_distance(max_m: f64, imperial: bool) -> f64 {
    let unit = if !imperial {
        1.0
    } else if max_m < geom::METRES_PER_MILE {
        geom::METRES_PER_FOOT
    } else {
        geom::METRES_PER_MILE
    };
    geom::nice_distance(max_m / unit) * unit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_entry_accepts_units_and_rejects_invalid_values() {
        assert_eq!(parse_distance("", false).unwrap(), None);
        assert_eq!(parse_distance("500", false).unwrap(), Some(500.0));
        assert_eq!(parse_distance("2 km", false).unwrap(), Some(2000.0));
        assert_eq!(parse_distance(" 1.25 MI ", true).unwrap(), Some(1.25 * geom::METRES_PER_MILE));
        assert_eq!(parse_distance("3", true).unwrap(), Some(3.0 * geom::METRES_PER_FOOT));
        assert_eq!(parse_distance("500 m", true).unwrap(), Some(500.0));
        for invalid in ["0", "-1", "NaN", "inf", "1e999", "3 yards"] {
            assert!(parse_distance(invalid, false).is_err(), "{invalid}");
        }
    }

    #[test]
    fn nonfinite_measurements_are_unavailable_in_both_unit_systems() {
        for imperial in [false, true] {
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(distance(value, imperial), "—");
                assert_eq!(area(value, imperial), "—");
            }
        }
        assert_eq!(distance(0.0, true), "0 ft");
        assert_eq!(area(0.0, true), "0 ft²");
        assert_eq!(distance(geom::METRES_PER_FOOT, true), "1 ft");
        assert_eq!(distance(-0.1 * geom::METRES_PER_FOOT, true), "-0.10 ft");
        assert_eq!(area(geom::SQUARE_METRES_PER_SQUARE_FOOT, true), "1 ft²");
        assert_eq!(area(-0.1 * geom::SQUARE_METRES_PER_SQUARE_FOOT, true), "-0.10 ft²");
        assert_eq!(distance(-geom::METRES_PER_FOOT, true), "-1 ft");
        assert_eq!(distance(-5280.0 * geom::METRES_PER_FOOT, true), "-1.0 mi");
        assert_eq!(distance(-20_000.0, true), "-12.4 mi");
        assert_eq!(area(-geom::SQUARE_METRES_PER_SQUARE_FOOT, true), "-1 ft²");
        assert_eq!(area(-geom::SQUARE_METRES_PER_ACRE, true), "-1.0 acres");
        assert_eq!(area(-10.0 * geom::SQUARE_METRES_PER_ACRE, true), "-10.0 acres");
        assert_eq!(area(-geom::SQUARE_METRES_PER_SQUARE_MILE, true), "-1.0 mi²");
        assert_eq!(area(-10_000_000.0, true), "-3.9 mi²");
    }
}
