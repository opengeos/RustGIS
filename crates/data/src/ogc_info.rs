//! WMS GetFeatureInfo (WMS 1.3.0 §7.4, 1.1.1 §7.3): how a service's layer is asked what is at a
//! point of the map, read from its capabilities; the request for a point; and the answer —
//! GeoJSON, GML (a WFS-style feature collection or MapServer's), Esri's XML, plain text or
//! HTML — read as features. Answers are hostile input: they are capped like capabilities.

use roxmltree::Node;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{base, child, children, href, is_web, local, query_value, text_of, with_query, xml};

/// How to ask a WMS layer what is at a point. A layer keeps it for the session; it can be saved
/// (a project might keep it one day).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureInfo {
    /// The GetFeatureInfo address (the request's HTTP GET online resource).
    pub url: String,
    /// The WMS version the service is asked in: `1.3.0` or `1.1.1`.
    pub version: String,
    /// The layer's name: its LAYERS and QUERY_LAYERS.
    pub layer: String,
    /// The layer's GetMap image format (part of the request).
    pub map_format: String,
    /// The answer formats (INFO_FORMAT) the service offers; none when the layer isn't queryable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<String>,
}

/// Pixels across the map window a request describes, the point in its middle pixel.
const WINDOW: u32 = 101;
/// Most features an answer is asked for.
const FEATURE_COUNT: usize = 10;
/// Most features read from an answer, attributes kept of each, and characters of a value.
const MAX_FEATURES: usize = 100;
const MAX_ATTRIBUTES: usize = 200;
const MAX_VALUE: usize = 4000;

impl FeatureInfo {
    /// Whether the layer can be asked (the service offers GetFeatureInfo and calls it queryable).
    pub fn queryable(&self) -> bool {
        !self.formats.is_empty() && !self.layer.is_empty() && is_web(&self.url)
    }

    /// The service's request for one of its layers.
    pub(super) fn for_layer(&self, layer: &str, queryable: bool) -> FeatureInfo {
        let formats = if queryable { self.formats.clone() } else { Vec::new() };
        FeatureInfo { layer: layer.to_string(), formats, ..self.clone() }
    }

    /// The answer format asked for: GeoJSON, then GML, then other XML, plain text and HTML.
    pub fn info_format(&self) -> Option<&str> {
        let rank = |f: &str| {
            let f = f.to_ascii_lowercase();
            if f.contains("geojson") || f.contains("geo+json") || f == "application/json" {
                0
            } else if f.contains("gml") {
                1
            } else if f.contains("xml") && !f.contains("html") {
                2
            } else if f.starts_with("text/plain") {
                3
            } else if f.contains("html") {
                4
            } else {
                5
            }
        };
        self.formats.iter().filter(|f| rank(f) < 5).min_by_key(|f| rank(f)).map(String::as_str)
    }

    /// The GetFeatureInfo address for a point (`x`, `y` in Web Mercator metres) of a map drawn at
    /// `m_per_px` metres per pixel: a small window of the map around the point, which is its
    /// middle pixel. `None` when the layer can't be asked.
    pub fn request(&self, x: f64, y: f64, m_per_px: f64) -> Option<String> {
        if !self.queryable() || !(x.is_finite() && y.is_finite() && m_per_px.is_finite() && m_per_px > 0.0) {
            return None;
        }
        let format = self.info_format()?;
        let half = f64::from(WINDOW) / 2.0 * m_per_px;
        let bbox = format!("{},{},{},{}", x - half, y - half, x + half, y + half);
        let mid = WINDOW / 2;
        let v13 = self.version.starts_with("1.3");
        let (crs, i, j, exceptions) = if v13 { ("CRS", "I", "J", "XML") } else { ("SRS", "X", "Y", "application/vnd.ogc.se_xml") };
        let layer = super::encode(&self.layer);
        let map_format = if self.map_format.is_empty() { "image/png" } else { &self.map_format };
        Some(with_query(
            &self.url,
            &format!(
                "SERVICE=WMS&VERSION={}&REQUEST=GetFeatureInfo&LAYERS={layer}&STYLES=&{crs}=EPSG:3857&BBOX={bbox}&WIDTH={WINDOW}&HEIGHT={WINDOW}&FORMAT={}&TRANSPARENT=TRUE&QUERY_LAYERS={layer}&INFO_FORMAT={}&FEATURE_COUNT={FEATURE_COUNT}&{i}={mid}&{j}={mid}&EXCEPTIONS={}",
                super::encode(&self.version),
                super::encode(map_format),
                super::encode(format),
                super::encode(exceptions),
            ),
        ))
    }
}

