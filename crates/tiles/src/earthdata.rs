//! NASA Earthdata through Earthaccess's way of searching it: NASA's Common Metadata Repository
//! (CMR) for collections by keyword, then a collection's granules where the map is and between
//! two dates, their footprints and data links. Requests are built and answers read here (sans
//! I/O); the app fetches them off the frame, the command line waits for them.
//!
//! Searching needs no sign-in. Most files need an Earthdata Login token: [`set_token`] keeps one
//! in memory for the session (never in a project), and [`authorization`] gives it only to HTTPS
//! addresses on NASA's Earthdata data hosts (`*.earthdatacloud.nasa.gov`, `*.earthdata.nasa.gov`),
//! never to the login host (`urs.earthdata.nasa.gov`). The native fetcher asks for it at every
//! hop, so a redirect that leaves those hosts (to a presigned CloudFront or S3 address) goes
//! without it, and a redirect to the login host means the token was refused ([`is_login`]).

use std::sync::Mutex;

use serde_json::{Value, json};

/// NASA's Common Metadata Repository search API.
pub const CMR: &str = "https://cmr.earthdata.nasa.gov/search";
/// The login host: never sent the token; a redirect there means it was refused.
pub const LOGIN_HOST: &str = "urs.earthdata.nasa.gov";
/// The page where a token is made (Earthdata Login › Generate Token).
pub const TOKEN_PAGE: &str = "https://urs.earthdata.nasa.gov/documentation/for_users/user_token";
/// The most collections or granules a page asks for.
pub const MAX_PAGE: usize = 100;
/// The furthest page asked for (CMR pages by number up to 1 million hits).
pub const MAX_PAGE_NUM: usize = 1000;
/// The most granules read from an answer, and the most points a footprint keeps.
const MAX_ITEMS: usize = 2000;
const MAX_POINTS: usize = 10_000;
/// The longest token kept (Earthdata Login's JWTs are about 1 kB).
const MAX_TOKEN: usize = 16 << 10;

/// Popular datasets: CMR short name and title.
pub const PRESETS: &[(&str, &str)] = &[
    ("ATL03", "ICESat-2 ATL03 Geolocated Photons"),
    ("ATL06", "ICESat-2 ATL06 Land Ice Height"),
    ("ATL08", "ICESat-2 ATL08 Land and Vegetation Height"),
    ("GEDI02_A", "GEDI L2A Elevation and Height Metrics"),
    ("GEDI02_B", "GEDI L2B Canopy Cover and Vertical Profile"),
    ("GEDI_L4A_AGB_Density_V2_1_2056", "GEDI L4A Aboveground Biomass Density"),
    ("HLSL30", "HLS Landsat 30 m Surface Reflectance"),
    ("HLSS30", "HLS Sentinel-2 30 m Surface Reflectance"),
    ("NASADEM_HGT", "NASADEM Elevation"),
];

fn token_store() -> &'static Mutex<Option<String>> {
    static T: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// Keeps an Earthdata Login token for the session (`None` forgets it). A token is one word of
/// JWT characters; anything else is refused, so it can't break the header it goes in.
pub fn set_token(token: Option<&str>) -> Result<(), String> {
    let token = match token.map(str::trim).filter(|t| !t.is_empty()) {
        None => None,
        Some(t) => {
            let t = t.strip_prefix("Bearer ").map(str::trim).unwrap_or(t);
            if t.len() > MAX_TOKEN || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~+/=".contains(&b)) {
                return Err(
                    "an Earthdata Login token is one word of letters, digits and - _ . ~ + / = (Generate Token on Earthdata Login)".into(),
                );
            }
            Some(t.to_string())
        }
    };
    *token_store().lock().unwrap_or_else(|e| e.into_inner()) = token;
    Ok(())
}

