//! XML parsed safely: every reader of XML (GPX, KML, GML, CityGML, GeoRSS, LandXML, OGC
//! capabilities, S3 listings, QGIS projects and styles) goes through [`parse`], which refuses a
//! document nested deeper than [`MAX_DEPTH`], or whose namespaces would take more than
//! [`NAMESPACE_WORK`] times its size to resolve, before parsing it. Nesting thousands of elements
//! deep exhausts the stack while the document is parsed or walked, and crafted namespace
//! declarations (or attributes) make the parse grow as their number times the elements they stand
//! around; a hostile file must be an error, never a crash.

use std::collections::HashMap;

use roxmltree::{Document, ParsingOptions};

/// How deep elements may nest. Real files stay far below it (KML folders, GML geometry and WMS
/// layer trees are a few dozen deep at most).
pub const MAX_DEPTH: usize = 256;

/// How many steps of namespace resolution a document may take per byte of its text. The parser
/// (roxmltree) looks each element's prefix and each attribute's up among the namespaces in scope one
/// by one, checks each attribute against the element's others, and, for an element that declares
/// namespaces, checks each declaration against the others and copies each namespace in scope in,
/// checking it against all it holds so far. A real file (a few dozen namespaces on its root, an
/// element every few dozen bytes) takes under one step a byte; one declaring hundreds of namespaces
/// around many elements, thousands.
pub const NAMESPACE_WORK: u64 = 64;

/// The namespace resolution any document may take, however short: a root declaring several hundred
/// namespaces is no threat.
const MIN_NAMESPACE_WORK: u64 = 1 << 20;

/// How many namespaces a document's elements may copy in, per byte of its text: each copy is kept
/// in the parsed document (two bytes), so this bounds the memory they take to a few times the
/// text's size, about what its elements take parsed.
const NAMESPACE_COPIES: u64 = 4;

/// Parses `text` (a leading byte-order mark dropped) with `opts`, once [`check_limits`] passes.
pub fn parse<'a>(text: &'a str, opts: ParsingOptions<'a>) -> Result<Document<'a>, String> {
    let text = text.trim_start_matches('\u{feff}');
    check_limits(text, opts.allow_dtd)?;
    Document::parse_with_options(text, opts).map_err(|e| e.to_string())
}

