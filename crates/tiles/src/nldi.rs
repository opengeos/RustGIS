//! The USGS Network-Linked Data Index (NLDI): a point's flowline (traced from where a raindrop
//! falling there would run), the basin draining to it, and navigation up- or downstream along
//! the NHDPlus network to the flowlines and the linked sites (stream gages, water quality
//! stations…) on the way. Requests are built and answers read here (sans I/O).
//!
//! The flowline comes from the NLDI's pygeoapi `nldi-flowtrace` process (a POST); when it fails,
//! the point's hydrolocation (the nearest flowline's COMID and where the point snaps to it) and
//! that COMID's flowline stand in.

use serde_json::{Value, json};

/// The NLDI's address.
pub const BASE: &str = "https://api.water.usgs.gov/nldi";
/// The longest navigation (km).
pub const MAX_DISTANCE: f64 = 9999.0;
/// The most features kept from an answer.
const MAX_FEATURES: usize = 200_000;

/// The navigation modes: the NLDI's code, its key in a COMID's navigation list, and a label.
pub const MODES: &[(&str, &str, &str)] = &[
    ("UM", "upstreamMain", "Upstream main stem"),
    ("UT", "upstreamTributaries", "Upstream with tributaries"),
    ("DM", "downstreamMain", "Downstream main stem"),
    ("DD", "downstreamDiversions", "Downstream with diversions"),
];

/// The flowtrace directions: the whole flowline, or its part up- or downstream of the point.
pub const DIRECTIONS: &[(&str, &str)] = &[("none", "Whole flowline"), ("up", "Upstream part"), ("down", "Downstream part")];

/// A navigation mode's code from its code or key (`UM`, `upstreamMain`, …).
pub fn mode(m: &str) -> Result<&'static str, String> {
    let m = m.trim();
    MODES
        .iter()
        .find(|(code, key, _)| code.eq_ignore_ascii_case(m) || key.eq_ignore_ascii_case(m))
        .map(|(code, _, _)| *code)
        .ok_or_else(|| format!("'{m}' isn't a navigation: one of UM (upstreamMain), UT (upstreamTributaries), DM (downstreamMain), DD (downstreamDiversions)"))
}

/// A navigation mode's label.
pub fn mode_label(code: &str) -> &'static str {
    MODES.iter().find(|(c, _, _)| *c == code).map(|(_, _, l)| *l).unwrap_or("Navigation")
}

/// A flowtrace direction, checked.
pub fn direction(d: &str) -> Result<&'static str, String> {
    let d = d.trim();
    DIRECTIONS
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(d))
        .map(|(k, _)| *k)
        .ok_or_else(|| format!("'{d}' isn't a direction: none, up or down"))
}

/// A point, checked: in the NLDI's coverage it is the conterminous United States, but anywhere
/// on the globe is asked.
pub fn point(lon: f64, lat: f64) -> Result<(f64, f64), String> {
    if !(lon.is_finite() && lat.is_finite()) || lon.abs() > 180.0 || lat.abs() > 90.0 {
        return Err("the point is a longitude (−180…180) and latitude (−90…90)".into());
    }
    Ok((lon, lat))
}

/// The flowtrace process's address.
pub fn flowtrace_url() -> String {
    format!("{BASE}/pygeoapi/processes/nldi-flowtrace/execution?f=json")
}

/// The flowtrace request's JSON body (the inputs as a list, which the process reads).
pub fn flowtrace_body(lon: f64, lat: f64, direction: &str) -> String {
    json!({"inputs": [
        {"id": "lat", "value": format!("{lat:.6}"), "type": "text/plain"},
        {"id": "lon", "value": format!("{lon:.6}"), "type": "text/plain"},
        {"id": "direction", "value": direction, "type": "text/plain"},
    ]})
    .to_string()
}

/// The point's hydrolocation: the nearest flowline's COMID and where the point snaps to it.
pub fn hydrolocation_url(lon: f64, lat: f64) -> String {
    format!("{BASE}/linked-data/hydrolocation?f=json&coords=POINT%28{lon:.6}%20{lat:.6}%29")
}

