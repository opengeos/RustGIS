//! Web map and feature services: connect to a WMS, WMTS, WFS, OGC API Features or ArcGIS REST
//! service, list what it offers (from its capabilities), and add a layer — map layers as tile
//! layers, feature types and collections as data downloaded as GeoJSON.
//!
//! The app fetches the capabilities and features in the background and passes the capabilities
//! here as `"text"`; the command line, MCP and the desktop assistant let these commands fetch
//! (waiting for the answer).

use rustgis_data::ogc::{Offer, Service};
use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("service.layers", "Service Layers", "Home › Add", layers).view().params(
            r#"{"url": "a WMS, WMTS or WFS address (GetCapabilities is added), an OGC API /collections address (features, and map and vector tiles) or landing page (its map tiles), or an ArcGIS FeatureServer, MapServer, ImageServer or VectorTileServer", "type"?: "wms"|"wmts"|"wfs"|"ogcapi"|"arcgis" (guessed from the address), "text"?: "the capabilities document, when already fetched"}"#,
        ),
        C::new("service.add", "Add Service Layer", "Home › Add", add).params(
            r#"{"url": "as service.layers", "type"?, "layer": "the layer's name (service.layers lists them)", "name"?: text, "text"?: "the capabilities document, when already fetched", "tiles"?: "an OGC API layer's tile template, when already read from its tileset (the app's)"}"#,
        ),
    ]
}

fn url_of(p: &Value) -> Result<&str, String> {
    let url = ps(p, "url").map(str::trim).filter(|u| !u.is_empty()).ok_or("url is required")?;
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(format!("{url}: a service is given by its http:// or https:// address"));
    }
    Ok(url)
}

/// The service at `p`'s address: from its capabilities in `"text"`, or fetched (waiting for them).
fn capabilities(p: &Value) -> Result<Service, String> {
    let url = url_of(p)?;
    let caps = rustgis_data::ogc::capabilities_url(url, ps(p, "type"));
    let svc = match ps(p, "text") {
        Some(text) => rustgis_data::ogc::parse(&caps, text.as_bytes()).map_err(|e| format!("{url}: {e}"))?,
        None => fetch_capabilities(url, &caps)?,
    };
    Ok(vet(svc, &caps))
}

