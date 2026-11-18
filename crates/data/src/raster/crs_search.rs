//! Finding a coordinate system by name or code in terrio-crs's catalog, for the coordinate system
//! pickers: `swiss lv95`, `2056`, `EPSG:2056` and `epsg lv95` all find EPSG:2056.

use std::sync::OnceLock;

/// What a coordinate system found by [`search`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// In degrees of longitude and latitude (EPSG:4326).
    Geographic,
    /// On a map projection, in metres or feet (EPSG:2056).
    Projected,
}

/// A coordinate system of the catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// Its code as RustGIS reads it: `EPSG:2056`, `ESRI:102033`.
    pub code: String,
    /// Its name: `CH1903+ / LV95`.
    pub name: String,
    pub kind: Kind,
    /// The name of its area of use: `Liechtenstein; Switzerland.`
    pub area: Option<String>,
    /// Its projection's method, for a projected system: `Transverse Mercator`.
    pub method: Option<String>,
}

/// One catalog entry, with what a search compares in lower case.
struct Entry {
    found: Found,
    authority: &'static str,
    number: u32,
    /// The code's digits.
    digits: String,
    /// `epsg:2056`.
    key: String,
    /// The name in lower case.
    search: String,
    /// The area of use's name in lower case.
    area: String,
    /// The places the area of use names first, in lower case: `liechtenstein`, `switzerland` for
    /// `Liechtenstein; Switzerland.`, `usa` for `USA - Indiana - Dearborn, Ohio and Switzerland
    /// counties`.
    places: Vec<String>,
    /// The projection's method in lower case.
    method: String,
}

/// The current EPSG systems in degrees or projected, and the projected ESRI ones (the ESRI codes
/// RustGIS reads), read from the catalog once a run.
fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut out = Vec::new();
        for (authority, kinds) in [
            ("EPSG", vec![terrio_crs::CrsKind::Geographic2D, terrio_crs::CrsKind::Projected]),
            ("ESRI", vec![terrio_crs::CrsKind::Projected]),
        ] {
            let query = terrio_crs::CrsQuery { authorities: vec![authority.into()], kinds, ..Default::default() };
            for e in terrio_crs::list_crs(&query) {
                let kind = if e.kind == terrio_crs::CrsKind::Projected { Kind::Projected } else { Kind::Geographic };
                let code = format!("{}:{}", e.authority, e.code);
                out.push(Entry {
                    authority: e.authority,
                    number: e.code,
                    digits: e.code.to_string(),
                    key: code.to_ascii_lowercase(),
                    search: e.name.to_lowercase(),
                    area: e.area_name.as_deref().unwrap_or_default().to_lowercase(),
                    places: places(e.area_name.as_deref().unwrap_or_default()),
                    method: e.projection_method.as_deref().unwrap_or_default().to_lowercase(),
                    found: Found { code, name: e.name, kind, area: e.area_name, method: e.projection_method },
                });
            }
        }
        out
    })
}