/// A COMID's flowline.
pub fn comid_url(comid: u64) -> String {
    format!("{BASE}/linked-data/comid/{comid}?f=json")
}

/// The basin draining to a COMID (simplified).
pub fn basin_url(comid: u64) -> String {
    format!("{BASE}/linked-data/comid/{comid}/basin?f=json&simplified=true")
}

/// The sources a navigation can list.
pub fn sources_url(comid: u64, mode: &str) -> String {
    format!("{BASE}/linked-data/comid/{comid}/navigation/{mode}?f=json")
}

/// A source's features along a navigation, up to `distance` km.
pub fn navigation_url(comid: u64, mode: &str, source: &str, distance: f64) -> Result<String, String> {
    let source = source.trim();
    if source.is_empty() || source.len() > 64 || !source.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(format!("'{source}' isn't a source (flowlines, nwissite, wqp…: nldi.sources lists them)"));
    }
    let d = clamp_distance(distance);
    Ok(format!("{BASE}/linked-data/comid/{comid}/navigation/{mode}/{}?f=json&distance={d}", source.to_ascii_lowercase()))
}

/// A navigation distance within 1–9999 km (500 when it isn't a number).
pub fn clamp_distance(d: f64) -> f64 {
    if d.is_finite() { d.clamp(1.0, MAX_DISTANCE).round() } else { 500.0 }
}

/// A fetch's error as the user reads it: a server failure (HTTP 5xx) says the NLDI is struggling.
pub fn explain(e: String) -> String {
    match crate::cache::http_status(&e) {
        Some(code) if code >= 500 => {
            format!(
                "the NLDI's server failed (HTTP {code}); it is often busy for a point it hasn't seen before: try again in a minute, or another point"
            )
        }
        _ => e,
    }
}

/// An answer that should be a GeoJSON FeatureCollection, with its features capped.
pub fn collection(bytes: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| error_text(bytes))?;
    let features = v.get("features").and_then(Value::as_array).ok_or_else(|| problem(&v))?;
    let kept: Vec<Value> =
        features.iter().filter(|f| f.get("geometry").is_some_and(Value::is_object)).take(MAX_FEATURES).cloned().collect();
    Ok(json!({"type": "FeatureCollection", "features": kept}))
}

/// What an answer that isn't JSON says (its start).
fn error_text(bytes: &[u8]) -> String {
    let t: String = String::from_utf8_lossy(bytes.get(..300).unwrap_or(bytes)).trim().chars().filter(|c| !c.is_control()).collect();
    if t.is_empty() { "the NLDI's answer is empty".into() } else { format!("the NLDI answered: {t}") }
}

/// What a JSON answer that isn't features says.
fn problem(v: &Value) -> String {
    let d = ["description", "detail", "message", "title"].iter().find_map(|k| v.get(*k).and_then(Value::as_str));
    match d {
        Some(d) => format!("the NLDI answered: {}", d.chars().take(300).collect::<String>()),
        None => "the NLDI's answer has no features".into(),
    }
}

/// A feature's COMID (`comid` or `COMID`, a number or digits).
fn comid_of(f: &Value) -> Option<u64> {
    let p = f.get("properties")?;
    ["comid", "COMID", "nhdplus_comid"].iter().find_map(|k| match p.get(*k)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

/// A flowtrace's answer: the flowline, the raindrop's path to it, and the flowline's COMID.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// The flowline's features (a FeatureCollection).
    pub flowline: Value,
    /// The raindrop's path (a FeatureCollection), when there is one.
    pub raindrop: Option<Value>,
    pub comid: Option<u64>,
}

/// Reads a flowtrace's answer: one FeatureCollection whose `raindropPath` feature is the path and
/// the others the flowline (the process's answer), or `flowline`/`raindropPath` members.
pub fn parse_flowtrace(bytes: &[u8]) -> Result<Trace, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| error_text(bytes))?;
    let is_raindrop = |f: &Value| f.get("id").and_then(Value::as_str).is_some_and(|id| id.to_ascii_lowercase().starts_with("raindrop"));
    let feats = |v: &Value| -> Vec<Value> {
        v.get("features")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter(|f| f.get("geometry").is_some_and(Value::is_object)).take(MAX_FEATURES).cloned().collect())
            .unwrap_or_default()
    };
    let (line, drop): (Vec<Value>, Vec<Value>) = if v.get("features").is_some() {
        feats(&v).into_iter().partition(|f| !is_raindrop(f))
    } else {
        let member = |keys: &[&str]| keys.iter().find_map(|k| v.get(*k)).map(feats).unwrap_or_default();
        (member(&["flowline", "flowLine"]), member(&["raindropPath", "raindrop_path", "raindrop"]))
    };
    if line.is_empty() {
        return Err(problem(&v));
    }
    let comid = line.iter().find_map(comid_of);
    let fc = |f: Vec<Value>| json!({"type": "FeatureCollection", "features": f});
    Ok(Trace { flowline: fc(line), raindrop: (!drop.is_empty()).then(|| fc(drop)), comid })
}

