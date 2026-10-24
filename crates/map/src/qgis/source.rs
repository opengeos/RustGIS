//! QGIS data sources: a layer's provider and `<datasource>` text, read into where RustGIS finds
//! the data. Paths are made absolute from the project's folder; web addresses are decoded.

/// Where a QGIS layer's data is.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A file (`ogr`, `gdal`, `spatialite`): its path, the layer or table in it, and the filter
    /// QGIS's subset string sets (as QGIS has it).
    File { path: String, layer: Option<String>, subset: Option<String> },
    /// A delimited text file (`delimitedtext`): its path, and the fields its points (or WKT
    /// geometry) are in.
    Csv { path: String, x: Option<String>, y: Option<String>, wkt: Option<String>, crs: Option<String> },
    /// XYZ tiles (`wms` with `type=xyz`), raster or vector (`vectortile`).
    Xyz { url: String, zmin: Option<u8>, zmax: Option<u8>, vector: bool },
    /// A WMS or WMTS layer.
    Wms { url: String, layer: String, crs: Option<String> },
    /// A WFS or OGC API Features layer.
    Wfs { url: String, layer: String },
    /// A PostGIS table: QGIS's connection text (its password left out) and the table.
    Postgres { connection: String, table: String },
    /// One RustGIS doesn't read.
    Other { provider: String },
}

/// The source of a layer with QGIS's `provider` and `datasource`, relative paths taken from `dir`.
pub fn read(provider: &str, datasource: &str, dir: &str) -> Source {
    match provider {
        "ogr" | "gdal" => file(datasource, dir),
        "spatialite" => {
            let kv = conninfo(datasource);
            match (kv.iter().find(|(k, _)| k == "dbname"), table_of(datasource)) {
                (Some((_, db)), Some(t)) => Source::File { path: absolute(db, dir), layer: Some(t), subset: None },
                _ => Source::Other { provider: provider.into() },
            }
        }
        "delimitedtext" => csv(datasource, dir),
        "wms" | "vectortile" => {
            let q = query(datasource);
            let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
            let zoom = |k: &str| get(k).and_then(|v| v.parse::<u8>().ok());
            match (get("type").as_deref(), get("url")) {
                (Some("xyz"), Some(url)) => Source::Xyz { url, zmin: zoom("zmin"), zmax: zoom("zmax"), vector: provider == "vectortile" },
                (_, Some(url)) if provider == "wms" => match get("layers") {
                    Some(layer) => Source::Wms { url, layer, crs: get("crs") },
                    None => Source::Other { provider: provider.into() },
                },
                _ => Source::Other { provider: provider.into() },
            }
        }
        "WFS" | "wfs" | "OAPIF" | "oapif" => {
            let kv = conninfo(datasource);
            let get = |k: &str| kv.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
            match (get("url"), get("typename")) {
                (Some(url), Some(layer)) => Source::Wfs { url, layer },
                _ => Source::Other { provider: provider.into() },
            }
        }
        "postgres" => match table_of(datasource) {
            Some(table) => {
                // The password stays out of what the import keeps or reports.
                let kept: Vec<String> = conninfo(datasource)
                    .into_iter()
                    .filter(|(k, _)| matches!(k.as_str(), "dbname" | "host" | "port" | "user" | "service" | "sslmode"))
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                Source::Postgres { connection: kept.join(" "), table }
            }
            None => Source::Other { provider: provider.into() },
        },
        _ => Source::Other { provider: provider.into() },
    }
}

/// An `ogr` or `gdal` source: `path|layername=…|subset=…` (or `|layerid=…`), or GDAL's
/// `GPKG:path:table`.
fn file(datasource: &str, dir: &str) -> Source {
    if let Some(rest) = datasource.strip_prefix("GPKG:") {
        // GPKG:path:table, the path itself possibly with a drive letter's colon.
        if let Some((path, table)) = rest.rsplit_once(':') {
            return Source::File { path: absolute(path, dir), layer: Some(table.to_string()), subset: None };
        }
    }
    let mut parts = datasource.split('|');
    let path = parts.next().unwrap_or_default().trim();
    let (mut layer, mut subset) = (None, None);
    for p in parts {
        if let Some(v) = p.strip_prefix("layername=") {
            layer = Some(v.to_string());
        } else if let Some(v) = p.strip_prefix("subset=") {
            subset = Some(v.to_string()).filter(|s| !s.trim().is_empty());
        }
    }
    Source::File { path: absolute(path, dir), layer, subset }
}

/// A `delimitedtext` source: `file:///path.csv?type=csv&xField=…&yField=…&crs=…`.
fn csv(datasource: &str, dir: &str) -> Source {
    let (path, q) = datasource.split_once('?').unwrap_or((datasource, ""));
    // file:///absolute/path, or a relative one as file:./path.
    let path = path.strip_prefix("file://").or_else(|| path.strip_prefix("file:")).unwrap_or(path);
    let q = query(q);
    let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    Source::Csv { path: absolute(&decode(path), dir), x: get("xField"), y: get("yField"), wkt: get("wktField"), crs: get("crs") }
}

/// `key=value` pairs joined by `&`, each value percent-decoded (QGIS's web sources).
fn query(s: &str) -> Vec<(String, String)> {
    s.split('&').take(200).filter_map(|kv| kv.split_once('=').or(Some((kv, "")))).map(|(k, v)| (k.to_string(), decode(v))).collect()
}