/// The places an area of use's name lists first: each `;`-separated part up to its first ` - `.
fn places(area: &str) -> Vec<String> {
    area.split(';')
        .filter_map(|part| part.split(" - ").next())
        .map(|p| p.trim().trim_end_matches('.').trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

/// How well `e` matches the query, lower first; `None` when it doesn't. Every word must be in the
/// name, the projection's method or the area of use's name, begin the code or `authority:code`, or
/// be the authority.
fn rank(e: &Entry, words: &[String], whole: &str) -> Option<u8> {
    let mut code_word = false;
    let mut beside_name = false;
    for w in words {
        let in_code = e.digits.starts_with(w.as_str()) || e.key.starts_with(w.as_str());
        let in_name = in_code || e.search.contains(w.as_str()) || w.eq_ignore_ascii_case(e.authority);
        if !in_name && !e.method.contains(w.as_str()) && !e.area.contains(w.as_str()) {
            return None;
        }
        code_word |= in_code;
        beside_name |= !in_name;
    }
    Some(if whole == e.digits || whole == e.key {
        0
    } else if code_word {
        1
    } else if e.places.iter().any(|p| p == whole) {
        // A country typed: the systems for that country before those naming it otherwise.
        2
    } else if beside_name {
        5
    } else if e.search.starts_with(whole) {
        3
    } else {
        4
    })
}

/// The coordinate systems matching `text` (words of the name, the projection's method or the area
/// of use, a code or `EPSG:code`, in any case), best first: the code typed, codes beginning with it,
/// names beginning with the text, other names, then systems matched by their method or area of use; EPSG before ESRI,
/// and by code. At most `limit`, and only projected ones when
/// `projected_only` (a map is drawn in one). Nothing for blank text.
pub fn search(text: &str, projected_only: bool, limit: usize) -> Vec<Found> {
    let lower = text.trim().to_lowercase();
    // `EPSG: 2056` is one code.
    let whole = lower.replace(": ", ":");
    let words: Vec<String> = whole.split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, &Entry)> = entries()
        .iter()
        .filter(|e| !projected_only || e.found.kind == Kind::Projected)
        .filter_map(|e| rank(e, &words, &whole).map(|r| (r, e)))
        .collect();
    hits.sort_by_key(|(r, e)| (*r, e.authority != "EPSG", e.number));
    hits.into_iter().take(limit).map(|(_, e)| e.found.clone()).collect()
}

/// The catalog's name of `code` (`EPSG:2056`, `esri:102033`, `2056`), if it lists it.
pub fn name_of(code: &str) -> Option<&'static str> {
    let lower = code.trim().to_ascii_lowercase().replace(' ', "");
    let key = if lower.bytes().all(|b| b.is_ascii_digit()) { format!("epsg:{lower}") } else { lower };
    entries().iter().find(|e| e.key == key).map(|e| e.found.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_is_found_by_its_code_and_by_words_of_its_name() {
        for text in ["2056", "EPSG:2056", "epsg: 2056", "lv95", "CH1903+ LV95", "epsg lv95"] {
            let found = search(text, false, 50);
            assert!(found.iter().any(|f| f.code == "EPSG:2056" && f.name == "CH1903+ / LV95"), "{text}: {found:?}");
        }
        // The code typed comes first.
        assert_eq!(search("2056", false, 5)[0].code, "EPSG:2056");
        assert_eq!(search("esri:102033", false, 5)[0].code, "ESRI:102033");
    }

    #[test]
    fn a_map_is_offered_only_projected_systems() {
        assert_eq!(search("4326", false, 1)[0].kind, Kind::Geographic);
        assert!(search("wgs 84", true, 500).iter().all(|f| f.kind == Kind::Projected));
        assert!(!search("4326", true, 500).iter().any(|f| f.code == "EPSG:4326"));
    }

    #[test]
    fn blank_or_unknown_text_finds_nothing() {
        assert!(search("  ", false, 10).is_empty());
        assert!(search("no such system anywhere", false, 10).is_empty());
        // A country finds the systems used there, after those it names.
        let swiss = search("switzerland", true, 500);
        assert!(swiss.iter().any(|f| f.code == "EPSG:2056"), "{swiss:?}");
        // Switzerland's own before Switzerland County, Indiana (EPSG:7285).
        let at = |code: &str| swiss.iter().position(|f| f.code == code);
        assert!(at("EPSG:2056") < at("EPSG:7285"), "{swiss:?}");
        // So does a projection's method, with the area: Swiss Oblique Mercator.
        let oblique = search("oblique mercator switzerland", true, 50);
        assert!(oblique.iter().any(|f| f.code == "EPSG:2056"), "{oblique:?}");
        assert!(search("lambert azimuthal", true, 500).iter().any(|f| f.code == "EPSG:3035"));
        assert_eq!(search("utm", false, 7).len(), 7, "the limit holds");
    }

    #[test]
    fn a_code_is_named_from_the_catalog() {
        assert_eq!(name_of("EPSG:2056"), Some("CH1903+ / LV95"));
        assert_eq!(name_of("2056"), Some("CH1903+ / LV95"));
        assert_eq!(name_of("epsg:4326"), Some("WGS 84"));
        assert_eq!(name_of("vn2000tm3"), None);
    }
}