/// Reads a hydrolocation: the COMID, and where the point snapped to its flowline.
pub fn parse_hydrolocation(bytes: &[u8]) -> Result<(u64, Option<[f64; 2]>), String> {
    let fc = collection(bytes)?;
    let features = fc["features"].as_array().cloned().unwrap_or_default();
    let comid =
        features.iter().find_map(comid_of).ok_or("no flowline near the point (the NLDI covers the United States' NHDPlus network)")?;
    let snapped = features
        .iter()
        .find(|f| f.pointer("/properties/type").and_then(Value::as_str) == Some("hydrolocation"))
        .and_then(|f| f.pointer("/geometry/coordinates"))
        .and_then(Value::as_array)
        .and_then(|c| Some([c.first()?.as_f64()?, c.get(1)?.as_f64()?]))
        .filter(|[x, y]| x.is_finite() && y.is_finite());
    Ok((comid, snapped))
}

/// A source a navigation can list.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// Its id, lowercase (`flowlines`, `nwissite`…).
    pub id: String,
    pub name: String,
}

/// Reads the sources a navigation lists (`[{source, sourceName, features}]`), the flowlines
/// first.
pub fn parse_sources(bytes: &[u8]) -> Result<Vec<Source>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| error_text(bytes))?;
    let list = v.as_array().ok_or_else(|| problem(&v))?;
    let mut out: Vec<Source> = Vec::new();
    for s in list.iter().take(500) {
        let Some(id) = s.get("source").and_then(Value::as_str).map(str::trim).map(str::to_ascii_lowercase) else { continue };
        if id.is_empty()
            || id.len() > 64
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || out.iter().any(|o| o.id == id)
        {
            continue;
        }
        let name = s.get("sourceName").and_then(Value::as_str).map(|n| n.trim().chars().take(200).collect()).unwrap_or_else(|| id.clone());
        out.push(Source { id, name });
    }
    out.sort_by_key(|s| s.id != "flowlines");
    Ok(out)
}