#[cfg(target_arch = "wasm32")]
fn fetch_capabilities(url: &str, _caps: &str) -> Result<Service, String> {
    Err(format!("{url}: in the web app, use Home › Add Data › From Service"))
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_capabilities(url: &str, caps: &str) -> Result<Service, String> {
    let bytes = crate::render::rustgis_tiles::cache::download_blocking(caps).map_err(|e| format!("{url}: {e}"))?;
    rustgis_data::ogc::parse(caps, &bytes).map_err(|e| format!("{url}: {e}"))
}

/// Leaves out the layers whose addresses a document on the web may not send RustGIS to: the
/// capabilities are hostile input, and their endpoints and links must not reach into the local
/// network (unless the service itself is there) or a forbidden address.
fn vet(mut svc: Service, caps: &str) -> Service {
    let before = svc.layers.len();
    svc.layers.retain(|l| {
        let u = match &l.offer {
            Offer::Tiles { template } | Offer::VectorTiles { template, .. } => template,
            Offer::Features { url } | Offer::Style { url } => url,
        };
        crate::render::rustgis_tiles::address::check(u, Some(caps)).is_ok()
    });
    let dropped = before - svc.layers.len();
    if dropped > 0 {
        svc.notes.push(format!("{dropped} layers point to addresses RustGIS won't fetch for a service on the web and are left out"));
    }
    svc
}

/// A service's GeoJSON answer made ready to read: latitude-first coordinates turned round, and a
/// cut-off answer pointed out.
pub fn features_answer(s: &mut Session, name: &str, bytes: Vec<u8>) -> Vec<u8> {
    if let Some(note) = rustgis_data::ogc::partial_note(&bytes) {
        s.notify(format!("{name}: {note}"), false);
    }
    rustgis_data::ogc::fix_axes(&bytes).unwrap_or(bytes)
}

/// A tile template given for a service's layer: a web address with `{z}`, `{x}` and `{y}`, that
/// the service's document may point to.
fn checked_template(service: &str, t: &str) -> Result<String, String> {
    let t = t.trim();
    if !(t.contains("{z}") && t.contains("{x}") && t.contains("{y}")) || t.len() > 2000 {
        return Err(format!("{t}: not a tile template"));
    }
    crate::render::rustgis_tiles::address::check(t, Some(service))?;
    Ok(t.to_string())
}

/// The tile template an OGC API tileset list advertises (its WebMercatorQuad tileset's), read now
/// (the list, then the tileset when the list doesn't carry it). `None` when it can't be read.
#[cfg(not(target_arch = "wasm32"))]
fn advertised_template(list: &str, vector: bool) -> Option<String> {
    use rustgis_data::ogc::{TilesStep, resolve_tiles};
    let mut doc = list.to_string();
    for _ in 0..2 {
        crate::render::rustgis_tiles::address::check(&doc, Some(list)).ok()?;
        let body = crate::render::rustgis_tiles::cache::download_blocking(&doc).ok()?;
        match resolve_tiles(list, &doc, &body, vector)? {
            TilesStep::Template(t) => return checked_template(list, &t).ok(),
            TilesStep::Next(next) => doc = next,
        }
    }
    None
}

/// In the browser nothing waits: the app reads the template itself and passes it as `"tiles"`.
#[cfg(target_arch = "wasm32")]
fn advertised_template(_: &str, _: bool) -> Option<String> {
    None
}

/// A service as the app and other callers see it.
pub fn service_json(svc: &Service) -> Value {
    let layers: Vec<Value> = svc
        .layers
        .iter()
        .map(|l| {
            let offer = match &l.offer {
                Offer::Tiles { template } => json!({"type": "tiles", "url": template}),
                Offer::Features { url } => json!({"type": "features", "url": url}),
                Offer::Style { url } => json!({"type": "style", "url": url}),
                Offer::VectorTiles { template, layers } => json!({"type": "vectorTiles", "url": template, "layers": layers}),
            };
            json!({"name": l.name, "title": l.title, "bbox": l.bbox, "offer": offer, "tilesets": l.tilesets})
        })
        .collect();
    json!({"kind": svc.kind, "title": svc.title, "count": layers.len(), "layers": layers, "notes": svc.notes})
}

fn layers(_s: &mut Session, p: &Value) -> CmdResult {
    Ok(service_json(&capabilities(p)?))
}

/// Adds one of a service's layers: map layers as tiles (an overlay), a style as a style, features
/// downloaded.
fn add(s: &mut Session, p: &Value) -> CmdResult {
    let url = url_of(p)?.to_string();
    let want = ps(p, "layer").map(str::trim).filter(|l| !l.is_empty()).ok_or("layer is required")?;
    let svc = capabilities(p)?;
    let layer = svc
        .layers
        .iter()
        .find(|l| l.name == want)
        .or_else(|| svc.layers.iter().find(|l| l.name.eq_ignore_ascii_case(want) || l.title.eq_ignore_ascii_case(want)))
        .ok_or_else(|| format!("{url}: no layer '{want}' (service.layers lists them)"))?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&layer.title).to_string();
    let zoom = |s: &mut Session| {
        if let Some([w, so, e, n]) = layer.bbox {
            s.zoom_to(&rustgis_geom::Extent::new(w, so, e, n));
        }
    };
    // An OGC API layer's tiles: the template its tileset advertises (given, or read now), else
    // the usual path.
    let advertised = match (&layer.tilesets, ps(p, "tiles")) {
        (Some(_), Some(t)) => Some(checked_template(&url, t)?),
        (Some(list), None) => advertised_template(list, matches!(layer.offer, Offer::VectorTiles { .. })),
        _ => None,
    };
    match &layer.offer {
        Offer::Tiles { template } => {
            let template = advertised.as_deref().unwrap_or(template);
            let r = super::tiles::add(s, &json!({"url": template, "kind": "raster", "name": name, "overlay": true}))?;
            zoom(s);
            Ok(r)
        }
        Offer::VectorTiles { template, layers } => {
            let template = advertised.as_deref().unwrap_or(template);
            let r = super::tiles::add(s, &json!({"url": template, "kind": "vector", "name": name, "layers": layers, "overlay": true}))?;
            zoom(s);
            Ok(r)
        }
        Offer::Style { url: style } => {
            let r = super::tiles::add(s, &json!({"url": style, "kind": "style", "name": name}))?;
            zoom(s);
            Ok(r)
        }
        Offer::Features { url: items } => {
            #[cfg(target_arch = "wasm32")]
            {
                let _ = items;
                Err(format!("{url}: in the web app, Home › Add Data › From Service downloads the features"))
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // `download_blocking` is bounded: MAX_DOWNLOAD bytes and BLOCKING_TIMEOUT.
                let bytes = crate::render::rustgis_tiles::cache::download_blocking(items).map_err(|e| format!("{items}: {e}"))?;
                let bytes = features_answer(s, &name, bytes);
                let r = super::raster::add_named_bytes(s, &json!({"name": name}), format!("{name}.geojson"), bytes)?;
                zoom(s);
                Ok(r)
            }
        }
    }
}