/// A capabilities document's GetFeatureInfo request, for no layer yet: `None` when the service
/// doesn't offer one.
pub(super) fn request_of(root: &Node, url: &str, version: &str, map_format: &str) -> Option<FeatureInfo> {
    let request = root.descendants().find(|n| n.is_element() && local(n) == "GetFeatureInfo")?;
    let formats: Vec<String> = children(&request, "Format").map(|f| text_of(&f)).filter(|f| !f.is_empty()).take(32).collect();
    let get = request
        .descendants()
        .find(|n| n.is_element() && local(n) == "Get")
        .and_then(|g| child(&g, "OnlineResource"))
        .and_then(|o| href(&o))
        .filter(|h| is_web(h))
        .unwrap_or_else(|| super::endpoint(url));
    Some(FeatureInfo { url: get, version: version.to_string(), layer: String::new(), map_format: map_format.to_string(), formats })
}

/// The GetMap parameters a WMS tile template carries, left out of its service's address.
const GET_MAP_KEYS: [&str; 16] = [
    "service",
    "request",
    "version",
    "layers",
    "styles",
    "crs",
    "srs",
    "bbox",
    "width",
    "height",
    "format",
    "transparent",
    "bgcolor",
    "exceptions",
    "time",
    "elevation",
];

/// A WMS tile template's (a GetMap request's) service and layer: its GetCapabilities address and
/// the layer's name. How a layer whose GetFeatureInfo request isn't known (one from a project)
/// finds it. `None` for other templates.
pub fn wms_layer_of(template: &str) -> Option<(String, String)> {
    let is = |key: &str, value: &str| query_value(template, key).is_some_and(|v| v.eq_ignore_ascii_case(value));
    if !is_web(template) || !is("service", "WMS") || !is("request", "GetMap") {
        return None;
    }
    let decode = |v: &str| url::form_urlencoded::parse(format!("v={v}").as_bytes()).next().map(|(_, v)| v.into_owned());
    let layer = query_value(template, "layers").and_then(decode).filter(|l| !l.is_empty())?;
    let version = query_value(template, "version").and_then(decode).filter(|v| v.starts_with("1.1") || v.starts_with("1.3"));
    let kept: Vec<&str> = template
        .split('#')
        .next()
        .and_then(|u| u.split_once('?'))
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter(|kv| {
            let k = kv.split_once('=').map(|(k, _)| k).unwrap_or(kv).to_ascii_lowercase();
            !kv.is_empty() && !GET_MAP_KEYS.contains(&k.as_str())
        })
        .collect();
    let service = if kept.is_empty() { base(template).to_string() } else { format!("{}?{}", base(template), kept.join("&")) };
    let mut q = String::from("SERVICE=WMS&REQUEST=GetCapabilities");
    if let Some(v) = version {
        q.push_str(&format!("&VERSION={}", super::encode(&v)));
    }
    Some((with_query(&service, &q), layer))
}

/// A feature of a GetFeatureInfo answer: the layer it is in, when the answer says, and its
/// attributes as text, in the answer's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InfoFeature {
    pub layer: Option<String>,
    pub attributes: Vec<(String, String)>,
}

/// Reads a GetFeatureInfo answer, whatever its format (found from the answer itself): its
/// features, none when nothing is at the point, or the service's exception as the error.
pub fn read_feature_info(body: &[u8]) -> Result<Vec<InfoFeature>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "the service's answer isn't text (perhaps an image)".to_string())?;
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    if text.starts_with('{') || text.starts_with('[') {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("the service's JSON: {e}"))?;
        return Ok(json_features(&v));
    }
    if text.starts_with('<') {
        let lower = text.get(..text.len().min(512)).unwrap_or(text).to_ascii_lowercase();
        if lower.contains("<html") || lower.contains("<!doctype html") || lower.contains("<body") {
            return Ok(text_feature(&html_text(text)));
        }
        return match xml(text, 1_000_000) {
            Ok(doc) => xml_features(&doc.root_element()),
            // Not XML after all (an HTML fragment): its text.
            Err(_) => Ok(text_feature(&html_text(text))),
        };
    }
    Ok(plain_features(text))
}

fn capped(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_VALUE) {
        Some((i, _)) => format!("{}…", s.get(..i).unwrap_or(s)),
        None => s.to_string(),
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => capped(s),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => capped(&other.to_string()),
    }
}