/// Whether a token is kept.
pub fn has_token() -> bool {
    token_store().lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// When the token kept expires (Unix seconds), as its JWT `exp` claim says, if it says.
pub fn token_expiry() -> Option<i64> {
    token_store().lock().unwrap_or_else(|e| e.into_inner()).as_deref().and_then(jwt_expiry)
}

/// A JWT's `exp` claim: its middle part, base64url-decoded, is JSON.
fn jwt_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let v: Value = serde_json::from_slice(&base64url(payload)?).ok()?;
    v.get("exp")?.as_i64()
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().take(MAX_TOKEN) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// The host of an address the token may go to, lowercase: HTTPS, no user info or port.
fn https_host(url: &str) -> Option<String> {
    let rest = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).and_then(|_| url.get(8..))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains(['@', ':', '[', '\\']) {
        return None;
    }
    Some(authority.trim_end_matches('.').to_ascii_lowercase())
}

fn within(host: &str, suffix: &str) -> bool {
    host == suffix || host.strip_suffix(suffix).is_some_and(|h| h.ends_with('.'))
}

/// Earthdata's public services, which never need the token: the searches, Earthdata Search,
/// GIBS imagery tiles and the Earthdata GIS portal.
const PUBLIC_HOSTS: &[&str] = &["cmr.earthdata.nasa.gov", "search.earthdata.nasa.gov", "gibs.earthdata.nasa.gov", "gis.earthdata.nasa.gov"];

/// Whether `url` is on an Earthdata data host, the only place the token is sent: HTTPS on
/// `*.earthdatacloud.nasa.gov` or `*.earthdata.nasa.gov`, never a login host (`*urs.…`) or a public
/// service ([`PUBLIC_HOSTS`]).
pub fn is_data_host(url: &str) -> bool {
    https_host(url).is_some_and(|h| {
        !within(&h, LOGIN_HOST)
            && !PUBLIC_HOSTS.iter().any(|p| within(&h, p))
            && (within(&h, "earthdatacloud.nasa.gov") || within(&h, "earthdata.nasa.gov"))
    })
}

/// Whether `url` is Earthdata Login (where a data host sends a request it didn't accept).
pub fn is_login(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h).split(':').next().unwrap_or("");
    within(&host.trim_end_matches('.').to_ascii_lowercase(), LOGIN_HOST)
}

/// The `Authorization` header a request to `url` carries: the token kept, for a data host only.
pub fn authorization(url: &str) -> Option<String> {
    if !is_data_host(url) {
        return None;
    }
    token_store().lock().unwrap_or_else(|e| e.into_inner()).as_deref().map(|t| format!("Bearer {t}"))
}

/// The message for a request Earthdata Login turned away.
pub fn refused_message(url: &str) -> String {
    let file = url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("");
    if has_token() {
        format!(
            "{file}: Earthdata Login didn't accept the token (it may have expired, or the dataset needs its EULA accepted on Earthdata Login)"
        )
    } else {
        format!(
            "{file}: NASA asks for an Earthdata Login token for this file: give one in Web Data › NASA Earthaccess (or earthdata.login)"
        )
    }
}

/// A CMR collection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Collection {
    /// Its concept id (`C…-PROVIDER`).
    pub id: String,
    pub short_name: String,
    pub version: String,
    pub title: String,
    pub data_center: String,
    pub cloud_hosted: bool,
    pub time_start: String,
    pub time_end: String,
    pub summary: String,
}

/// A granule (a file or a set of files of a collection).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Granule {
    /// Its concept id (`G…-PROVIDER`).
    pub id: String,
    pub title: String,
    pub collection: String,
    pub time_start: String,
    pub time_end: String,
    /// Its size in MB, as CMR gives it.
    pub size_mb: Option<f64>,
    pub cloud_cover: Option<f64>,
    /// Its data files' addresses.
    pub data: Vec<String>,
    /// Its preview images' addresses.
    pub browse: Vec<String>,
    /// Its footprint, a GeoJSON geometry.
    pub geometry: Option<Value>,
}

impl Granule {
    /// Its GeoTIFF data files (added to the map as COGs).
    pub fn tiffs(&self) -> impl Iterator<Item = &String> {
        self.data.iter().filter(|u| is_tiff(u))
    }

    /// Its page on Earthdata Search.
    pub fn details_url(&self) -> String {
        format!("https://search.earthdata.nasa.gov/search/granules?p={}&g={}", encode(&self.collection), encode(&self.id))
    }
}

