//! Display units for live map measurements and the scale bar. Geometry stays in SI units.
use rustgis_engine::geom;

pub fn distance(m: f64, imperial: bool) -> String {
    if !imperial {
        return geom::format_distance(m);
    }
    let ft = m / geom::METRES_PER_FOOT;
    if ft.round() < 5280.0 {
        crate::i18n::fmt("{distance} ft", &[("distance", &format!("{ft:.0}"))])
    } else {
        crate::i18n::fmt("{distance} mi", &[("distance", &format!("{:.1}", m / geom::METRES_PER_MILE))])
    }
}

pub fn area(m2: f64, imperial: bool) -> String {
    if !imperial {
        return geom::format_area(m2);
    }
    if m2 < geom::SQUARE_METRES_PER_ACRE {
        crate::i18n::fmt("{area} ft²", &[("area", &format!("{:.0}", m2 / geom::SQUARE_METRES_PER_SQUARE_FOOT))])
    } else if m2 < geom::SQUARE_METRES_PER_SQUARE_MILE {
        crate::i18n::fmt("{area} acres", &[("area", &format!("{:.1}", m2 / geom::SQUARE_METRES_PER_ACRE))])
    } else {
        crate::i18n::fmt("{area} mi²", &[("area", &format!("{:.1}", m2 / geom::SQUARE_METRES_PER_SQUARE_MILE))])
    }
}

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