/// `key=value` and `key='value'` pairs joined by spaces (QGIS's connection texts).
fn conninfo(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() && out.len() < 100 {
        let Some((k, after)) = rest.split_once('=') else { break };
        let k = k.trim().to_string();
        let (v, next) = if let Some(quoted) = after.strip_prefix('\'') {
            match quoted.find('\'') {
                Some(end) => (quoted.get(..end).unwrap_or_default(), quoted.get(end + 1..).unwrap_or_default()),
                None => (quoted, ""),
            }
        } else {
            let end = after.find(' ').unwrap_or(after.len());
            (after.get(..end).unwrap_or_default(), after.get(end..).unwrap_or_default())
        };
        out.push((k, v.to_string()));
        rest = next.trim_start();
    }
    out
}

/// A connection text's `table="schema"."table" (geom)`, as `schema.table` (or `table`).
fn table_of(s: &str) -> Option<String> {
    let at = s.find("table=")?;
    let rest = s.get(at + 6..)?;
    let end = rest.find(" (").or_else(|| rest.find(' ')).unwrap_or(rest.len());
    let t = rest.get(..end)?.replace('"', "");
    (!t.is_empty()).then_some(t)
}

/// `path` from the project's folder `dir`, unless it is absolute or a web address.
fn absolute(path: &str, dir: &str) -> String {
    let path = path.trim();
    if path.is_empty() || !is_relative(path) || dir.is_empty() {
        return path.to_string();
    }
    let rel = path.strip_prefix("./").unwrap_or(path);
    format!("{}/{}", dir.trim_end_matches(['/', '\\']), rel)
}

/// Whether `path` is relative to a Windows drive's working directory (`C:roads.gpkg`, not
/// `C:\…` or `C:/…`): no project folder tells where that is.
pub fn is_drive_relative(path: &str) -> bool {
    let b = path.trim().as_bytes();
    b.get(1) == Some(&b':') && b.first().is_some_and(u8::is_ascii_alphabetic) && !matches!(b.get(2), Some(b'/' | b'\\'))
}

/// Whether `path` is relative to the project's folder: not absolute (`/…`, `\…`, `C:\…`), not
/// on a drive (`C:…`, see [`is_drive_relative`]) and not a web address.
pub fn is_relative(path: &str) -> bool {
    let path = path.trim();
    let windows_drive = path.as_bytes().get(1) == Some(&b':') && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    !(path.starts_with('/') || path.starts_with('\\') || windows_drive || path.contains("://"))
}

/// Percent-decodes `s` (`%7B` to `{`); a broken escape is kept as it is.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| b.get(j).and_then(|h| (*h as char).to_digit(16));
        match (c, hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_read_as_their_providers_write_them() {
        let dir = "/data/project";
        assert_eq!(
            read("ogr", "./roads.gpkg|layername=roads|subset=\"type\" = 'motorway'", dir),
            Source::File {
                path: "/data/project/roads.gpkg".into(),
                layer: Some("roads".into()),
                subset: Some("\"type\" = 'motorway'".into())
            }
        );
        assert_eq!(read("ogr", "/abs/rivers.shp", dir), Source::File { path: "/abs/rivers.shp".into(), layer: None, subset: None });
        assert_eq!(read("gdal", "C:/maps/dem.tif", dir), Source::File { path: "C:/maps/dem.tif".into(), layer: None, subset: None });
        assert_eq!(
            read("gdal", "GPKG:./m.gpkg:relief", dir),
            Source::File { path: "/data/project/m.gpkg".into(), layer: Some("relief".into()), subset: None }
        );
        assert_eq!(
            read("delimitedtext", "file:///data/pts%20a.csv?type=csv&xField=lon&yField=lat&crs=EPSG:4326", dir),
            Source::Csv {
                path: "/data/pts a.csv".into(),
                x: Some("lon".into()),
                y: Some("lat".into()),
                wkt: None,
                crs: Some("EPSG:4326".into())
            }
        );
        assert_eq!(
            read("wms", "type=xyz&url=https://tile.openstreetmap.org/%7Bz%7D/%7Bx%7D/%7By%7D.png&zmax=19&zmin=0", dir),
            Source::Xyz { url: "https://tile.openstreetmap.org/{z}/{x}/{y}.png".into(), zmin: Some(0), zmax: Some(19), vector: false }
        );
        assert_eq!(
            read("wms", "crs=EPSG:2056&format=image/png&layers=ch.swisstopo.pixelkarte-farbe&styles&url=https://wms.geo.admin.ch/", dir),
            Source::Wms {
                url: "https://wms.geo.admin.ch/".into(),
                layer: "ch.swisstopo.pixelkarte-farbe".into(),
                crs: Some("EPSG:2056".into())
            }
        );
        // A password stays behind.
        let pg = read("postgres", "dbname='gis' host=db port=5432 user='me' password='secret' table=\"public\".\"roads\" (geom)", dir);
        assert_eq!(pg, Source::Postgres { connection: "dbname=gis host=db port=5432 user=me".into(), table: "public.roads".into() });
        assert_eq!(read("memory", "Point?crs=EPSG:4326", dir), Source::Other { provider: "memory".into() });
        assert_eq!(
            read("delimitedtext", "file:./pts.csv?wktField=geom", dir),
            Source::Csv { path: "/data/project/pts.csv".into(), x: None, y: None, wkt: Some("geom".into()), crs: None }
        );
        assert_eq!(decode("%E2%9C%93%zz"), "✓%zz");
    }
}
