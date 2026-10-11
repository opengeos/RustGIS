//! The plugin registry: a curated list of plugins (`registry.json`) and a list of pulled ones
//! (`blocklist.json`), published at <https://plugins.rustgis.app>.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::{Permission, valid_id, version_parts};

/// The hosted registry.
pub const DEFAULT_URL: &str = "https://plugins.rustgis.app/registry.json";

/// The most entries read from a registry.
const MAX_ENTRIES: usize = 5_000;

/// One plugin in the registry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub categories: Vec<String>,
    /// The `.rgplugin` package: an HTTPS URL, or a path relative to the registry.
    pub package: String,
    /// Lower-case hex SHA-256 of the package: the download must match it.
    pub sha256: String,
    /// The package's size in bytes.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub min_rustgis_version: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

/// A pulled plugin: every version (no `sha256`), or the one package with that hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Blocked {
    pub id: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub reason: String,
}

/// Whether `s` is a lower-case hex SHA-256.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `rel` resolved against the URL `base` (a file in a folder): absolute URLs are kept, a path
/// replaces the last segment of `base`.
pub fn resolve(base: &str, rel: &str) -> String {
    if rel.contains("://") {
        return rel.to_string();
    }
    let Some(host) = base.find("://").map(|i| i + 3) else { return rel.to_string() };
    // The origin (`https://host`) and the folder (`https://host/a/`).
    let origin = base.get(host..).and_then(|r| r.find('/')).and_then(|i| base.get(..host + i)).unwrap_or(base);
    if let Some(abs) = rel.strip_prefix('/') {
        return format!("{origin}/{abs}");
    }
    let dir = match base.rfind('/') {
        Some(i) if i >= host => base.get(..=i).unwrap_or(base).to_string(),
        _ => format!("{base}/"),
    };
    format!("{dir}{}", rel.trim_start_matches("./"))
}

/// Whether a package URL may be downloaded: HTTPS, or plain HTTP to this computer (for testing
/// a local registry).
pub fn allowed_url(url: &str) -> bool {
    url.starts_with("https://") || is_local(url)
}

/// Whether `url` is plain HTTP to this computer (`http://localhost`, `http://127.0.0.1`, with a
/// port or path), and not a host that merely starts like one (`localhost.example.com`) or hides
/// another host after user info (`localhost:80@example.com`).
fn is_local(url: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|p| {
        url.strip_prefix(p).is_some_and(|r| {
            let authority_rest = r.split(['/', '?', '#']).next().unwrap_or_default();
            (r.is_empty() || r.starts_with([':', '/', '?', '#'])) && !authority_rest.contains('@')
        })
    })
}

/// Reads a registry (an object with a `plugins` array, or a bare array) fetched from `url`,
/// resolving package paths. Entries that don't check out are skipped, not fatal: one bad entry
/// mustn't hide the rest.
pub fn parse(text: &str, url: &str) -> Result<Vec<Entry>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("the plugin registry isn't valid JSON: {e}"))?;
    let list = match &root {
        Value::Array(a) => a,
        Value::Object(o) => o.get("plugins").and_then(Value::as_array).ok_or("the plugin registry has no 'plugins' list")?,
        _ => return Err("the plugin registry isn't a list of plugins".into()),
    };
    let mut out: Vec<Entry> = Vec::new();
    for v in list.iter().take(MAX_ENTRIES) {
        let Ok(mut e) = serde_json::from_value::<Entry>(v.clone()) else { continue };
        if !valid_id(&e.id, false) || version_parts(&e.version).is_none() || !is_sha256(&e.sha256) || e.name.trim().is_empty() {
            continue;
        }
        e.package = resolve(url, &e.package);
        if !allowed_url(&e.package) || out.iter().any(|o| o.id == e.id) {
            continue;
        }
        if !e.homepage.starts_with("https://") && !e.homepage.starts_with("http://") {
            e.homepage.clear();
        }
        out.push(e);
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    Ok(out)
}

/// The blocklist published beside a registry.
pub fn blocklist_url(registry: &str) -> String {
    resolve(registry, "blocklist.json")
}

/// Reads a blocklist (an object with a `blocked` array, or a bare array).
pub fn parse_blocklist(text: &str) -> Result<Vec<Blocked>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("the blocklist isn't valid JSON: {e}"))?;
    let list = match &root {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("blocked").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(list.into_iter().take(MAX_ENTRIES).filter_map(|v| serde_json::from_value::<Blocked>(v).ok()).collect())
}

/// Why a package is blocked, if it is.
pub fn blocked<'a>(list: &'a [Blocked], id: &str, sha256: &str) -> Option<&'a Blocked> {
    list.iter().find(|b| b.id == id && b.sha256.as_deref().is_none_or(|h| h.eq_ignore_ascii_case(sha256)))
}

/// Downloads `url` (at most `limit` bytes). Native only: the browser fetches with its own API.
#[cfg(not(target_arch = "wasm32"))]
pub fn fetch(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if !allowed_url(url) {
        return Err(format!("{url}: only HTTPS addresses are allowed"));
    }
    // Redirects stay on HTTPS (a local test registry may use plain HTTP, and only locally).
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .https_only(!is_local(url))
        .max_redirects(if is_local(url) { 0 } else { 5 })
        .build()
        .into();
    let mut resp =
        agent.get(url).header("User-Agent", concat!("RustGIS/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| format!("{url}: {e}"))?;
    let mut buf = Vec::new();
    resp.body_mut().with_config().limit(limit as u64 + 1).reader().read_to_end(&mut buf).map_err(|e| format!("{url}: {e}"))?;
    if buf.len() > limit {
        return Err(format!("{url} is larger than {} MB", limit / (1024 * 1024)));
    }
    Ok(buf)
}