/// A GeoJSON feature collection's (or one feature's) features. The layer is the feature's
/// `layerName` or `layerBodId` (swisstopo's), or the part of a GeoServer id before its dot.
fn json_features(v: &Value) -> Vec<InfoFeature> {
    let features: Vec<&Value> = match v.get("features").and_then(Value::as_array) {
        Some(list) => list.iter().collect(),
        None if v.get("type").and_then(Value::as_str) == Some("Feature") => vec![v],
        None => match v.get("results").and_then(Value::as_array) {
            Some(list) => list.iter().collect(),
            None => Vec::new(),
        },
    };
    features
        .into_iter()
        .take(MAX_FEATURES)
        .map(|f| {
            let attributes = f
                .get("properties")
                .or_else(|| f.get("attributes"))
                .and_then(Value::as_object)
                .map(|o| o.iter().take(MAX_ATTRIBUTES).map(|(k, v)| (k.clone(), value_text(v))).collect())
                .unwrap_or_default();
            let layer =
                ["layerName", "layerBodId", "layer"].iter().find_map(|k| f.get(*k).and_then(Value::as_str)).map(str::to_string).or_else(
                    || {
                        let id = f.get("id").and_then(Value::as_str)?;
                        let (layer, rest) = id.rsplit_once('.')?;
                        (!layer.is_empty() && !rest.is_empty()).then(|| layer.to_string())
                    },
                );
            InfoFeature { layer, attributes }
        })
        .collect()
}

/// Whether an element is in the GML namespace (geometry, bounds, a GML name).
fn is_gml(n: &Node) -> bool {
    n.tag_name().namespace().is_some_and(|ns| ns.starts_with("http://www.opengis.net/gml"))
}

/// A GML feature's attributes: its child elements holding text only, not GML's own.
fn gml_attributes(feature: &Node) -> Vec<(String, String)> {
    feature
        .children()
        .filter(|c| c.is_element() && !is_gml(c) && !c.children().any(|g| g.is_element()))
        .take(MAX_ATTRIBUTES)
        .map(|c| (local(&c).to_string(), capped(&text_of(&c))))
        .collect()
}

fn xml_features(root: &Node) -> Result<Vec<InfoFeature>, String> {
    let mut out = Vec::new();
    match local(root) {
        "ServiceExceptionReport" | "ExceptionReport" => {
            return Err(format!("the service answered with an error: {}", text_of(root).chars().take(300).collect::<String>()));
        }
        // MapServer: <msGMLOutput><roads_layer><roads_feature>…
        "msGMLOutput" => {
            for layer in root.children().filter(|c| c.is_element()) {
                let name = local(&layer).strip_suffix("_layer").unwrap_or(local(&layer)).to_string();
                for f in layer.children().filter(|c| c.is_element() && local(c).ends_with("_feature")) {
                    out.push(InfoFeature { layer: Some(name.clone()), attributes: gml_attributes(&f) });
                }
            }
        }
        // ArcGIS Server: <FeatureInfoResponse><FeatureInfoCollection layername="…"><FeatureInfo>
        // <Field><FieldName/><FieldValue/></Field>…, or <FIELDS a="…" b="…"/> (1.1.1).
        "FeatureInfoResponse" => {
            for f in root.descendants().filter(|n| n.is_element() && matches!(local(n), "FIELDS" | "FeatureInfo")) {
                let layer = f.ancestors().find_map(|a| a.attribute("layername")).map(str::to_string);
                let attributes: Vec<(String, String)> = if local(&f) == "FIELDS" {
                    f.attributes().take(MAX_ATTRIBUTES).map(|a| (a.name().to_string(), capped(a.value()))).collect()
                } else {
                    children(&f, "Field")
                        .take(MAX_ATTRIBUTES)
                        .filter_map(|field| {
                            let name = child(&field, "FieldName").map(|n| text_of(&n)).filter(|n| !n.is_empty())?;
                            Some((name, capped(&child(&field, "FieldValue").map(|v| text_of(&v)).unwrap_or_default())))
                        })
                        .collect()
                };
                out.push(InfoFeature { layer, attributes });
            }
        }
        // GML: a feature collection's members (GML 2/3 featureMember(s), WFS 2 member).
        _ => {
            let is_member = |n: &Node| n.is_element() && matches!(local(n), "featureMember" | "featureMembers" | "member");
            // A member inside another is a feature's attribute (one named `member`), not a feature.
            for member in root.descendants().filter(|n| is_member(n) && !n.ancestors().skip(1).any(|a| is_member(&a))) {
                for f in member.children().filter(|c| c.is_element()) {
                    out.push(InfoFeature { layer: Some(local(&f).to_string()), attributes: gml_attributes(&f) });
                }
            }
        }
    }
    out.truncate(MAX_FEATURES);
    Ok(out)
}