/// Refuses XML nested deeper than [`MAX_DEPTH`], or whose namespaces would take more than
/// [`NAMESPACE_WORK`] steps a byte to resolve (or copy in more than [`NAMESPACE_COPIES`] a byte),
/// by a scan that never recurses. It counts what opens and closes an element, skipping comments,
/// CDATA, processing instructions and quoted attribute values, and the prefixes in scope as the
/// parser does (one declared again is the same prefix); where it can't tell, it counts too deep
/// rather than too shallow, and too much work rather than too little (an attribute with no prefix
/// counts as looked up). With a DTD allowed, an entity whose value holds markup is refused too:
/// expanded, it would nest in ways the text doesn't show.
pub fn check_limits(text: &str, allow_dtd: bool) -> Result<(), String> {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0usize, 0usize);
    // The prefixes each open element declares, how many declarations of each prefix are open (the
    // prefixes in scope), and the steps resolving them takes so far, up to `limit`; in 64 bits, so
    // the browser's 32-bit sizes can't saturate the limit away.
    let mut declared: Vec<Vec<&[u8]>> = Vec::new();
    let mut prefixes: HashMap<&[u8], usize> = HashMap::new();
    let (mut work, mut copies) = (0u64, 0u64);
    let limit = (text.len() as u64).saturating_mul(NAMESPACE_WORK).max(MIN_NAMESPACE_WORK);
    let copy_limit = (text.len() as u64).saturating_mul(NAMESPACE_COPIES).max(MIN_NAMESPACE_WORK);
    let too_much = || "the document's namespace declarations would take more to resolve than its size allows".to_string();
    let find = |from: usize, pat: &[u8]| -> Option<usize> { b.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| from + p) };
    while i < b.len() {
        if b.get(i) != Some(&b'<') {
            i += 1;
            continue;
        }
        let rest = b.get(i..).unwrap_or(&[]);
        if rest.starts_with(b"<!--") {
            match find(i + 4, b"-->") {
                Some(e) => i = e + 3,
                None => return Ok(()),
            }
        } else if rest.starts_with(b"<![CDATA[") {
            match find(i + 9, b"]]>") {
                Some(e) => i = e + 3,
                None => return Ok(()),
            }
        } else if rest.starts_with(b"<?") {
            match find(i + 2, b"?>") {
                Some(e) => i = e + 2,
                None => return Ok(()),
            }
        } else if rest.starts_with(b"<!") {
            // A DOCTYPE (with any internal subset): to its closing `>`, `[ … ]` and quotes skipped.
            // Comments and processing instructions in the subset are skipped whole (an apostrophe in
            // one isn't a quote). A declaration that never ends is refused: the rest of the
            // document would go uncounted.
            let mut j = i + 2;
            let (mut bracket, mut quote) = (0usize, None::<u8>);
            let mut closed = false;
            while let Some(&c) = b.get(j) {
                if quote.is_none() && b.get(j..).is_some_and(|r| r.starts_with(b"<!--")) {
                    j = find(j + 4, b"-->").map(|e| e + 3).ok_or("the document's DOCTYPE has an unclosed comment")?;
                    continue;
                }
                if quote.is_none() && b.get(j..).is_some_and(|r| r.starts_with(b"<?")) {
                    j = find(j + 2, b"?>").map(|e| e + 2).ok_or("the document's DOCTYPE has an unclosed instruction")?;
                    continue;
                }
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'[') => bracket += 1,
                    (None, b']') => bracket = bracket.saturating_sub(1),
                    (None, b'>') if bracket == 0 => {
                        closed = true;
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            if !closed {
                return Err("the document's DOCTYPE never ends".into());
            }
            let decl = b.get(i..j).unwrap_or(&[]);
            if allow_dtd && entity_with_markup(decl) {
                return Err("the document declares entities holding markup, which aren't read".into());
            }
            i = j + 1;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            // Its name is compared with its start tag's; the prefixes its element declared go out of
            // scope.
            for prefix in declared.pop().unwrap_or_default() {
                close(&mut prefixes, prefix);
            }
            i = find(i + 2, b">").map(|e| e + 1).unwrap_or(b.len());
        } else {
            // An element's start tag: to its `>` (quoted values skipped); `/>` closes it at once.
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(format!("the document nests elements more than {MAX_DEPTH} deep"));
            }
            let mut j = i + 1;
            let mut quote = None::<u8>;
            // Its attributes, by the name before each `=`: the prefixes it declares (`""` the
            // default namespace) and the others.
            let (mut own, mut attributes) = (Vec::<&[u8]>::new(), 0u64);
            let (mut word, mut name) = (None::<usize>, (0usize, 0usize));
            while let Some(&c) = b.get(j) {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'>') => break,
                    (None, b'=') => {
                        if let Some(s) = word.take() {
                            name = (s, j);
                        }
                        let key = b.get(name.0..name.1).unwrap_or(&[]);
                        if key == b"xmlns" {
                            own.push(b"");
                        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                            own.push(prefix);
                        } else {
                            attributes += 1;
                        }
                    }
                    (None, c) if c.is_ascii_whitespace() || c == b'/' => {
                        if let Some(s) = word.take() {
                            name = (s, j);
                        }
                    }
                    (None, _) => {
                        word.get_or_insert(j);
                    }
                }
                j += 1;
            }
            // The namespaces in scope around it and in it, the `xml` one among them.
            let around = prefixes.len() as u64 + 1;
            for prefix in &own {
                *prefixes.entry(prefix).or_default() += 1;
            }
            let scope = prefixes.len() as u64 + 1;
            let mine = own.len() as u64;
            // Its name and each attribute's prefix are looked up among the namespaces in scope, and
            // each attribute is checked against the others.
            let mut cost = scope.saturating_mul(attributes.saturating_add(1)).saturating_add(attributes.saturating_mul(attributes));
            if mine > 0 {
                // Each declaration is checked against the others; each namespace around it is copied
                // in, checked against its own and those copied before it.
                let copying = around.saturating_mul(mine).saturating_add(around.saturating_mul(around.saturating_sub(1)) / 2);
                cost = cost.saturating_add(mine.saturating_mul(mine)).saturating_add(copying);
                copies = copies.saturating_add(around);
            }
            work = work.saturating_add(cost);
            if work > limit || copies > copy_limit {
                return Err(too_much());
            }
            if j > 0 && b.get(j - 1) == Some(&b'/') {
                depth = depth.saturating_sub(1);
                for prefix in own {
                    close(&mut prefixes, prefix);
                }
            } else {
                declared.push(own);
            }
            i = j + 1;
        }
    }
    Ok(())
}

