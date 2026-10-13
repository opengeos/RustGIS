//! Overture Maps (https://overturemaps.org): the open map data of its releases, found through its
//! STAC catalog (https://stac.overturemaps.org). A theme's type (buildings' `building`,
//! transportation's `segment`…) is a few hundred GeoParquet files, each covering an area; the
//! catalog's collection lists every file's extent, so a view needs only the files that cover it,
//! read in place by range. Each theme is also a PMTiles archive of the whole world. Requests are
//! built and answers read here, the fetching is the caller's.

use serde_json::Value;

/// Overture's STAC catalog.
pub const STAC: &str = "https://stac.overturemaps.org";
/// Where a release's PMTiles archives are.
pub const TILES: &str = "https://tiles.overturemaps.org";
/// The most files a view is read from (a city needs one or two).
pub const MAX_FILES: usize = 6;
/// The most files a collection may list.
const MAX_LISTED: usize = 20_000;

/// The themes and their types (the first type is the theme's default).
pub const THEMES: &[(&str, &[&str])] = &[
    ("buildings", &["building", "building_part"]),
    ("places", &["place"]),
    ("transportation", &["segment", "connector"]),
    ("divisions", &["division_area", "division", "division_boundary"]),
    ("base", &["land_use", "land", "land_cover", "water", "infrastructure", "bathymetry"]),
    ("addresses", &["address"]),
];

/// A theme and its type, checked against [`THEMES`] (the theme's first type by default).
pub fn theme_type(theme: &str, ty: Option<&str>) -> Result<(&'static str, &'static str), String> {
    let theme = theme.trim().to_ascii_lowercase();
    let (t, types) = THEMES
        .iter()
        .find(|(t, _)| *t == theme)
        .ok_or_else(|| format!("'{theme}' isn't an Overture theme ({})", THEMES.iter().map(|(t, _)| *t).collect::<Vec<_>>().join(", ")))?;
    let ty = match ty.map(|y| y.trim().to_ascii_lowercase()).filter(|y| !y.is_empty()) {
        None => types.first().ok_or("the theme has no types")?,
        Some(y) => types.iter().find(|x| **x == y).ok_or_else(|| format!("'{y}' isn't a type of {t} ({})", types.join(", ")))?,
    };
    Ok((t, ty))
}

/// A release's name (such as `2026-09-23.1`), checked: it goes into addresses.
pub fn release(r: &str) -> Result<String, String> {
    let r = r.trim();
    if !r.is_empty() && r.len() <= 32 && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') && !r.contains("..") {
        Ok(r.to_string())
    } else {
        Err(format!("'{r}' isn't an Overture release (such as 2026-09-23.1)"))
    }
}

/// The catalog of every release.
pub fn catalog_url() -> String {
    format!("{STAC}/catalog.json")
}

/// The latest release, from the catalog: its `latest`, else the newest release it links.
pub fn parse_latest(bytes: &[u8]) -> Result<String, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "Overture's catalog isn't JSON".to_string())?;
    if let Some(r) = v.get("latest").and_then(Value::as_str) {
        return release(r);
    }
    // Release names sort by date.
    v.get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(1000)
        .filter(|l| l.get("rel").and_then(Value::as_str) == Some("child"))
        .filter_map(|l| {
            l.get("href").and_then(Value::as_str)?.strip_prefix(STAC)?.trim_start_matches('/').split('/').next().map(str::to_string)
        })
        .filter_map(|r| release(&r).ok())
        .max()
        .ok_or_else(|| "Overture's catalog names no release".to_string())
}

/// A theme type's collection in a release: every file's extent and item.
pub fn collection_url(release: &str, theme: &str, ty: &str) -> String {
    format!("{STAC}/{release}/{theme}/{ty}/collection.json")
}

/// A theme's PMTiles archive in a release (the whole world, as map tiles).
pub fn pmtiles_url(release: &str, theme: &str) -> String {
    format!("{TILES}/{release}/{theme}.pmtiles")
}

/// One of a collection's files: its extent (west, south, east, north) and its STAC item.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub bbox: [f64; 4],
    pub item: String,
}

