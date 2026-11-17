//! QGIS data sources: a layer's provider and `<datasource>` text, read into where RustGIS finds
//! the data. Paths are made absolute from the project's folder; web addresses are decoded.

/// Where a QGIS layer's data is.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A file (`ogr`, `gdal`, `spatialite`): its path, the layer or table in it, and the filter
    /// QGIS's subset string sets (as QGIS has it), and the source's options RustGIS doesn't read
    /// (`layerid=2`, `geometrytype=Point`), for the import to note.
    File { path: String, layer: Option<String>, subset: Option<String>, ignored: Vec<String> },
    /// A delimited text file (`delimitedtext`): its path, and the fields its points (or WKT
    /// geometry) are in.
    Csv { path: String, x: Option<String>, y: Option<String>, wkt: Option<String>, crs: Option<String> },
    /// XYZ tiles (`wms` with `type=xyz`), raster or vector (`vectortile`, `xyzvectortiles`), and
    /// the MapLibre style QGIS made a vector tile layer's style from (`styleUrl`), for the note.
    Xyz { url: String, zmin: Option<u8>, zmax: Option<u8>, vector: bool, style: Option<String> },
    /// A tile archive: an MBTiles file (`mbtilesvectortiles`, or `wms` with `type=mbtiles`) or a
    /// PMTiles archive (`xyzvectortiles` whose address is one), by its path or web address, as
    /// `map.addTiles` takes it (`kind`: `mbtiles` or `pmtiles`).
    Archive { path: String, kind: &'static str, vector: bool },
    /// A memory layer (`memory`): its geometry and fields. QGIS keeps no features of one in the
    /// project: it opens empty. Fields of a kind RustGIS hasn't are text (`as_text`); past
    /// [`MAX_MEMORY_FIELDS`], they are left out (`cut`).
    Memory {
        geometry: rustgis_data::GeometryType,
        /// The coordinate system its source names (`crs=EPSG:…`), when it names one.
        crs: Option<String>,
        fields: Vec<rustgis_data::Field>,
        /// The first 32 of them (with their kind), and how many there are.
        as_text: (Vec<String>, usize),
        cut: usize,
    },
    /// A virtual layer (`virtual`): its query, and when the query is `SELECT * FROM t` on one
    /// layer it names, with or without a `WHERE` clause, that layer (`from`) and the clause
    /// (`filter`). The project reads such a one as that layer's source with the filter; the rest
    /// are noted.
    Virtual { query: String, from: Option<VirtualFrom>, filter: Option<String> },
    /// A WMS or WMTS layer.
    Wms { url: String, layer: String, crs: Option<String> },
    /// A WFS or OGC API Features layer.
    Wfs { url: String, layer: String },
    /// A PostGIS table: QGIS's connection text (its password left out) and the table.
    Postgres { connection: String, table: String },
    /// A SQL Server table (`mssql`): QGIS's connection text (its password left out) and the table.
    SqlServer { connection: String, table: String },
    /// One RustGIS doesn't read.
    Other { provider: String },
}

/// The layer a virtual layer selects from: a source it names (`layer=provider:source:name`), or
/// another of the project's layers, by its id (`layer_ref=id:name`).
#[derive(Clone, Debug, PartialEq)]
pub enum VirtualFrom {
    Source(Box<Source>),
    Layer(String),
}

/// The most fields a memory layer keeps.
pub const MAX_MEMORY_FIELDS: usize = 2000;

