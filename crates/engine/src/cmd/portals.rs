//! Open-data portals: OpenStreetMap data for an area (through Overpass) and administrative
//! boundaries for a country (from geoBoundaries).

use serde_json::{Value, json};

use super::{b, ps};
use crate::render::rustgis_tiles::{boundaries, overpass};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("osm.download", "Download OpenStreetMap Data", "Home › Add › Add Data", osm_download).params(
            r#"{"bbox"?: [west, south, east, north] | "view" (the map's extent, the default), "view"?: bool (false: a bbox must be given), "preset"?: "buildings"|"roads"|"railways"|"water"|"landuse"|"parks"|"pois"|"boundaries", "tags"?: "key=value, key" (any of them), "url"?: "an Overpass server (default https://overpass-api.de/api/interpreter)", "name"?: text, "answer"?: "the server's JSON answer, when already fetched (the app's background request)"}"#,
        ),
        C::new("boundaries.add", "Add Administrative Boundaries", "Home › Add › Add Data", boundaries_add).params(
            r#"{"country": "ISO 3166 alpha-3 code (CHE, KEN…)", "level"?: "ADM0" (the country, default) … "ADM5", "name"?: text, "text"?: "the boundaries' GeoJSON, when already fetched (the app's background request)"}"#,
        ),
        C::new("boundaries.countries", "List geoBoundaries Countries", "Home › Add › Add Data", boundaries_countries).view(),
    ]
}

/// The box a command names, the map's extent by default.
fn bbox(s: &Session, p: &Value) -> Result<[f64; 4], String> {
    match p.get("bbox") {
        Some(v @ Value::Array(_)) => {
            // Every entry a number: one that isn't is an error, not dropped.
            let n: Option<Vec<f64>> = v.as_array().into_iter().flatten().map(Value::as_f64).collect();
            match n.as_deref().unwrap_or_default() {
                [w, south, e, n] => Ok([*w, *south, *e, *n]),
                _ => Err("bbox is [west, south, east, north], or \"view\"".into()),
            }
        }
        Some(Value::String(v)) if v != "view" => Err("bbox is [west, south, east, north], or \"view\"".into()),
        _ if b(p, "view") == Some(false) => Err("give bbox".into()),
        _ => {
            let e = super::workspace::view_extent(s);
            if e.is_empty() {
                return Err("the map's extent isn't known; give bbox".into());
            }
            Ok([e.min_x.max(-180.0), e.min_y.max(-90.0), e.max_x.min(180.0), e.max_y.min(90.0)])
        }
    }
}

/// The Overpass query a command describes (the app builds its request from it too).
pub fn osm_query(s: &Session, p: &Value) -> Result<overpass::Query, String> {
    overpass::Query::new(bbox(s, p)?, ps(p, "preset"), ps(p, "tags"))
}

fn osm_download(s: &mut Session, p: &Value) -> CmdResult {
    let q = osm_query(s, p)?;
    let url = q.url(ps(p, "url"))?;
    let bytes = match ps(p, "answer") {
        Some(a) => a.as_bytes().to_vec(),
        None => fetch(&url)?,
    };
    let answer = overpass::parse(&bytes)?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| q.title());
    let mut layers = Vec::new();
    let mut counts = serde_json::Map::new();
    for (kind, fc) in answer.layers() {
        let (ds, _) = rustgis_data::io::read_geojson(&fc.to_string(), &format!("{name} ({kind})")).map_err(|e| e.to_string())?;
        counts.insert(kind.to_string(), json!(ds.features.len()));
        layers.push(super::layer::add_dataset(s, ds));
    }
    for n in answer.notes {
        s.notify(format!("{name}: {n}"), false);
    }
    if layers.is_empty() {
        return Err(format!("{name}: nothing found in the area"));
    }
    Ok(json!({"layers": layers, "features": counts}))
}

fn boundaries_add(s: &mut Session, p: &Value) -> CmdResult {
    let country = boundaries::country(ps(p, "country").ok_or("country is required (such as CHE)")?)?;
    let level = match p.get("level") {
        Some(Value::Number(n)) => boundaries::level(&n.to_string())?,
        _ => boundaries::level(ps(p, "level").unwrap_or("ADM0"))?,
    };
    let (text, title) = match ps(p, "text") {
        Some(t) => (t.to_string(), None),
        None => {
            let meta = boundaries::parse_meta(&fetch(&boundaries::meta_url(&country, &level))?)?;
            let bytes = fetch(&meta.geojson)?;
            (String::from_utf8(bytes).map_err(|_| "the boundaries aren't UTF-8 text".to_string())?, Some(meta.name))
        }
    };
    let name = ps(p, "name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} {level}", title.unwrap_or_else(|| country.clone())));
    let (ds, _) = rustgis_data::io::read_geojson(&text, &name).map_err(|e| format!("{name}: {e}"))?;
    let n = ds.features.len();
    let id = super::layer::add_dataset(s, ds);
    if let Some(e) = s.map().layer(id).map(|l| l.extent()) {
        s.zoom_to(&e);
    }
    Ok(json!({"layer": id, "features": n}))
}

fn boundaries_countries(_s: &mut Session, _p: &Value) -> CmdResult {
    let list = boundaries::parse_countries(&fetch(&boundaries::countries_url())?)?;
    Ok(json!({"count": list.len(), "countries": list.iter().map(|(iso, name)| json!({"iso": iso, "name": name})).collect::<Vec<_>>()}))
}

/// A fetch the command waits for (the command line and the assistant; the app fetches off the
/// frame and passes the answer).
#[cfg(not(target_arch = "wasm32"))]
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    crate::render::rustgis_tiles::cache::download_blocking(url).map_err(|e| {
        if e.contains("timeout") || e.contains("timed out") {
            "the server didn't answer within a minute (it may be busy): try again later, or a smaller area".to_string()
        } else if e.contains("504") || e.contains("429") {
            format!("{e}: the server is busy, try again in a minute")
        } else {
            e
        }
    })
}

#[cfg(target_arch = "wasm32")]
fn fetch(_url: &str) -> Result<Vec<u8>, String> {
    Err("in the web app, use Home › Add Data › OpenStreetMap… or Boundaries…".into())
}