/// Whether an address is a GeoTIFF's.
pub fn is_tiff(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    path.ends_with(".tif") || path.ends_with(".tiff")
}

fn encode(s: &str) -> String {
    crate::s3::encode(s, true)
}

/// The text of a field, trimmed and capped.
fn text(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.trim().chars().take(4000).collect(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn number(v: &Value, k: &str) -> Option<f64> {
    match v.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

/// A collection search: by keyword (free text) or short name, optionally cloud-hosted only.
pub fn collections_url(keyword: &str, short_name: Option<&str>, cloud_hosted: bool, page_size: usize, page: usize) -> String {
    let mut q = vec![("has_granules", "true".to_string())];
    let keyword = keyword.trim();
    if !keyword.is_empty() {
        q.push(("keyword", keyword.chars().take(500).collect()));
    }
    if let Some(sn) = short_name.map(str::trim).filter(|s| !s.is_empty()) {
        q.push(("short_name", sn.chars().take(200).collect()));
    }
    if cloud_hosted {
        q.push(("cloud_hosted", "true".into()));
    }
    q.push(("page_size", page_size.clamp(1, MAX_PAGE).to_string()));
    q.push(("page_num", page.clamp(1, MAX_PAGE_NUM).to_string()));
    let query: Vec<String> = q.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
    format!("{CMR}/collections.json?{}", query.join("&"))
}

/// The entries of a CMR answer (`feed.entry`).
fn entries(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("CMR's answer isn't JSON: {e}"))?;
    if let Some(errors) = v.get("errors").and_then(Value::as_array) {
        let msg: Vec<String> = errors.iter().take(3).filter_map(Value::as_str).map(|s| s.chars().take(300).collect()).collect();
        return Err(format!("CMR: {}", msg.join("; ")));
    }
    let entry = v.get("feed").and_then(|f| f.get("entry")).ok_or("CMR's answer has no entries (feed.entry)")?;
    Ok(entry.as_array().map(|a| a.iter().take(MAX_ITEMS).cloned().collect()).unwrap_or_default())
}

/// The collections a CMR answer lists.
pub fn parse_collections(bytes: &[u8]) -> Result<Vec<Collection>, String> {
    Ok(entries(bytes)?
        .iter()
        .filter(|e| e.is_object())
        .map(|e| Collection {
            id: text(e, "id"),
            short_name: text(e, "short_name"),
            version: text(e, "version_id"),
            title: Some(text(e, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| text(e, "dataset_id")),
            data_center: text(e, "data_center"),
            cloud_hosted: e.get("cloud_hosted").and_then(Value::as_bool).unwrap_or(false),
            time_start: text(e, "time_start"),
            time_end: text(e, "time_end"),
            summary: text(e, "summary"),
        })
        .filter(|c| !c.id.is_empty())
        .collect())
}

/// The collection a short name means among several: cloud-hosted first, then the highest
/// version.
pub fn best<'a>(cols: &'a [Collection], short_name: &str) -> Option<&'a Collection> {
    let named: Vec<&Collection> = cols.iter().filter(|c| c.short_name.eq_ignore_ascii_case(short_name)).collect();
    let pool = if named.is_empty() { cols.iter().collect() } else { named };
    pool.into_iter().max_by(|a, b| {
        a.cloud_hosted
            .cmp(&b.cloud_hosted)
            .then_with(|| version_key(&a.version).partial_cmp(&version_key(&b.version)).unwrap_or(std::cmp::Ordering::Equal))
    })
}

/// A version's numbers, for ordering (`2.0` > `1.10` > `1.9`; `002` is 2).
fn version_key(v: &str) -> Vec<f64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).take(8).filter_map(|p| p.parse::<f64>().ok()).collect()
}

/// Whether a date is `YYYY-MM-DD`.
pub fn is_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
        && d.get(5..7).and_then(|m| m.parse::<u32>().ok()).is_some_and(|m| (1..=12).contains(&m))
        && d.get(8..10).and_then(|m| m.parse::<u32>().ok()).is_some_and(|m| (1..=31).contains(&m))
}