/// The source of a layer with QGIS's `provider` and `datasource`, relative paths taken from `dir`.
pub fn read(provider: &str, datasource: &str, dir: &str) -> Source {
    match provider {
        "ogr" | "gdal" => file(datasource, dir),
        "spatialite" => {
            // `dbname='…' key='id' srid=4326 type=Point table="t" (geom) sql=…`: the filter, if
            // any, runs to the end.
            let (head, sql) = datasource.rsplit_once(" sql=").unwrap_or((datasource, ""));
            let subset = Some(sql.trim().to_string()).filter(|q| !q.is_empty());
            let kv = conninfo(head);
            // What QGIS always writes and RustGIS reads or needn't.
            const READ_OR_HARMLESS: [&str; 7] = ["dbname", "table", "key", "srid", "type", "estimatedmetadata", "checkPrimaryKeyUnicity"];
            let ignored = kv
                .iter()
                .filter(|(k, _)| !READ_OR_HARMLESS.iter().any(|r| r.eq_ignore_ascii_case(k)))
                .take(32)
                .map(|(k, v)| noted_option(&format!("{k}={v}")))
                .collect();
            match (kv.iter().find(|(k, _)| k == "dbname"), table_of(head)) {
                (Some((_, db)), Some(t)) => Source::File { path: absolute(db, dir), layer: Some(t), subset, ignored },
                _ => Source::Other { provider: provider.into() },
            }
        }
        "delimitedtext" => csv(datasource, dir),
        "wms" | "vectortile" | "xyzvectortiles" | "mbtilesvectortiles" => {
            let q = query(datasource);
            let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
            let zoom = |k: &str| get(k).and_then(|v| v.parse::<u8>().ok());
            let vector = provider != "wms";
            match (get("type").as_deref(), get("url")) {
                // An archive by its path (`file://` or a plain one, relative to the project) or
                // by its web address.
                (Some("mbtiles"), Some(url)) => Source::Archive { path: archive_path(&url, dir), kind: "mbtiles", vector },
                (Some("xyz"), Some(url)) if vector && is_pmtiles(&url) => {
                    Source::Archive { path: archive_path(&url, dir), kind: "pmtiles", vector }
                }
                (Some("xyz"), Some(url)) => Source::Xyz { url, zmin: zoom("zmin"), zmax: zoom("zmax"), vector, style: get("styleUrl") },
                (_, Some(url)) if provider == "wms" => match get("layers") {
                    Some(layer) => Source::Wms { url, layer, crs: get("crs") },
                    None => Source::Other { provider: provider.into() },
                },
                _ => Source::Other { provider: provider.into() },
            }
        }
        "gpx" => {
            // `path?type=waypoint|route|track`: RustGIS reads a GPX file as layers of each.
            let (path, q) = datasource.split_once('?').unwrap_or((datasource, ""));
            let kind = query(q).into_iter().find(|(k, _)| k == "type").map(|(_, v)| v);
            let layer = match kind.as_deref() {
                Some("waypoint") => Some("waypoints"),
                Some("route") => Some("routes"),
                Some("track") => Some("tracks"),
                _ => None,
            };
            match layer {
                Some(l) => Source::File { path: absolute(path, dir), layer: Some(l.into()), subset: None, ignored: Vec::new() },
                None => Source::Other { provider: provider.into() },
            }
        }
        "memory" => memory(datasource),
        "virtual" => virtual_layer(datasource, dir),
        "WFS" | "wfs" | "OAPIF" | "oapif" => {
            let kv = conninfo(datasource);
            let get = |k: &str| kv.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
            match (get("url"), get("typename")) {
                (Some(url), Some(layer)) => Source::Wfs { url, layer },
                _ => Source::Other { provider: provider.into() },
            }
        }
        "postgres" | "mssql" => match table_of(datasource) {
            Some(table) => {
                // The password stays out of what the import keeps or reports.
                let kept: Vec<String> = conninfo(datasource)
                    .into_iter()
                    .filter(|(k, _)| matches!(k.as_str(), "dbname" | "host" | "port" | "user" | "service" | "sslmode"))
                    // Quoted, as QGIS writes it, when a value holds a space or a quote.
                    .map(|(k, v)| if v.is_empty() || v.contains([' ', '\'']) { format!("{k}='{v}'") } else { format!("{k}={v}") })
                    .collect();
                let connection = kept.join(" ");
                if provider == "mssql" { Source::SqlServer { connection, table } } else { Source::Postgres { connection, table } }
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
            return Source::File { path: absolute(path, dir), layer: Some(table.to_string()), subset: None, ignored: Vec::new() };
        }
    }
    let mut parts = datasource.split('|');
    let path = parts.next().unwrap_or_default().trim();
    let (mut layer, mut subset, mut ignored) = (None, None, Vec::new());
    // Every part is read for the layer and the filter; at most 32 others are kept for the note.
    for p in parts {
        if let Some(v) = p.strip_prefix("layername=") {
            layer = Some(v.to_string());
        } else if let Some(v) = p.strip_prefix("subset=") {
            subset = Some(v.to_string()).filter(|s| !s.trim().is_empty());
        } else if !p.trim().is_empty() && p.trim() != "layerid=0" && ignored.len() < 32 {
            // (layerid=0 is the first layer, which is what opens.)
            ignored.push(noted_option(p.trim()));
        }
    }
    // A layer named as well as numbered: the name is what's read.
    if layer.is_some() {
        ignored.retain(|o: &String| !o.starts_with("layerid="));
    }
    Source::File { path: absolute(path, dir), layer, subset, ignored }
}

/// A source option as the import notes it: with its value only for the options known to carry
/// nothing private (`layerid=2`, `geometrytype=Point`), else by its name alone, since a value can
/// be a key, a token or a password under any name.
fn noted_option(option: &str) -> String {
    const SHOWN: [&str; 3] = ["layerid", "geometrytype", "uniquegeometrytype"];
    match option.split_once('=') {
        Some((key, _)) if !SHOWN.iter().any(|k| k.eq_ignore_ascii_case(key.trim())) => {
            format!("{}=…", key.trim().chars().take(100).collect::<String>())
        }
        _ => option.chars().take(100).collect(),
    }
}

/// Whether a tile address is a PMTiles archive (`….pmtiles`, perhaps with a query).
fn is_pmtiles(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".pmtiles")
}