/// A plain text answer's features: `name = value` lines (GeoServer's and MapServer's), a feature
/// at each layer heading, `Feature` line or dashed rule after attributes; any other text as one.
fn plain_features(text: &str) -> Vec<InfoFeature> {
    let mut out: Vec<InfoFeature> = Vec::new();
    let mut layer: Option<String> = None;
    let mut current: Vec<(String, String)> = Vec::new();
    fn flush(layer: &Option<String>, current: &mut Vec<(String, String)>, out: &mut Vec<InfoFeature>) {
        if !current.is_empty() && out.len() < MAX_FEATURES {
            out.push(InfoFeature { layer: layer.clone(), attributes: std::mem::take(current) });
        }
        current.clear();
    }
    for line in text.lines().take(100_000) {
        let t = line.trim();
        let quoted = |rest: &str| rest.split(['\'', '"']).nth(1).map(str::to_string);
        // A `name = value` line is an attribute, whatever its name starts with ("Layer name = x").
        let heading = !t.contains(" = ");
        if let Some(rest) = t.strip_prefix("Layer ").or_else(|| t.strip_prefix("Results for FeatureType")).filter(|_| heading) {
            flush(&layer, &mut current, &mut out);
            layer = quoted(rest).or(layer.take());
        } else if (heading && t.starts_with("Feature ")) || (t.len() >= 3 && t.bytes().all(|b| b == b'-' || b == b'=')) {
            flush(&layer, &mut current, &mut out);
        } else if let Some((k, v)) = t.split_once(" = ")
            && current.len() < MAX_ATTRIBUTES
        {
            let v = v.trim();
            let v = v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).unwrap_or(v);
            current.push((k.trim().to_string(), capped(v)));
        }
    }
    flush(&layer, &mut current, &mut out);
    if out.is_empty() { text_feature(text) } else { out }
}

/// Text that isn't features (an HTML page's, a sentence): one feature with it, none for no text.
fn text_feature(text: &str) -> Vec<InfoFeature> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() { Vec::new() } else { vec![InfoFeature { layer: None, attributes: vec![("text".to_string(), capped(&t))] }] }
}