/// A collection's files. Its spatial extent lists the whole collection's box first, then each
/// item's, in the order of its item links.
pub fn parse_collection(bytes: &[u8]) -> Result<Vec<File>, String> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|_| "the Overture collection isn't JSON (no such theme or type in the release?)".to_string())?;
    let boxes = v.pointer("/extent/spatial/bbox").and_then(Value::as_array).ok_or("the Overture collection lists no extents")?;
    let items: Vec<&str> = v
        .get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|l| l.get("rel").and_then(Value::as_str) == Some("item"))
        .filter_map(|l| l.get("href").and_then(Value::as_str))
        .take(MAX_LISTED + 1)
        .collect();
    if items.is_empty() || items.len() > MAX_LISTED {
        return Err(format!("the Overture collection lists {} files", items.len()));
    }
    if boxes.len() != items.len() + 1 {
        return Err("the Overture collection's extents don't match its files".into());
    }
    let mut out = Vec::with_capacity(items.len());
    for (b, item) in boxes.iter().skip(1).zip(items) {
        let n: Vec<f64> = b.as_array().into_iter().flatten().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect();
        // Only Overture's own catalog is followed (an item is fetched, then its file).
        let ([w, s, e, nn], true) = (n.as_slice().try_into().unwrap_or([f64::NAN; 4]), item.starts_with(&format!("{STAC}/"))) else {
            continue;
        };
        if w <= e
            && s <= nn
            && (-180.0..=180.0).contains(&w)
            && (-180.0..=180.0).contains(&e)
            && (-90.0..=90.0).contains(&s)
            && (-90.0..=90.0).contains(&nn)
        {
            out.push(File { bbox: [w, s, e, nn], item: item.to_string() });
        }
    }
    Ok(out)
}

/// The files whose extents meet `bbox` (west, south, east, north).
pub fn files_in(files: &[File], bbox: [f64; 4]) -> Vec<&File> {
    let [w, s, e, n] = bbox;
    files.iter().filter(|f| f.bbox[0] <= e && f.bbox[2] >= w && f.bbox[1] <= n && f.bbox[3] >= s).collect()
}

/// A file's GeoParquet address, from its STAC item: the AWS asset's web address (else the first
/// GeoParquet asset's).
pub fn parse_item(bytes: &[u8]) -> Result<String, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "the Overture item isn't JSON".to_string())?;
    let assets = v.get("assets").and_then(Value::as_object).ok_or("the Overture item has no assets")?;
    let href = |a: &Value| a.get("href").and_then(Value::as_str).map(str::to_string);
    let parquet = |a: &&Value| {
        a.get("type").and_then(Value::as_str).is_some_and(|t| t.contains("parquet"))
            || a.get("href").and_then(Value::as_str).is_some_and(|h| h.split('?').next().unwrap_or(h).ends_with(".parquet"))
    };
    let url = assets
        .get("aws")
        .filter(parquet)
        .and_then(href)
        .or_else(|| assets.values().take(50).find(parquet).and_then(href))
        .ok_or("the Overture item has no GeoParquet file")?;
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("s3://"))
        || url.len() > 2048
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("the Overture item's file has no web address".into());
    }
    Ok(url)
}

/// A layer's name: `Overture building`, numbered when the view takes several files.
pub fn layer_name(ty: &str, i: usize, n: usize) -> String {
    if n > 1 { format!("Overture {ty} ({}/{n})", i + 1) } else { format!("Overture {ty}") }
}