/// The path of a `file:` address: `file:///abs/x` (`file:///C:/x` on Windows) or `file:./x`.
fn file_url_path(url: &str) -> Option<&str> {
    let path = url.strip_prefix("file://").or_else(|| url.strip_prefix("file:"))?;
    // `/C:/x`: a Windows drive's path.
    let b = path.as_bytes();
    let drive = b.first() == Some(&b'/') && b.get(1).is_some_and(u8::is_ascii_alphabetic) && b.get(2) == Some(&b':');
    Some(if drive { path.get(1..).unwrap_or(path) } else { path })
}

/// A tile archive's path: a `file://` address (`file:../x` when relative) as its path, a relative
/// one from the project's folder; a web address as it is.
fn archive_path(url: &str, dir: &str) -> String {
    let url = url.trim();
    match file_url_path(url) {
        Some(path) => absolute(path, dir),
        None if url.contains("://") => url.to_string(),
        None => absolute(url, dir),
    }
}

/// A memory layer's `[memory?]Point?crs=…&field=name:string(20,0)&…` (QGIS 2 and 3 put the
/// geometry first; QGIS 4 as `geometry=`).
fn memory(datasource: &str) -> Source {
    use rustgis_data::{Field, FieldType, GeometryType};
    let rest = datasource.strip_prefix("memory?").unwrap_or(datasource);
    let (head, q) = match rest.split_once('?') {
        Some((head, q)) if !head.contains('=') => (head, q),
        // `Point` alone: a geometry, nothing else.
        None if !rest.contains('=') => (rest, ""),
        _ => ("", rest),
    };
    let crs =
        q.split('&').find_map(|kv| kv.strip_prefix("crs=")).map(rustgis_data::address::percent_decode).filter(|c| !c.trim().is_empty());
    // Every field is looked at (and counted past the limit), not only `query`'s first pairs.
    let geometry = q
        .split('&')
        .find_map(|kv| kv.strip_prefix("geometry="))
        .map_or_else(|| head.to_string(), rustgis_data::address::percent_decode)
        .to_ascii_lowercase();
    let geometry = if geometry.contains("polygon") || geometry.contains("surface") {
        GeometryType::Polygon
    } else if geometry.contains("line") || geometry.contains("curve") {
        GeometryType::Polyline
    } else if geometry.contains("point") {
        GeometryType::Point
    } else {
        GeometryType::None
    };
    let (mut fields, mut as_text, mut texts, mut cut) = (Vec::new(), Vec::new(), 0usize, 0usize);
    for f in q.split('&').filter_map(|kv| kv.strip_prefix("field=")) {
        if fields.len() >= MAX_MEMORY_FIELDS {
            cut += 1;
            continue;
        }
        let f = rustgis_data::address::percent_decode(f);
        // name:type(length,precision)[[]] — the name itself may hold a colon.
        let (name, ty) = f.rsplit_once(':').unwrap_or((&f, "string"));
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let base = ty.split(['(', '[']).next().unwrap_or_default().trim().to_ascii_lowercase();
        let list = ty.contains('[');
        let ty = match base.as_str() {
            _ if list => None,
            "string" | "varchar" | "text" | "char" => Some(FieldType::Text),
            "integer" | "int" | "integer64" | "long" | "int8" | "longlong" | "short" => Some(FieldType::Integer),
            "double" | "real" | "float" | "decimal" | "numeric" => Some(FieldType::Double),
            "bool" | "boolean" => Some(FieldType::Boolean),
            "date" => Some(FieldType::Date),
            "datetime" => Some(FieldType::DateTime),
            _ => None,
        };
        texts += usize::from(ty.is_none());
        if ty.is_none() && as_text.len() < 32 {
            as_text.push(format!("{} ({})", name.chars().take(100).collect::<String>(), base.chars().take(40).collect::<String>()));
        }
        fields.push(Field::new(name, ty.unwrap_or(FieldType::Text)));
    }
    Source::Memory { geometry, crs, fields, as_text: (as_text, texts), cut }
}