/// The raindrop's path when the flowline came from the hydrolocation: a line from the point to
/// where it snapped.
pub fn snapped_path(lon: f64, lat: f64, snapped: [f64; 2]) -> Value {
    json!({"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {"kind": "to the nearest flowline"},
        "geometry": {"type": "LineString", "coordinates": [[lon, lat], snapped]}}]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests() {
        assert_eq!(flowtrace_url(), "https://api.water.usgs.gov/nldi/pygeoapi/processes/nldi-flowtrace/execution?f=json");
        let b: Value = serde_json::from_str(&flowtrace_body(-89.4, 43.07, "up")).unwrap();
        assert_eq!(b["inputs"][0], json!({"id": "lat", "value": "43.070000", "type": "text/plain"}));
        assert_eq!(b["inputs"][2]["value"], "up");
        assert_eq!(
            hydrolocation_url(-89.4, 43.07),
            "https://api.water.usgs.gov/nldi/linked-data/hydrolocation?f=json&coords=POINT%28-89.400000%2043.070000%29"
        );
        assert_eq!(basin_url(13294374), "https://api.water.usgs.gov/nldi/linked-data/comid/13294374/basin?f=json&simplified=true");
        assert_eq!(
            navigation_url(1, "UM", "NWISsite", 1e9).unwrap(),
            "https://api.water.usgs.gov/nldi/linked-data/comid/1/navigation/UM/nwissite?f=json&distance=9999"
        );
        assert!(navigation_url(1, "UM", "../x", 5.0).is_err());
        assert!(navigation_url(1, "UM", "", 5.0).is_err());
        assert_eq!(clamp_distance(f64::NAN), 500.0);
        assert!(explain("http status: 502".into()).contains("HTTP 502"));
        assert!(explain("HTTP 503".into()).contains("HTTP 503"));
        assert_eq!(explain("http status: 404".into()), "http status: 404");
        assert_eq!(clamp_distance(-3.0), 1.0);
        assert_eq!(mode("upstreamtributaries"), Ok("UT"));
        assert_eq!(mode("dd"), Ok("DD"));
        assert!(mode("sideways").is_err());
        assert_eq!(direction("UP"), Ok("up"));
        assert!(direction("left").is_err());
        assert!(point(200.0, 0.0).is_err() && point(0.0, f64::NAN).is_err() && point(-89.4, 43.0).is_ok());
    }

    #[test]
    fn flowtraces() {
        let process = br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","id":"nhdFlowline","geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]},"properties":{"comid":13294374}},
            {"type":"Feature","id":"raindropPath","geometry":{"type":"LineString","coordinates":[[0,0],[0,1]]},"properties":{}}]}"#;
        let t = parse_flowtrace(process).unwrap();
        assert_eq!(t.comid, Some(13294374));
        assert_eq!(t.flowline["features"].as_array().unwrap().len(), 1);
        assert_eq!(t.raindrop.unwrap()["features"].as_array().unwrap().len(), 1);
        // GeoLibre's shape, and a COMID's own flowline (its COMID a string).
        let members = br#"{"flowLine":{"features":[{"geometry":{"type":"LineString","coordinates":[]},"properties":{"COMID":"42"}}]},"raindrop_path":{"features":[]}}"#;
        let t = parse_flowtrace(members).unwrap();
        assert_eq!((t.comid, t.raindrop), (Some(42), None));
        let failed = br#"{"type":"InvalidParameterValue","description":"Error executing process: string indices"}"#;
        assert!(parse_flowtrace(failed).unwrap_err().contains("Error executing process"));
        assert!(parse_flowtrace(b"Method Not Allowed").unwrap_err().contains("Method Not Allowed"));
        for junk in [&b""[..], b"[]", b"7", b"{\"features\":7}", b"{\"features\":[{\"geometry\":7}]}", b"\xff"] {
            assert!(parse_flowtrace(junk).is_err());
            let _ = parse_hydrolocation(junk);
            let _ = parse_sources(junk);
            let _ = collection(junk);
        }
    }

    #[test]
    fn hydrolocations_and_sources() {
        let h = br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[-89.395,43.059]},"properties":{"comid":13294374,"type":"hydrolocation"}},
            {"type":"Feature","geometry":{"type":"Point","coordinates":[-89.4,43.07]},"properties":{"comid":"","type":"point"}}]}"#;
        assert_eq!(parse_hydrolocation(h).unwrap(), (13294374, Some([-89.395, 43.059])));
        assert!(parse_hydrolocation(br#"{"type":"FeatureCollection","features":[]}"#).unwrap_err().contains("no flowline"));
        let s = br#"[{"source":"WQP","sourceName":"Water Quality Portal","features":"https://x/wqp"},
            {"source":"Flowlines","sourceName":"NHDPlus flowlines"},{"source":"../evil"},{"source":"wqp"},{"sourceName":"no id"},7]"#;
        let s = parse_sources(s).unwrap();
        assert_eq!(s.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["flowlines", "wqp"]);
        assert_eq!(s[1].name, "Water Quality Portal");
        let fc = collection(br#"{"features":[{"geometry":{"type":"Point","coordinates":[0,0]}},{"geometry":null}]}"#).unwrap();
        assert_eq!(fc["features"].as_array().unwrap().len(), 1);
        assert_eq!(snapped_path(0.0, 0.0, [1.0, 1.0])["features"][0]["geometry"]["coordinates"], json!([[0.0, 0.0], [1.0, 1.0]]));
    }
}