/// An HTML answer's text: its tags, scripts and styles left out.
fn html_text(html: &str) -> String {
    // ASCII lowercasing keeps byte offsets, so one copy serves every search (a copy per tag
    // would be quadratic on an answer of many short scripts).
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut at = 0;
    while let Some(open) = html.get(at..).and_then(|r| r.find('<')).map(|i| at + i) {
        out.push_str(html.get(at..open).unwrap_or(""));
        out.push(' ');
        let tag = lower.get(open..).unwrap_or("");
        let skip_to = if tag.starts_with("<script") {
            tag.find("</script>").map(|i| i + 9)
        } else if tag.starts_with("<style") {
            tag.find("</style>").map(|i| i + 8)
        } else {
            tag.find('>').map(|i| i + 1)
        };
        at = skip_to.map_or(html.len(), |i| open + i);
    }
    out.push_str(html.get(at..).unwrap_or(""));
    out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS_13: &str = r#"<?xml version="1.0"?>
<WMS_Capabilities version="1.3.0" xmlns="http://www.opengis.net/wms" xmlns:xlink="http://www.w3.org/1999/xlink">
  <Service><Title>Test</Title></Service>
  <Capability>
    <Request>
      <GetMap><Format>image/png</Format>
        <DCPType><HTTP><Get><OnlineResource xlink:href="https://maps.example.org/wms?map=x&amp;"/></Get></HTTP></DCPType>
      </GetMap>
      <GetFeatureInfo><Format>text/plain</Format><Format>application/vnd.ogc.gml</Format><Format>application/json</Format>
        <DCPType><HTTP><Get><OnlineResource xlink:href="https://info.example.org/wms?"/></Get></HTTP></DCPType>
      </GetFeatureInfo>
    </Request>
    <Layer queryable="1"><Title>Root</Title><CRS>EPSG:3857</CRS>
      <Layer><Name>roads</Name><Title>Roads</Title></Layer>
      <Layer queryable="0"><Name>relief</Name><Title>Relief</Title></Layer>
    </Layer>
  </Capability>
</WMS_Capabilities>"#;

    fn info_of(caps: &str, layer: &str) -> FeatureInfo {
        let service = crate::ogc::parse("https://maps.example.org/wms?SERVICE=WMS&REQUEST=GetCapabilities", caps.as_bytes()).unwrap();
        service.layers.into_iter().find(|l| l.name == layer).and_then(|l| l.feature_info).unwrap()
    }

    #[test]
    fn capabilities_give_the_request_and_queryable_is_inherited() {
        let roads = info_of(CAPS_13, "roads");
        assert!(roads.queryable());
        assert_eq!(roads.url, "https://info.example.org/wms?");
        assert_eq!(roads.version, "1.3.0");
        assert_eq!(roads.info_format(), Some("application/json"));
        let relief = info_of(CAPS_13, "relief");
        assert!(!relief.queryable());
        assert_eq!(relief.request(0.0, 0.0, 10.0), None);
    }

    #[test]
    fn requests_follow_the_version() {
        let roads = info_of(CAPS_13, "roads");
        let r = roads.request(1000.0, 2000.0, 2.0).unwrap();
        assert!(r.starts_with(
            "https://info.example.org/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetFeatureInfo&LAYERS=roads&STYLES=&CRS=EPSG:3857"
        ));
        assert!(r.contains("&BBOX=899,1899,1101,2101&WIDTH=101&HEIGHT=101&"));
        assert!(r.contains("&QUERY_LAYERS=roads&INFO_FORMAT=application%2Fjson&FEATURE_COUNT=10&I=50&J=50&EXCEPTIONS=XML"));
        let old = FeatureInfo { version: "1.1.1".into(), formats: vec!["text/html".into(), "application/vnd.ogc.gml".into()], ..roads };
        let r = old.request(0.0, 0.0, 1.0).unwrap();
        assert!(r.contains("&SRS=EPSG:3857&"));
        assert!(r.contains("&INFO_FORMAT=application%2Fvnd.ogc.gml&"));
        assert!(r.contains("&X=50&Y=50&EXCEPTIONS=application%2Fvnd.ogc.se_xml"));
        assert_eq!(old.request(f64::NAN, 0.0, 1.0), None);
        assert_eq!(old.request(0.0, 0.0, 0.0), None);
    }

    #[test]
    fn without_a_get_feature_info_request_layers_have_none() {
        let caps = CAPS_13.replace("GetFeatureInfo", "GetLegendGraphic");
        let service = crate::ogc::parse("https://maps.example.org/wms", caps.as_bytes()).unwrap();
        assert!(service.layers.iter().all(|l| l.feature_info.is_none()));
    }

    #[test]
    fn a_template_gives_its_service_and_layer() {
        let t = "https://maps.example.org/wms?map=x&SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=ch.swisstopo%3Aroads&STYLES=&CRS=EPSG:3857&BBOX={bbox-epsg-3857}&WIDTH=256&HEIGHT=256&FORMAT=image%2Fpng&TRANSPARENT=TRUE";
        let (caps, layer) = wms_layer_of(t).unwrap();
        assert_eq!(caps, "https://maps.example.org/wms?map=x&SERVICE=WMS&REQUEST=GetCapabilities&VERSION=1.3.0");
        assert_eq!(layer, "ch.swisstopo:roads");
        assert_eq!(wms_layer_of("https://tile.example.org/{z}/{x}/{y}.png"), None);
    }

    #[test]
    fn geojson_answers() {
        let body = br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","id":"roads.7","properties":{"name":"Main St","lanes":2,"note":null},"geometry":null},
            {"type":"Feature","layerBodId":"ch.bfs.gebaeude","properties":{"egid":123},"geometry":null}]}"#;
        let f = read_feature_info(body).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].layer.as_deref(), Some("roads"));
        assert_eq!(f[0].attributes, vec![("lanes".into(), "2".into()), ("name".into(), "Main St".into()), ("note".into(), String::new())]);
        assert_eq!(f[1].layer.as_deref(), Some("ch.bfs.gebaeude"));
        assert!(read_feature_info(br#"{"type":"FeatureCollection","features":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn gml_answers() {
        let gml = br#"<?xml version="1.0"?>
<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs" xmlns:gml="http://www.opengis.net/gml" xmlns:t="urn:t">
  <gml:featureMember><t:roads fid="roads.1"><gml:boundedBy><gml:Box/></gml:boundedBy><t:name>Main St</t:name>
    <t:geom><gml:Point><gml:coordinates>1,2</gml:coordinates></gml:Point></t:geom><t:lanes>2</t:lanes></t:roads></gml:featureMember>
</wfs:FeatureCollection>"#;
        let f = read_feature_info(gml).unwrap();
        assert_eq!(
            f,
            vec![InfoFeature {
                layer: Some("roads".into()),
                attributes: vec![("name".into(), "Main St".into()), ("lanes".into(), "2".into())]
            }]
        );
        // A feature's attribute named `member` is no member of the collection.
        let wfs2 = br#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:t="urn:t">
  <wfs:member><t:clubs><t:name>Chess</t:name><t:member>Ann</t:member></t:clubs></wfs:member></wfs:FeatureCollection>"#;
        let f = read_feature_info(wfs2).unwrap();
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].layer.as_deref(), Some("clubs"));
        let ms = br#"<msGMLOutput xmlns:gml="http://www.opengis.net/gml"><parks_layer><gml:name>parks</gml:name>
            <parks_feature><gml:boundedBy/><NAME>Central</NAME></parks_feature></parks_layer></msGMLOutput>"#;
        let f = read_feature_info(ms).unwrap();
        assert_eq!(f, vec![InfoFeature { layer: Some("parks".into()), attributes: vec![("NAME".into(), "Central".into())] }]);
        let esri = br#"<FeatureInfoResponse><FIELDS OBJECTID="4" NAME="Lake"/></FeatureInfoResponse>"#;
        assert_eq!(read_feature_info(esri).unwrap()[0].attributes, vec![("OBJECTID".into(), "4".into()), ("NAME".into(), "Lake".into())]);
    }

    #[test]
    fn exceptions_are_errors() {
        let e = br#"<ServiceExceptionReport version="1.3.0"><ServiceException code="LayerNotQueryable">Layer roads is not queryable</ServiceException></ServiceExceptionReport>"#;
        let err = read_feature_info(e).unwrap_err();
        assert!(err.contains("not queryable"), "{err}");
        assert!(read_feature_info(&[0x89, b'P', b'N', b'G', 0xff, 0xfe]).is_err());
    }

    #[test]
    fn text_and_html_answers() {
        let geoserver = "Results for FeatureType 'roads':\n--------------------------------------------\nname = Main St\nlanes = 2\n--------------------------------------------\nname = Side St\nlanes = 1\n";
        let f = read_feature_info(geoserver.as_bytes()).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].layer.as_deref(), Some("roads"));
        assert_eq!(f[1].attributes, vec![("name".into(), "Side St".into()), ("lanes".into(), "1".into())]);
        let named = "Layer 'roads'\n  Feature 3: \n    Feature class = 'Road'\n    Layer name = 'base'\n";
        let f = read_feature_info(named.as_bytes()).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].attributes, vec![("Feature class".into(), "Road".into()), ("Layer name".into(), "base".into())]);
        let mapserver = "GetFeatureInfo results:\n\nLayer 'parks'\n  Feature 12: \n    NAME = 'Central'\n";
        assert_eq!(read_feature_info(mapserver.as_bytes()).unwrap()[0].attributes, vec![("NAME".into(), "Central".into())]);
        let html = "<html><head><style>p{}</style><script>x()</script></head><body><p>Lake &amp; shore</p></body></html>";
        assert_eq!(read_feature_info(html.as_bytes()).unwrap()[0].attributes, vec![("text".into(), "Lake & shore".into())]);
        // Any case of script and style is left out; many short scripts read in linear time.
        let shouting = "<HTML><SCRIPT>x()</Script><Style>p{}</STYLE><B>Peak</B></HTML>";
        assert_eq!(html_text(shouting).split_whitespace().collect::<Vec<_>>(), ["Peak"]);
        let many = format!("<html>{}<p>end</p>", "<script></script>".repeat(200_000));
        assert_eq!(html_text(&many).split_whitespace().collect::<Vec<_>>(), ["end"]);
        assert!(read_feature_info(b"   ").unwrap().is_empty());
    }

    #[test]
    fn hostile_answers_never_panic() {
        for body in [
            &b"{"[..],
            b"<",
            b"<a><b>",
            b"Layer '",
            b"x = ",
            b"[1,2]",
            b"{\"features\":7}",
            b"<FeatureInfoResponse><FeatureInfo><Field/></FeatureInfo></FeatureInfoResponse>",
        ] {
            let _ = read_feature_info(body);
        }
        let _ = wms_layer_of("http://x?service=wms&request=getmap&layers=%");
    }
}