/// The most layers a virtual layer's sources are looked through for (each one is read).
const MAX_VIRTUAL_LAYERS: usize = 64;

/// A virtual layer's `?layer=ogr:path:name&layer_ref=id:name&query=SELECT …` (each part of a
/// `layer=` percent-encoded on its own).
fn virtual_layer(datasource: &str, dir: &str) -> Source {
    let raw = datasource.strip_prefix('?').unwrap_or(datasource);
    let decode = rustgis_data::address::percent_decode;
    let (mut named, mut query_text) = (Vec::new(), None);
    // Every pair is looked at (the query may come last); at most MAX_VIRTUAL_LAYERS sources read.
    for kv in raw.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "query" => query_text = Some(decode(v)),
            "layer" if named.len() < MAX_VIRTUAL_LAYERS => {
                // provider:source[:name[:encoding]], each part encoded.
                let mut parts = v.split(':').map(decode);
                let (provider, source, name) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next());
                // (A virtual layer on a virtual layer isn't read as a file's layer: left as it is.)
                let from = (provider != "virtual").then(|| read(&provider, &source, dir));
                named.push((name.unwrap_or_default(), from.map(|f| VirtualFrom::Source(Box::new(f)))));
            }
            "layer_ref" if named.len() < MAX_VIRTUAL_LAYERS => {
                let v = decode(v);
                let (id, name) = v.split_once(':').unwrap_or((&v, ""));
                named.push((name.to_string(), Some(VirtualFrom::Layer(id.to_string()))));
            }
            _ => {}
        }
    }
    let query = query_text.unwrap_or_default();
    // With no query, QGIS reads its one layer whole.
    if query.trim().is_empty() && named.len() == 1 {
        let from = named.pop().and_then(|(_, from)| from);
        return Source::Virtual { query, from, filter: None };
    }
    let (from, filter) = match select_all(&query) {
        Some((table, filter)) => {
            let from = named.into_iter().find(|(n, _)| n.eq_ignore_ascii_case(&table)).and_then(|(_, from)| from);
            (from, filter)
        }
        None => (None, None),
    };
    let filter = filter.filter(|_| from.is_some());
    Source::Virtual { query, from, filter }
}

/// `SELECT * FROM t [WHERE …][;]`, as the table (`t` or `"t"`) and the `WHERE` clause; `None` for
/// any other query (several tables, columns, joins, grouping, ordering, limits).
fn select_all(query: &str) -> Option<(String, Option<String>)> {
    let q = query.trim().trim_end_matches(';').trim();
    let lower = q.to_ascii_lowercase();
    let rest = lower.strip_prefix("select")?.trim_start();
    let rest = rest.strip_prefix('*')?.trim_start();
    let rest = rest.strip_prefix("from")?;
    if !rest.starts_with(char::is_whitespace) && !rest.starts_with('"') {
        return None;
    }
    // The same place in the original text, its case kept.
    let at = q.len() - rest.len();
    let after = q.get(at..)?.trim_start();
    let (table, tail) = match after.strip_prefix('"') {
        Some(quoted) => {
            let end = quoted.find('"')?;
            (quoted.get(..end)?.to_string(), quoted.get(end + 1..)?)
        }
        None => {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            let t = after.get(..end)?;
            if t.is_empty() || !t.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            (t.to_string(), after.get(end..)?)
        }
    };
    let tail = tail.trim();
    if tail.is_empty() {
        return Some((table, None));
    }
    let lower = tail.to_ascii_lowercase();
    let clause = lower.strip_prefix("where")?;
    if !clause.starts_with(char::is_whitespace) && !clause.starts_with('(') {
        return None;
    }
    let clause = tail.get(tail.len() - clause.len()..)?.trim();
    // A clause that goes on into anything else a query has (or another query) isn't a filter: its
    // words outside quotes, split at anything that can't be in a name, have none of a query's own.
    let mut outside = String::new();
    let mut quote = None;
    for c in clause.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => outside.push(c),
        }
    }
    const OTHER: [&str; 13] =
        ["select", "from", "group", "order", "limit", "offset", "having", "union", "join", "window", "except", "intersect", "returning"];
    let words = outside.to_ascii_lowercase();
    let word = |w: &str| words.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|t| t == w);
    if clause.is_empty() || quote.is_some() || outside.contains(';') || OTHER.iter().any(|w| word(w)) {
        return None;
    }
    Some((table, Some(clause.to_string())))
}