/// A granule search's area: within the globe, and the whole longitude range when it crosses the
/// antimeridian or isn't a box.
pub fn search_box(b: [f64; 4]) -> Result<[f64; 4], String> {
    let [w, s, e, n] = b;
    if !(w.is_finite() && s.is_finite() && e.is_finite() && n.is_finite()) {
        return Err("the area isn't four numbers".into());
    }
    let (s, n) = (s.clamp(-90.0, 90.0), n.clamp(-90.0, 90.0));
    if s >= n {
        return Err("the area's south is north of its north".into());
    }
    let (w, e) = if e - w >= 360.0 || w < -180.0 || e > 180.0 || w >= e { (-180.0, 180.0) } else { (w, e) };
    Ok([w, s, e, n])
}

/// A collection's granules: where (`area`) and when (`start`, `end`: `YYYY-MM-DD`, either open),
/// newest first.
pub fn granules_url(
    collection: &str,
    area: Option<[f64; 4]>,
    start: Option<&str>,
    end: Option<&str>,
    page_size: usize,
    page: usize,
) -> Result<String, String> {
    let collection = collection.trim();
    if collection.is_empty() || collection.len() > 100 || !collection.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(format!("'{collection}' isn't a collection's concept id (C…-PROVIDER, as earthdata.collections lists them)"));
    }
    let mut q = vec![format!("collection_concept_id={}", encode(collection))];
    if let Some(a) = area {
        let [w, s, e, n] = search_box(a)?;
        q.push(format!("bounding_box={}", encode(&format!("{w},{s},{e},{n}"))));
    }
    let start = start.map(str::trim).filter(|d| !d.is_empty());
    let end = end.map(str::trim).filter(|d| !d.is_empty());
    for d in [start, end].into_iter().flatten() {
        if !is_date(d) {
            return Err(format!("'{d}' isn't a date (YYYY-MM-DD)"));
        }
    }
    if let (Some(a), Some(b)) = (start, end)
        && a > b
    {
        return Err(format!("the start ({a}) is after the end ({b})"));
    }
    if start.is_some() || end.is_some() {
        let a = start.map(|d| format!("{d}T00:00:00Z")).unwrap_or_default();
        let b = end.map(|d| format!("{d}T23:59:59Z")).unwrap_or_default();
        q.push(format!("temporal={}", encode(&format!("{a},{b}"))));
    }
    q.push(format!("page_size={}", page_size.clamp(1, MAX_PAGE)));
    q.push(format!("page_num={}", page.clamp(1, MAX_PAGE_NUM)));
    q.push(format!("sort_key={}", encode("-start_date")));
    Ok(format!("{CMR}/granules.json?{}", q.join("&")))
}

/// Whether a link is to a file worth listing: a data or browse link of the granule's own (not
/// the collection's), on the web, and not metadata.
fn kept_link(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && ![".xml", ".cmr.json", ".md5", ".sha256", ".dmrpp"].iter().any(|x| path.ends_with(x))
        && !path.ends_with("/s3credentials")
}