/// What the data's licence asks to be said (it differs by theme).
pub fn attribution(theme: &str) -> &'static str {
    match theme {
        "places" => "© Overture Maps Foundation (CDLA Permissive 2.0)",
        "addresses" => "© Overture Maps Foundation and its address sources (see the release's licences)",
        _ => "© OpenStreetMap contributors, Overture Maps Foundation (ODbL)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_releases_and_addresses() {
        assert_eq!(theme_type("Buildings", None).unwrap(), ("buildings", "building"));
        assert_eq!(theme_type("transportation", Some("connector")).unwrap(), ("transportation", "connector"));
        assert!(theme_type("roads", None).is_err() && theme_type("buildings", Some("segment")).is_err());
        assert_eq!(release(" 2026-09-23.1 ").unwrap(), "2026-09-23.1");
        for bad in ["", "../x", "2026/09", "a b", "x".repeat(40).as_str()] {
            assert!(release(bad).is_err(), "{bad}");
        }
        assert_eq!(
            collection_url("2026-09-23.1", "buildings", "building"),
            "https://stac.overturemaps.org/2026-09-23.1/buildings/building/collection.json"
        );
        assert_eq!(pmtiles_url("2026-09-23.1", "places"), "https://tiles.overturemaps.org/2026-09-23.1/places.pmtiles");
        assert_eq!(layer_name("building", 0, 1), "Overture building");
        assert_eq!(layer_name("building", 1, 3), "Overture building (2/3)");
    }

    #[test]
    fn catalog_and_collection() {
        let cat = br#"{"type":"Catalog","links":[{"rel":"child","href":"https://stac.overturemaps.org/2026-08-19.0/catalog.json"},{"rel":"child","href":"https://stac.overturemaps.org/2026-09-23.1/catalog.json"}],"latest":"2026-09-23.1"}"#;
        assert_eq!(parse_latest(cat).unwrap(), "2026-09-23.1");
        // Without `latest`: the newest release linked.
        let cat = br#"{"links":[{"rel":"child","href":"https://stac.overturemaps.org/2026-08-19.0/catalog.json"},{"rel":"child","href":"https://stac.overturemaps.org/2026-09-23.0/catalog.json"},{"rel":"child","href":"https://elsewhere.example/2099-01-01.0/catalog.json"}]}"#;
        assert_eq!(parse_latest(cat).unwrap(), "2026-09-23.0");
        assert!(parse_latest(b"{}").is_err() && parse_latest(br#"{"latest":"../x"}"#).is_err());

        let coll = br#"{"extent":{"spatial":{"bbox":[[-180,-90,180,90],[0,40,10,50],[10,40,20,50],[400,0,1,1],[5,45,6,46]]}},
          "links":[{"rel":"root","href":"https://stac.overturemaps.org/catalog.json"},
            {"rel":"item","href":"https://stac.overturemaps.org/r/b/b/00000/00000.json"},
            {"rel":"item","href":"https://stac.overturemaps.org/r/b/b/00001/00001.json"},
            {"rel":"item","href":"https://stac.overturemaps.org/r/b/b/00002/00002.json"},
            {"rel":"item","href":"https://elsewhere.example/00003.json"}]}"#;
        let files = parse_collection(coll).unwrap();
        // A box out of range and an item elsewhere are left out.
        assert_eq!(files.len(), 2);
        assert_eq!(files[1], File { bbox: [10.0, 40.0, 20.0, 50.0], item: "https://stac.overturemaps.org/r/b/b/00001/00001.json".into() });
        let near = files_in(&files, [9.0, 44.0, 9.5, 44.5]);
        assert_eq!(near.len(), 1);
        assert_eq!(files_in(&files, [9.9, 44.0, 10.1, 44.5]).len(), 2);
        assert!(files_in(&files, [100.0, 0.0, 101.0, 1.0]).is_empty());
        // Extents that don't match the items, or none: an error.
        assert!(
            parse_collection(
                br#"{"extent":{"spatial":{"bbox":[[0,0,1,1]]}},"links":[{"rel":"item","href":"https://stac.overturemaps.org/a.json"}]}"#
            )
            .is_err()
        );
        assert!(parse_collection(b"<html>").is_err() && parse_collection(b"{}").is_err());
    }

    #[test]
    fn items() {
        let item = br#"{"type":"Feature","assets":{"aws":{"href":"https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com/release/r/theme=buildings/type=building/part-00000.zstd.parquet","type":"application/vnd.apache.parquet"},"azure":{"href":"https://overturemapswestus2.blob.core.windows.net/x.parquet"}}}"#;
        assert!(parse_item(item).unwrap().ends_with("part-00000.zstd.parquet"));
        // Without an AWS asset: the first GeoParquet one.
        let item = br#"{"assets":{"thumb":{"href":"https://x.example/a.png"},"data":{"href":"https://x.example/a.parquet"}}}"#;
        assert_eq!(parse_item(item).unwrap(), "https://x.example/a.parquet");
        for bad in [&br#"{"assets":{}}"#[..], br#"{"assets":{"aws":{"href":"file:///etc/x.parquet"}}}"#, b"[]"] {
            assert!(parse_item(bad).is_err());
        }
    }
}