/// A `delimitedtext` source: `file:///path.csv?type=csv&xField=…&yField=…&crs=…`.
fn csv(datasource: &str, dir: &str) -> Source {
    let (path, q) = datasource.split_once('?').unwrap_or((datasource, ""));
    // file:///absolute/path, or a relative one as file:./path.
    let path = file_url_path(path).unwrap_or(path);
    let q = query(q);
    let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    Source::Csv {
        path: absolute(&rustgis_data::address::percent_decode(path), dir),
        x: get("xField"),
        y: get("yField"),
        wkt: get("wktField"),
        crs: get("crs"),
    }
}

/// `key=value` pairs joined by `&`, each value percent-decoded (QGIS's web sources).
fn query(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .take(200)
        .filter_map(|kv| kv.split_once('=').or(Some((kv, ""))))
        .map(|(k, v)| (k.to_string(), rustgis_data::address::percent_decode(v)))
        .collect()
}

/// `key=value` and `key='value'` pairs joined by spaces (QGIS's connection texts).
fn conninfo(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() && out.len() < 100 {
        let Some((k, after)) = rest.split_once('=') else { break };
        let k = k.trim().to_string();
        let (v, next) = if let Some(quoted) = after.strip_prefix('\'') {
            // A quote or backslash escaped with a backslash is part of the value (libpq's rule).
            let mut value = String::new();
            let mut end = quoted.len();
            let mut chars = quoted.char_indices();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next().map(|(_, e)| e)),
                    '\'' => {
                        end = i + 1;
                        break;
                    }
                    c => value.push(c),
                }
            }
            (value, quoted.get(end..).unwrap_or_default())
        } else {
            let end = after.find(' ').unwrap_or(after.len());
            (after.get(..end).unwrap_or_default().to_string(), after.get(end..).unwrap_or_default())
        };
        out.push((k, v));
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
pub(super) fn absolute(path: &str, dir: &str) -> String {
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
                subset: Some("\"type\" = 'motorway'".into()),
                ignored: Vec::new()
            }
        );
        assert_eq!(
            read("ogr", "/abs/rivers.shp", dir),
            Source::File { path: "/abs/rivers.shp".into(), layer: None, subset: None, ignored: Vec::new() }
        );
        assert_eq!(
            read("gdal", "C:/maps/dem.tif", dir),
            Source::File { path: "C:/maps/dem.tif".into(), layer: None, subset: None, ignored: Vec::new() }
        );
        // Options RustGIS doesn't read are kept for a note; the first layer's number isn't one.
        let Source::File { ignored, .. } = read("ogr", "./multi.gpkg|layerid=2|geometrytype=Point", dir) else { panic!() };
        assert_eq!(ignored, ["layerid=2", "geometrytype=Point"]);
        let Source::File { ignored, .. } = read("ogr", "./one.gpkg|layerid=0", dir) else { panic!() };
        assert!(ignored.is_empty());
        let Source::File { ignored, .. } = read("ogr", "./multi.gpkg|layername=roads|layerid=3", dir) else { panic!() };
        assert!(ignored.is_empty(), "the name is read: {ignored:?}");
        // An option that may carry a secret is noted by its name only.
        let Source::File { ignored, .. } = read("ogr", "/vsis3/b/x.gpkg|credential:AWS_SECRET_ACCESS_KEY=hunter2|token=abc", dir) else {
            panic!()
        };
        assert_eq!(ignored, ["credential:AWS_SECRET_ACCESS_KEY=…", "token=…"]);
        let Source::File { ignored, .. } = read("ogr", "./x.gpkg|header=Bearer abc|layerid=4", dir) else { panic!() };
        assert_eq!(ignored, ["header=…", "layerid=4"], "values only for options known to be harmless");
        // SpatiaLite: the table, its filter, and the options that matter noted.
        let Source::File { path, layer, subset, ignored } =
            read("spatialite", r#"dbname='./db.sqlite' key='id' srid=4326 type=Point table="towns" (geom) sql="pop" > 1000"#, dir)
        else {
            panic!()
        };
        assert_eq!(
            (path.as_str(), layer.as_deref(), subset.as_deref()),
            ("/data/project/db.sqlite", Some("towns"), Some(r#""pop" > 1000"#))
        );
        assert!(ignored.is_empty(), "{ignored:?}");
        let Source::File { subset, ignored, .. } = read("spatialite", r#"dbname='/a.sqlite' password='s3cret' table="t" (geom) sql="#, dir)
        else {
            panic!()
        };
        assert_eq!((subset, ignored), (None, vec!["password=…".to_string()]));
        // The layer and the filter are read past the options kept for the note.
        let many = format!("./m.gpkg|{}|layername=late|subset=a = 1", vec!["x=1"; 40].join("|"));
        let Source::File { layer, subset, ignored, .. } = read("ogr", &many, dir) else { panic!() };
        assert_eq!((layer.as_deref(), subset.as_deref(), ignored.len()), (Some("late"), Some("a = 1"), 32));
        assert_eq!(
            read("gdal", "GPKG:./m.gpkg:relief", dir),
            Source::File { path: "/data/project/m.gpkg".into(), layer: Some("relief".into()), subset: None, ignored: Vec::new() }
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
            Source::Xyz {
                url: "https://tile.openstreetmap.org/{z}/{x}/{y}.png".into(),
                zmin: Some(0),
                zmax: Some(19),
                vector: false,
                style: None
            }
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
        // An escaped quote stays inside the password, which stays behind; a value with a space quoted.
        let pg = read("postgres", r"dbname='my gis' password='p\' user=x' table=t", dir);
        assert_eq!(pg, Source::Postgres { connection: "dbname='my gis'".into(), table: "t".into() });
        let pg = read(
            "mssql",
            "dbname='gis' host=db\\sql port=1433 user='me' password='secret' srid=2056 type=Point table=\"dbo\".\"towns\" (geom)",
            dir,
        );
        assert_eq!(pg, Source::SqlServer { connection: "dbname=gis host=db\\sql port=1433 user=me".into(), table: "dbo.towns".into() });
        assert_eq!(
            read("delimitedtext", "file:./pts.csv?wktField=geom", dir),
            Source::Csv { path: "/data/project/pts.csv".into(), x: None, y: None, wkt: Some("geom".into()), crs: None }
        );
    }

    #[test]
    fn tile_sources_read_as_qgis_4_writes_them() {
        let dir = "/data/project";
        // QGIS 4's vector tile providers, and QGIS 3's.
        for provider in ["xyzvectortiles", "vectortile"] {
            assert_eq!(
                read(provider, "type=xyz&url=https://e.org/%7Bz%7D/%7Bx%7D/%7By%7D.pbf&zmax=14&zmin=0", dir),
                Source::Xyz { url: "https://e.org/{z}/{x}/{y}.pbf".into(), zmin: Some(0), zmax: Some(14), vector: true, style: None }
            );
        }
        let Source::Xyz { style, .. } =
            read("xyzvectortiles", "styleUrl=https://e.org/style.json&type=xyz&url=https://e.org/%7Bz%7D.pbf", dir)
        else {
            panic!()
        };
        assert_eq!(style.as_deref(), Some("https://e.org/style.json"));
        // Archives: relative (as QGIS writes them beside the project), absolute and on the web.
        assert_eq!(
            read("mbtilesvectortiles", "type=mbtiles&url=..%2Fdata%2Ftiles.mbtiles", dir),
            Source::Archive { path: "/data/project/../data/tiles.mbtiles".into(), kind: "mbtiles", vector: true }
        );
        assert_eq!(
            read("wms", "type=mbtiles&url=/abs/raster.mbtiles", dir),
            Source::Archive { path: "/abs/raster.mbtiles".into(), kind: "mbtiles", vector: false }
        );
        assert_eq!(
            read("xyzvectortiles", "type=xyz&url=file%3A..%2Fdata%2Ftiles.pmtiles", dir),
            Source::Archive { path: "/data/project/../data/tiles.pmtiles".into(), kind: "pmtiles", vector: true }
        );
        assert_eq!(
            read("xyzvectortiles", "type=xyz&url=file:///abs/t.PMTiles", dir),
            Source::Archive { path: "/abs/t.PMTiles".into(), kind: "pmtiles", vector: true }
        );
        assert_eq!(
            read("xyzvectortiles", "type=xyz&url=https://e.org/t.pmtiles?key=1", dir),
            Source::Archive { path: "https://e.org/t.pmtiles?key=1".into(), kind: "pmtiles", vector: true }
        );
        // A Windows drive's file address.
        assert_eq!(
            read("mbtilesvectortiles", "type=mbtiles&url=file:///C:/maps/t.mbtiles", dir),
            Source::Archive { path: "C:/maps/t.mbtiles".into(), kind: "mbtiles", vector: true }
        );
        let Source::Csv { path, .. } = read("delimitedtext", "file:///C:/maps/p.csv?xField=x", dir) else { panic!() };
        assert_eq!(path, "C:/maps/p.csv");
        // A service's vector tiles are for the services' part.
        assert_eq!(
            read("arcgisvectortileservice", "serviceType=arcgis&type=xyz&url=https://e.org/VectorTileServer", dir),
            Source::Other { provider: "arcgisvectortileservice".into() }
        );
    }

    #[test]
    fn gpx_and_memory_sources_read_as_qgis_writes_them() {
        use rustgis_data::{Field, FieldType, GeometryType};
        let dir = "/data/project";
        for (kind, layer) in [("waypoint", "waypoints"), ("route", "routes"), ("track", "tracks")] {
            assert_eq!(
                read("gpx", &format!("./walk.gpx?type={kind}"), dir),
                Source::File { path: "/data/project/walk.gpx".into(), layer: Some(layer.into()), subset: None, ignored: Vec::new() }
            );
        }
        assert_eq!(read("gpx", "./walk.gpx", dir), Source::Other { provider: "gpx".into() });
        // QGIS 4 writes the geometry as a parameter; QGIS 3 first.
        let fields = vec![Field::new("name", FieldType::Text), Field::new("n", FieldType::Integer)];
        let crs = Some("EPSG:2056".to_string());
        assert_eq!(
            read("memory", "memory?geometry=Point&crs=EPSG:2056&field=name:string(20,0)&field=n:integer(255,0)", dir),
            Source::Memory { geometry: GeometryType::Point, crs: crs.clone(), fields: fields.clone(), as_text: (Vec::new(), 0), cut: 0 }
        );
        assert_eq!(
            read("memory", "MultiLineStringZ?crs=EPSG:2056&field=name:string(20)&field=n:integer", dir),
            Source::Memory { geometry: GeometryType::Polyline, crs, fields, as_text: (Vec::new(), 0), cut: 0 }
        );
        // A geometry alone, with or without QGIS 4's prefix.
        for source in ["Point", "memory?Point", "memory?geometry=Point"] {
            let Source::Memory { geometry, crs, .. } = read("memory", source, dir) else { panic!() };
            assert_eq!((geometry, crs), (GeometryType::Point, None), "{source}");
        }
        let Source::Memory { geometry, fields, as_text: (as_text, texts), .. } = read(
            "memory",
            "Polygon?field=a:double&field=b:boolean&field=c:date&field=d:datetime&field=e:time&field=f:string[]&field=g%3Ah:long",
            dir,
        ) else {
            panic!()
        };
        assert_eq!(geometry, GeometryType::Polygon);
        let types: Vec<(&str, FieldType)> = fields.iter().map(|f| (f.name.as_str(), f.ty)).collect();
        assert_eq!(
            types,
            [
                ("a", FieldType::Double),
                ("b", FieldType::Boolean),
                ("c", FieldType::Date),
                ("d", FieldType::DateTime),
                ("e", FieldType::Text),
                ("f", FieldType::Text),
                ("g:h", FieldType::Integer)
            ]
        );
        assert_eq!((as_text, texts), (vec!["e (time)".to_string(), "f (string)".to_string()], 2));
        let many = format!("Point?{}", (0..40).map(|i| format!("field=t{i}:time")).collect::<Vec<_>>().join("&"));
        let Source::Memory { as_text: (as_text, texts), .. } = read("memory", &many, dir) else { panic!() };
        assert_eq!((as_text.len(), texts), (32, 40));
        let Source::Memory { geometry, fields, .. } = read("memory", "None", dir) else { panic!() };
        assert_eq!((geometry, fields.len()), (GeometryType::None, 0));
        let many = format!("Point?{}", (0..MAX_MEMORY_FIELDS + 3).map(|i| format!("field=f{i}:integer")).collect::<Vec<_>>().join("&"));
        let Source::Memory { fields, cut, .. } = read("memory", &many, dir) else { panic!() };
        assert_eq!((fields.len(), cut), (MAX_MEMORY_FIELDS, 3));
    }

    #[test]
    fn virtual_layers_selecting_one_layer_read_as_it() {
        let dir = "/data/project";
        let gpkg = Source::File {
            path: "/data/project/../data/sample.gpkg".into(),
            layer: Some("points".into()),
            subset: None,
            ignored: Vec::new(),
        };
        let from = |s: &Source| Some(VirtualFrom::Source(Box::new(s.clone())));
        // As QGIS writes it: each part of a source percent-encoded.
        let v = read(
            "virtual",
            "?layer=ogr:..%2Fdata%2Fsample.gpkg%7Clayername%3Dpoints:pts&query=SELECT%20*%20FROM%20pts%20WHERE%20value%20%3E%2020",
            dir,
        );
        assert_eq!(
            v,
            Source::Virtual { query: "SELECT * FROM pts WHERE value > 20".into(), from: from(&gpkg), filter: Some("value > 20".into()) }
        );
        let v =
            read("virtual", "?layer=ogr:..%2Fdata%2Fsample.gpkg%7Clayername%3Dpoints:pts:UTF-8&query=select%20*%20from%20%22Pts%22;", dir);
        assert_eq!(v, Source::Virtual { query: r#"select * from "Pts";"#.into(), from: from(&gpkg), filter: None });
        let v = read("virtual", "?layer_ref=points_1:pts&query=SELECT%20*%20FROM%20pts%20WHERE%20(kind%20=%20'a')", dir);
        assert_eq!(
            v,
            Source::Virtual {
                query: "SELECT * FROM pts WHERE (kind = 'a')".into(),
                from: Some(VirtualFrom::Layer("points_1".into())),
                filter: Some("(kind = 'a')".into())
            }
        );
        // Anything else a query does isn't a layer with a filter.
        for q in [
            "SELECT kind, count(*) FROM pts GROUP BY kind",
            "SELECT * FROM pts WHERE value > 2 ORDER BY value",
            "SELECT * FROM pts, other",
            "SELECT * FROM pts JOIN other ON pts.id = other.id",
            "SELECT * FROM pts WHERE id IN (SELECT id FROM other)",
            "SELECT * FROM pts LIMIT 3",
            "SELECT * FROM pts WHERE a = 1 LIMIT(3)",
            "SELECT * FROM pts WHERE a = 1 ORDER BY(b)",
            "SELECT * FROM pts WHERE (a=1)ORDER BY b",
            "SELECT * FROM pts WHERE id IN(SELECT id FROM(other))",
            "SELECT * FROM pts WHERE x = 1;  DELETE FROM y",
            "SELECT * FROM pts WHERE name = 'open",
            "SELECT * FROM nosuch",
        ] {
            let Source::Virtual { from, filter, .. } =
                read("virtual", &format!("?layer=ogr:..%2Fdata%2Fsample.gpkg:pts&query={}", q.replace(' ', "%20")), dir)
            else {
                panic!()
            };
            assert_eq!((from, filter), (None, None), "{q}");
        }
        // Words of a query in quotes are values, not the query's.
        let Source::Virtual { filter, .. } = read(
            "virtual",
            "?layer=ogr:..%2Fdata%2Fsample.gpkg:pts&query=SELECT%20*%20FROM%20pts%20WHERE%20name%20=%20'order%20by;%20me'",
            dir,
        ) else {
            panic!()
        };
        assert_eq!(filter.as_deref(), Some("name = 'order by; me'"));
        // With no query, its one layer whole; a query after many other pairs is read too.
        let Source::Virtual { from, filter, .. } = read("virtual", "?layer=ogr:..%2Fdata%2Fsample.gpkg%7Clayername%3Dpoints:pts", dir)
        else {
            panic!()
        };
        assert_eq!((from, filter), (Some(VirtualFrom::Source(Box::new(gpkg.clone()))), None));
        let many = format!("?{}&query=SELECT%20*%20FROM%20pts&layer=ogr:a.gpkg:pts", vec!["x=1"; 300].join("&"));
        assert!(matches!(read("virtual", &many, dir), Source::Virtual { from: Some(_), .. }));
        // A virtual layer on a virtual layer isn't read as a file's layer.
        let nested = "?layer=virtual:%3Flayer%3Dogr%253Aa.gpkg%253At:v&query=SELECT%20*%20FROM%20v";
        assert_eq!(read("virtual", nested, dir), Source::Virtual { query: "SELECT * FROM v".into(), from: None, filter: None });
    }
}