/// The granules a CMR answer lists.
pub fn parse_granules(bytes: &[u8]) -> Result<Vec<Granule>, String> {
    Ok(entries(bytes)?
        .iter()
        .filter(|e| e.is_object())
        .map(|e| {
            let (mut data, mut browse) = (Vec::new(), Vec::new());
            for l in e.get("links").and_then(Value::as_array).into_iter().flatten().take(500) {
                if l.get("inherited").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let (rel, href) = (text(l, "rel"), text(l, "href"));
                if !kept_link(&href) {
                    continue;
                }
                let list = if rel.ends_with("/data#") {
                    &mut data
                } else if rel.ends_with("/browse#") {
                    &mut browse
                } else {
                    continue;
                };
                if !list.contains(&href) {
                    list.push(href);
                }
            }
            Granule {
                id: text(e, "id"),
                title: Some(text(e, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| text(e, "producer_granule_id")),
                collection: text(e, "collection_concept_id"),
                time_start: text(e, "time_start"),
                time_end: text(e, "time_end"),
                size_mb: number(e, "granule_size").filter(|s| *s >= 0.0),
                cloud_cover: number(e, "cloud_cover").filter(|c| (0.0..=100.0).contains(c)),
                data,
                browse,
                geometry: geometry(e),
            }
        })
        .filter(|g| !g.id.is_empty())
        .collect())
}

/// Numbers in CMR's text (`"lat lon lat lon …"`), as `[lon, lat]` pairs within the globe.
fn latlons(s: &str) -> Option<Vec<[f64; 2]>> {
    let n: Vec<f64> = s.split_whitespace().take(MAX_POINTS * 2).map(str::parse).collect::<Result<_, _>>().ok()?;
    let pts: Vec<[f64; 2]> = n.as_chunks::<2>().0.iter().map(|[lat, lon]| [*lon, *lat]).collect();
    pts.iter().all(|[x, y]| x.is_finite() && y.is_finite() && x.abs() <= 360.0 && y.abs() <= 90.0).then_some(pts)
}

/// A granule's footprint as GeoJSON: its polygons, else its boxes (`"s w n e"`), lines or points.
fn geometry(e: &Value) -> Option<Value> {
    let strings =
        |k: &str| -> Vec<Value> { e.get(k).and_then(Value::as_array).map(|a| a.iter().take(200).cloned().collect()).unwrap_or_default() };
    // Polygons: each a list of rings (the outer first), each ring a string.
    let polys: Vec<Value> = strings("polygons")
        .iter()
        .filter_map(|p| {
            let rings: Vec<Value> = match p {
                Value::Array(r) => {
                    r.iter().take(50).filter_map(Value::as_str).filter_map(latlons).filter(|r| r.len() >= 4).map(|r| json!(r)).collect()
                }
                Value::String(s) => latlons(s).filter(|r| r.len() >= 4).map(|r| vec![json!(r)]).unwrap_or_default(),
                _ => Vec::new(),
            };
            (!rings.is_empty()).then(|| json!(rings))
        })
        .collect();
    if !polys.is_empty() {
        return Some(json!({"type": "MultiPolygon", "coordinates": polys}));
    }
    let boxes: Vec<Value> = strings("boxes")
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|b| {
            let n: Vec<f64> = b.split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
            let [s, w, n, e] = <[f64; 4]>::try_from(n).ok()?;
            ([s, w, n, e].iter().all(|x| x.is_finite()) && s.abs() <= 90.0 && n.abs() <= 90.0 && w.abs() <= 180.0 && e.abs() <= 180.0)
                .then_some([s, w, n, e])
        })
        .flat_map(|[s, w, n, e]| {
            let ring = |w: f64, e: f64| json!([[[w, s], [e, s], [e, n], [w, n], [w, s]]]);
            // West past east crosses the antimeridian: a box either side of it.
            if w > e { vec![ring(w, 180.0), ring(-180.0, e)] } else { vec![ring(w, e)] }
        })
        .collect();
    if !boxes.is_empty() {
        return Some(json!({"type": "MultiPolygon", "coordinates": boxes}));
    }
    let lines: Vec<Value> =
        strings("lines").iter().filter_map(Value::as_str).filter_map(latlons).filter(|l| l.len() >= 2).map(|l| json!(l)).collect();
    if !lines.is_empty() {
        return Some(json!({"type": "MultiLineString", "coordinates": lines}));
    }
    let points: Vec<Value> = strings("points").iter().filter_map(Value::as_str).filter_map(latlons).flatten().map(|p| json!(p)).collect();
    (!points.is_empty()).then(|| json!({"type": "MultiPoint", "coordinates": points}))
}