/// One declaration of `prefix` goes out of scope.
fn close(prefixes: &mut HashMap<&[u8], usize>, prefix: &[u8]) {
    if let Some(open) = prefixes.get_mut(prefix) {
        *open = open.saturating_sub(1);
        if *open == 0 {
            prefixes.remove(prefix);
        }
    }
}

/// Whether a DOCTYPE declares an entity whose value holds markup (a `<`).
fn entity_with_markup(decl: &[u8]) -> bool {
    let mut at = 0;
    while let Some(p) = decl.get(at..).and_then(|r| r.windows(8).position(|w| w == b"<!ENTITY")) {
        let start = at + p + 8;
        // The declaration runs to the first `>` outside quotes (a quoted value may hold `>`); a
        // `<` anywhere in it, quoted or not, is markup.
        let mut quote = None::<u8>;
        let mut end = decl.len();
        for (k, &c) in decl.get(start..).unwrap_or(&[]).iter().enumerate() {
            match (quote, c) {
                (_, b'<') => return true,
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') => quote = Some(c),
                (None, b'>') => {
                    end = start + k;
                    break;
                }
                _ => {}
            }
        }
        at = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(n: usize) -> String {
        format!("<?xml version=\"1.0\"?><r>{}{}</r>", "<a>".repeat(n), "</a>".repeat(n))
    }

    #[test]
    fn depth_is_bounded() {
        assert!(check_limits(&nested(200), false).is_ok());
        assert!(check_limits(&nested(10_000), false).is_err());
        // Self-closed, commented, CDATA and quoted `<`s don't nest.
        let flat = format!(
            "<r>{}<!-- {} --><![CDATA[{}]]><a v=\"{}\"/></r>",
            "<a/>".repeat(1000),
            "<a>".repeat(1000),
            "<a>".repeat(1000),
            "<a>".repeat(300)
        );
        assert!(check_limits(&flat, false).is_ok());
        // Unclosed opens count (a truncated file never reads as shallow).
        assert!(check_limits(&"<a>".repeat(300), false).is_err());
        let (deep, shallow) = (nested(10_000), nested(10));
        assert!(parse(&deep, ParsingOptions::default()).is_err());
        assert!(parse(&shallow, ParsingOptions::default()).is_ok());
        // An entity holding markup, with a DTD allowed: refused; plain ones are fine.
        let ent = "<!DOCTYPE r [<!ENTITY a \"<b><b>&a;</b></b>\">]><r>&a;</r>";
        assert!(check_limits(ent, true).is_err());
        // A `>` in the quoted value before its markup doesn't hide the markup.
        for hidden in ["<!DOCTYPE r [<!ENTITY a \"x>&b;<c>\">]><r/>", "<!DOCTYPE r [<!ENTITY a 'x>y<c>'>]><r/>"] {
            assert!(check_limits(hidden, true).is_err(), "{hidden}");
        }
        // A comment or instruction in the subset with an apostrophe doesn't hide the document
        // after it: its nesting is still counted.
        for subset in ["<!-- it's -->", "<?pi it's?>"] {
            let doc = format!("<!DOCTYPE r [ {subset} ]>{}", nested(10_000));
            assert!(check_limits(&doc, true).is_err() && check_limits(&doc, false).is_err(), "{subset}");
            let ok = format!("<!DOCTYPE r [ {subset} ]><r/>");
            assert!(check_limits(&ok, true).is_ok(), "{subset}");
        }
        // A DOCTYPE that never ends is refused rather than skipping the rest.
        assert!(check_limits(&format!("<!DOCTYPE r [ '{}", nested(10)), true).is_err());
        let plain =
            "<!DOCTYPE WMT_MS_Capabilities SYSTEM \"http://x/WMS_MS_Capabilities.dtd\" [<!ELEMENT VendorSpecificCapabilities EMPTY>]><r/>";
        assert!(check_limits(plain, true).is_ok());
    }

    /// `n` namespace declarations, `n0` on.
    fn declarations(n: usize) -> String {
        (0..n).map(|k| format!(r#" xmlns:n{k}="urn:{k}""#)).collect()
    }

    /// Namespaces declared around many elements make resolving them grow as their product: each
    /// way of building that is refused before parsing, while real documents pass.
    #[test]
    fn namespace_work_is_bounded() {
        // Each is refused by its own term: the root's own declarations are few enough to pass.
        // Copied: a few hundred namespaces around children that each declare one, so each copies
        // them all in, checking each against those copied before it.
        let copied = format!("<r{}>{}</r>", declarations(300), r#"<a xmlns:x="urn:x"/>"#.repeat(5000));
        // Checked against each other: one element declaring many.
        let one = format!("<r{}/>", declarations(20_000));
        // Looked up: a root declaring many, around many prefixed elements declaring none.
        let looked_up = format!("<r{}>{}</r>", declarations(1000), "<n0:a/>".repeat(20_000));
        // Attributes checked against each other: one element with many.
        let attributes: String = (0..20_000).map(|k| format!(" a{k}=''")).collect();
        let attributed = format!("<r{attributes}/>");
        for doc in [&copied, &one, &looked_up, &attributed] {
            let refused = check_limits(doc, false).unwrap_err();
            assert!(refused.contains("namespace declarations"), "{refused}");
            assert!(parse(doc, ParsingOptions::default()).is_err());
        }
        // A real document: a few dozen namespaces on its root, small elements with prefixed
        // attributes, a prefix declared again inside.
        let real = format!(
            "<r{}>{}</r>",
            declarations(40),
            r#"<n1:a n2:b="1"><n1:c xmlns:n1="urn:1" n3:d='x = "y"'>2</n1:c></n1:a>"#.repeat(20_000)
        );
        assert!(check_limits(&real, false).is_ok());
        assert!(parse(&real, ParsingOptions::default()).is_ok());
        // As many children declaring one, with a few dozen namespaces around them (as a service
        // writing its namespaces on every feature does).
        let few = format!("<r{}>{}</r>", declarations(40), r#"<a xmlns:x="urn:x"/>"#.repeat(5000));
        assert!(check_limits(&few, false).is_ok());
        // A root declaring a few hundred, in a short document.
        assert!(check_limits(&format!("<r{}/>", declarations(500)), false).is_ok());
    }

    /// Every XML reader refuses a document whose namespaces would take too long to resolve.
    #[test]
    fn every_reader_refuses_namespace_floods() {
        let flood = |root: &str| format!("<?xml version=\"1.0\"?><{root}{}/>", declarations(20_000)).into_bytes();
        let said = |e: String| e.contains("namespace declarations");
        assert!(crate::gpx::read("a.gpx", &flood("gpx")).is_err_and(|e| said(e.to_string())));
        assert!(crate::kml::read("a.kml", &flood("kml")).is_err_and(|e| said(e.to_string())));
        assert!(crate::gml::read("a.gml", &flood("FeatureCollection")).is_err_and(|e| said(e.to_string())));
        assert!(crate::georss::read("a.rss", &flood("rss")).is_err_and(|e| said(e.to_string())));
        assert!(crate::landxml::read("a.xml", &flood("LandXML"), None).is_err_and(|e| said(e.to_string())));
        assert!(crate::ogc::parse("https://x.example/wms", &flood("WMS_Capabilities")).is_err_and(|e| said(e.to_string())));
    }

    /// Every XML reader refuses a deeply nested document instead of overflowing the stack, run on
    /// a small one (debug builds use the most stack per frame).
    #[test]
    fn every_reader_refuses_deep_nesting() {
        let r = std::thread::Builder::new()
            .stack_size(512 << 10)
            .spawn(|| {
                let deep = |root: &str, inner: &str| {
                    format!(
                        "<?xml version=\"1.0\"?><{root}>{}{}</{root}>",
                        format!("<{inner}>").repeat(10_000),
                        format!("</{inner}>").repeat(10_000)
                    )
                    .into_bytes()
                };
                assert!(crate::gpx::read("a.gpx", &deep("gpx", "trk")).is_err());
                assert!(crate::kml::read("a.kml", &deep("kml", "Folder")).is_err());
                assert!(crate::kml::ground_overlays("a.kml", &deep("kml", "Folder")).is_err());
                assert!(crate::gml::read("a.gml", &deep("wfs:FeatureCollection", "gml:featureMember")).is_err());
                assert!(crate::georss::read("a.rss", &deep("rss", "channel")).is_err());
                assert!(crate::landxml::read("a.xml", &deep("LandXML", "Parcels"), None).is_err());
                assert!(crate::ogc::parse("https://x.example/wms", &deep("WMS_Capabilities", "Layer")).is_err());
                true
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(r);
    }

    /// A deep document is refused on a small stack (as a thread of the app's might have), never
    /// overflowing it.
    #[test]
    fn deep_documents_never_overflow_a_small_stack() {
        let text = nested(100_000);
        let r = std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(move || parse(&text, ParsingOptions::default()).is_err())
            .unwrap()
            .join()
            .unwrap();
        assert!(r);
    }
}