/// Granules' footprints as a GeoJSON FeatureCollection, with their titles, times and sizes.
pub fn footprints_geojson(granules: &[Granule]) -> Value {
    let features: Vec<Value> = granules
        .iter()
        .filter_map(|g| {
            let geom = g.geometry.clone()?;
            Some(json!({"type": "Feature", "geometry": geom, "properties": {
                "id": g.id, "title": g.title, "start": g.time_start, "end": g.time_end,
                "size_mb": g.size_mb, "cloud_cover": g.cloud_cover,
                "files": g.data.len(), "first_file": g.data.first(),
            }}))
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

/// A collection as the commands list it.
pub fn collection_json(c: &Collection) -> Value {
    json!({"id": c.id, "shortName": c.short_name, "version": c.version, "title": c.title, "dataCenter": c.data_center,
        "cloudHosted": c.cloud_hosted, "start": c.time_start, "end": c.time_end,
        "summary": c.summary.chars().take(400).collect::<String>()})
}

/// A granule as the commands list it.
pub fn granule_json(g: &Granule) -> Value {
    json!({"id": g.id, "title": g.title, "collection": g.collection, "start": g.time_start, "end": g.time_end,
        "sizeMB": g.size_mb, "cloudCover": g.cloud_cover, "data": g.data, "browse": g.browse,
        "geotiffs": g.tiffs().collect::<Vec<_>>(), "details": g.details_url()})
}

/// `YYYY-MM-DD` of a Unix time.
pub fn date_of(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Today's date and the one `days` before it (`YYYY-MM-DD`).
pub fn recent(days: i64) -> (String, String) {
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    (date_of(now - days * 86_400), date_of(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_goes_to_earthdata_data_hosts_only() {
        for url in [
            "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/a.tif",
            "https://archive.podaac.earthdata.nasa.gov/x/y.tif",
            "HTTPS://Data.ORNLDAAC.Earthdata.NASA.gov/protected/a.tif",
            "https://earthdatacloud.nasa.gov/a",
        ] {
            assert!(is_data_host(url), "{url}");
        }
        for url in [
            "http://data.lpdaac.earthdatacloud.nasa.gov/a.tif",
            "https://urs.earthdata.nasa.gov/oauth/authorize",
            "https://URS.earthdata.nasa.gov./x",
            "https://uat.urs.earthdata.nasa.gov/oauth/authorize",
            "https://evilearthdata.nasa.gov/a",
            "https://earthdata.nasa.gov.evil.com/a",
            "https://user@data.lpdaac.earthdatacloud.nasa.gov/a",
            "https://data.lpdaac.earthdatacloud.nasa.gov:8443/a",
            "https://d1nklfio7vscoe.cloudfront.net/x.tif",
            "https://cmr.earthdata.nasa.gov/search/granules.json",
            "https://gibs.earthdata.nasa.gov/wmts/x.png",
            "https://search.earthdata.nasa.gov/search",
            "https://gis.earthdata.nasa.gov/portal",
            "https://lp-prod-protected.s3.us-west-2.amazonaws.com/a.tif",
            "s3://lp-prod-protected/a.tif",
            "https://",
            "",
            "https://[::1]/a",
        ] {
            assert!(!is_data_host(url), "{url}");
        }
        assert!(is_login("https://urs.earthdata.nasa.gov/oauth/authorize?client_id=x"));
        assert!(is_login("https://URS.Earthdata.nasa.gov:443/x"));
        assert!(is_login("https://uat.urs.earthdata.nasa.gov/x"));
        assert!(!is_login("https://data.lpdaac.earthdatacloud.nasa.gov/login"));
    }

    #[test]
    fn the_token_is_kept_checked_and_given_where_it_may_go() {
        // One test owns the store, so tests running at once don't race on it.
        assert!(set_token(Some("bad token with spaces")).is_err());
        assert!(set_token(Some("a\r\nX-Evil: 1")).is_err());
        // A JWT whose payload is {"exp":1800000000}.
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJleHAiOjE4MDAwMDAwMDB9.c2ln";
        set_token(Some(&format!("Bearer {jwt}"))).unwrap();
        assert!(has_token());
        assert_eq!(token_expiry(), Some(1_800_000_000));
        assert_eq!(authorization("https://data.lpdaac.earthdatacloud.nasa.gov/a.tif").as_deref(), Some(format!("Bearer {jwt}").as_str()));
        assert_eq!(authorization("https://urs.earthdata.nasa.gov/a"), None);
        assert_eq!(authorization("https://d1.cloudfront.net/a.tif"), None);
        assert!(refused_message("https://x.earthdata.nasa.gov/a/b.tif?x=1").starts_with("b.tif: Earthdata Login didn't accept"));
        set_token(None).unwrap();
        assert!(!has_token());
        assert_eq!(authorization("https://data.lpdaac.earthdatacloud.nasa.gov/a.tif"), None);
        assert!(refused_message("https://x/b.tif").contains("asks for an Earthdata Login token"));
        // Not a JWT: kept, with no expiry.
        set_token(Some("abc")).unwrap();
        assert_eq!(token_expiry(), None);
        set_token(Some("  ")).unwrap();
        assert!(!has_token());
    }

    #[test]
    fn collection_requests_and_answers() {
        let u = collections_url("sea ice & snow", None, true, 500, 0);
        assert!(u.starts_with("https://cmr.earthdata.nasa.gov/search/collections.json?"), "{u}");
        assert!(
            u.contains("keyword=sea%20ice%20%26%20snow")
                && u.contains("cloud_hosted=true")
                && u.contains("page_size=100")
                && u.contains("page_num=1"),
            "{u}"
        );
        let u = collections_url("", Some("HLSL30"), false, 20, 2);
        assert!(u.contains("short_name=HLSL30") && !u.contains("cloud_hosted"), "{u}");
        let body = br#"{"feed":{"entry":[
            {"id":"C1-LPDAAC","short_name":"HLSL30","version_id":"1.5","title":"Old","cloud_hosted":false},
            {"id":"C2-LPCLOUD","short_name":"HLSL30","version_id":"2.0","title":"New","cloud_hosted":true,"data_center":"LPCLOUD"},
            {"id":"C3-LPCLOUD","short_name":"HLSL30","version_id":"1.10","dataset_id":"Mid","cloud_hosted":true},
            {"short_name":"no id"}, 7, null]}}"#;
        let c = parse_collections(body).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[2].title, "Mid");
        assert_eq!(best(&c, "hlsl30").map(|c| c.id.as_str()), Some("C2-LPCLOUD"));
        assert_eq!(best(&[], "x"), None);
        assert!(parse_collections(br#"{"errors":["bad keyword"]}"#).unwrap_err().contains("bad keyword"));
        for junk in [&b""[..], b"[]", b"{}", b"{\"feed\":7}", b"\xff\xfe", b"{\"feed\":{\"entry\":\"x\"}}"] {
            let _ = parse_collections(junk);
            let _ = parse_granules(junk);
        }
    }

    #[test]
    fn granule_requests_check_their_parts() {
        let u =
            granules_url("C2021957657-LPCLOUD", Some([-90.0, 42.0, -89.0, 43.0]), Some("2024-06-01"), Some("2024-06-30"), 25, 1).unwrap();
        assert!(u.contains("collection_concept_id=C2021957657-LPCLOUD"), "{u}");
        assert!(u.contains("bounding_box=-90%2C42%2C-89%2C43"), "{u}");
        assert!(u.contains("temporal=2024-06-01T00%3A00%3A00Z%2C2024-06-30T23%3A59%3A59Z"), "{u}");
        assert!(u.contains("sort_key=-start_date"), "{u}");
        // Open-ended dates; an area across the antimeridian becomes every longitude.
        let u = granules_url("C1-X", Some([170.0, -10.0, 190.0, 10.0]), None, Some("2024-01-01"), 1000, 99_999).unwrap();
        assert!(
            u.contains("bounding_box=-180%2C-10%2C180%2C10")
                && u.contains("temporal=%2C2024-01-01T23")
                && u.contains("page_size=100")
                && u.contains("page_num=1000"),
            "{u}"
        );
        for (c, s, e) in [
            ("", None, None),
            ("C1&x=1", None, None),
            ("C1-X", Some("2024-13-01"), None),
            ("C1-X", Some("yesterday"), None),
            ("C1-X", Some("2024-06-02"), Some("2024-06-01")),
        ] {
            assert!(granules_url(c, None, s, e, 10, 1).is_err(), "{c} {s:?} {e:?}");
        }
        assert!(granules_url("C1-X", Some([f64::NAN, 0.0, 1.0, 1.0]), None, None, 10, 1).is_err());
        assert!(granules_url("C1-X", Some([0.0, 5.0, 1.0, 1.0]), None, None, 10, 1).is_err());
    }

    #[test]
    fn granules_keep_their_data_links_and_footprints() {
        let body = br#"{"feed":{"entry":[{
            "id":"G1-LPCLOUD","title":"HLS.L30.T15TYG","collection_concept_id":"C2-LPCLOUD",
            "time_start":"2024-06-30T16:34:23Z","granule_size":"12.5","cloud_cover":"22",
            "polygons":[["41.40 -89.29 42.39 -89.23 42.39 -89.27 41.41 -89.57 41.40 -89.29"]],
            "links":[
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"https://data.lpdaac.earthdatacloud.nasa.gov/p/a.B04.tif"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"https://data.lpdaac.earthdatacloud.nasa.gov/p/a.B04.tif"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/s3#","href":"s3://p/a.B04.tif"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"s3://p/a.B05.tif"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"https://x.nasa.gov/a.h5"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"https://x.nasa.gov/a.cmr.xml"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"https://x.nasa.gov/a.tif.sha256"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/metadata#","href":"https://x.nasa.gov/s3credentials"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/browse#","href":"https://x.nasa.gov/a.jpg"},
              {"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":"https://search.earthdata.nasa.gov/x","inherited":true}
            ]},
            {"id":"G2","boxes":["-10 170 10 -170"],"granule_size":-3,"cloud_cover":"500"},
            {"id":"G3","lines":["0 0 1 1"],"points":["5 5"]},
            {"id":"G4","points":["5 6", "x y", "95 0"]},
            {"id":"G5","polygons":[["1 2 3"]],"boxes":["1 2 3"]}
        ]}}"#;
        let g = parse_granules(body).unwrap();
        assert_eq!(g.len(), 5);
        assert_eq!(g[0].data, ["https://data.lpdaac.earthdatacloud.nasa.gov/p/a.B04.tif", "https://x.nasa.gov/a.h5"]);
        assert_eq!(g[0].browse, ["https://x.nasa.gov/a.jpg"]);
        assert_eq!(g[0].tiffs().count(), 1);
        assert_eq!((g[0].size_mb, g[0].cloud_cover), (Some(12.5), Some(22.0)));
        assert_eq!(g[0].details_url(), "https://search.earthdata.nasa.gov/search/granules?p=C2-LPCLOUD&g=G1-LPCLOUD");
        let geom = g[0].geometry.as_ref().unwrap();
        assert_eq!(geom["type"], "MultiPolygon");
        assert_eq!(geom["coordinates"][0][0][0], json!([-89.29, 41.40]));
        assert_eq!((g[1].size_mb, g[1].cloud_cover), (None, None));
        // Across the antimeridian: two boxes, 170–180 and −180 to −170.
        let across = &g[1].geometry.as_ref().unwrap()["coordinates"];
        assert_eq!(across.as_array().unwrap().len(), 2);
        assert_eq!(across[0][0], json!([[170.0, -10.0], [180.0, -10.0], [180.0, 10.0], [170.0, 10.0], [170.0, -10.0]]));
        assert_eq!(across[1][0], json!([[-180.0, -10.0], [-170.0, -10.0], [-170.0, 10.0], [-180.0, 10.0], [-180.0, -10.0]]));
        assert_eq!(g[2].geometry.as_ref().unwrap()["type"], "MultiLineString");
        // Only the points that read are kept.
        assert_eq!(g[3].geometry.as_ref().unwrap()["coordinates"], json!([[6.0, 5.0]]));
        assert_eq!(g[4].geometry, None);
        let fc = footprints_geojson(&g);
        assert_eq!(fc["features"].as_array().unwrap().len(), 4);
        assert_eq!(fc["features"][0]["properties"]["title"], "HLS.L30.T15TYG");
    }

    #[test]
    fn dates() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(1_718_000_000), "2024-06-10");
        assert_eq!(date_of(-86_400), "1969-12-31");
        let (a, b) = recent(30);
        assert!(is_date(&a) && is_date(&b) && a < b);
        assert!(!is_date("2024-6-01") && !is_date("2024-00-10") && !is_date("２０２４-01-01"));
    }
}
