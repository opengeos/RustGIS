//! The RustGIS documentation (`docs/`), built in, so an agent can look up how RustGIS works
//! without a network: search it by words, then read a page or one of its sections.

use serde_json::{Value, json};

/// Where the published documentation lives.
pub const SITE: &str = "https://rustgis.app/";

/// One documentation page: its path under `docs/` and its Markdown.
pub struct Page {
    pub path: &'static str,
    pub text: &'static str,
}

macro_rules! pages {
    ($($path:literal),* $(,)?) => {
        &[$(Page { path: $path, text: include_str!(concat!("../../../docs/", $path)) }),*]
    };
}

/// Every page, in the site's order.
pub const PAGES: &[Page] = pages![
    "index.md",
    "installation.md",
    "getting-started.md",
    "user-guide/index.md",
    "user-guide/data.md",
    "user-guide/maps.md",
    "user-guide/cartography.md",
    "user-guide/editing.md",
    "user-guide/analysis.md",
    "user-guide/layouts.md",
    "user-guide/sharing.md",
    "user-guide/workspace.md",
    "user-guide/assistant.md",
    "comparison.md",
    "cli.md",
    "mcp.md",
    "plugins.md",
    "i18n.md",
    "architecture.md",
    "contributing.md",
    "sponsor.md",
];

/// The longest page or section handed back (characters).
pub const MAX_READ: usize = 24_000;

impl Page {
    /// The page's first heading.
    pub fn title(&self) -> &'static str {
        sections(self.text).into_iter().find_map(|(h, _)| (!h.is_empty()).then_some(h)).unwrap_or(self.path)
    }

    /// The page on the published site.
    pub fn url(&self) -> String {
        let p = self.path.trim_end_matches(".md");
        let p = p.strip_suffix("index").unwrap_or(p);
        if p.is_empty() || p.ends_with('/') { format!("{SITE}{p}") } else { format!("{SITE}{p}/") }
    }
}

/// A page by its path (`cli.md`, `user-guide/maps`, `/user-guide/maps.md`, or its site URL).
pub fn page(path: &str) -> Option<&'static Page> {
    let p = path.trim().trim_start_matches(SITE).trim_start_matches("rustgis://docs/").trim_matches('/');
    let p = p.strip_prefix("docs/").unwrap_or(p);
    let with_md = if p.ends_with(".md") {
        p.to_string()
    } else if p.is_empty() {
        "index.md".into()
    } else {
        format!("{p}.md")
    };
    PAGES.iter().find(|pg| pg.path.eq_ignore_ascii_case(&with_md) || pg.path.eq_ignore_ascii_case(&format!("{p}/index.md")))
}

/// A page's sections: (heading, body), the text before the first heading under an empty one.
/// Lines in code blocks never start a section.
pub fn sections(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut heading = "";
    let mut start = 0;
    let mut fenced = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") {
            fenced = !fenced;
        } else if !fenced && t.starts_with('#') {
            let h = t.trim_start_matches('#');
            if h.starts_with(' ') {
                out.push((heading, text.get(start..at).unwrap_or("")));
                heading = h.trim();
                start = at + line.len();
            }
        }
        at += line.len();
    }
    out.push((heading, text.get(start..).unwrap_or("")));
    out.retain(|(h, b)| !h.is_empty() || !b.trim().is_empty());
    out
}

/// The anchor the site gives a heading.
pub fn slug(heading: &str) -> String {
    let mut s = String::new();
    for c in heading.trim().to_lowercase().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            s.push(c);
        } else if c.is_whitespace() && !s.ends_with('-') {
            s.push('-');
        }
    }
    s.trim_matches('-').to_string()
}

/// The query's words, lower case, of two or more letters.
fn words(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        // One plural `s` ("labels" finds "label"); "class" and "address" stay whole.
        .map(|w| if w.chars().count() > 3 && !w.ends_with("ss") { w.strip_suffix('s').unwrap_or(w) } else { w })
        .map(str::to_string)
        .take(12)
        .collect()
}

/// The sections that best match `query`: page, heading, address and a snippet around the first
/// match.
pub fn search(query: &str, limit: usize) -> Result<Value, String> {
    let words = words(query);
    if words.is_empty() {
        return Err("query needs at least one word of two or more letters, e.g. \"label field\"".into());
    }
    let mut hits: Vec<(usize, Value)> = Vec::new();
    for pg in PAGES {
        let title = pg.title();
        for (heading, body) in sections(pg.text) {
            let (h, b) = (heading.to_lowercase(), body.to_lowercase());
            let mut score = 0;
            let mut matched = 0;
            for w in &words {
                let in_heading = h.contains(w.as_str());
                let in_body = b.matches(w.as_str()).count().min(5);
                if in_heading || in_body > 0 {
                    matched += 1;
                }
                score += if in_heading { 6 } else { 0 } + in_body;
            }
            if matched == 0 {
                continue;
            }
            // Sections holding every word come first.
            let score = score + if matched == words.len() { 20 } else { 0 };
            let url = if heading.is_empty() || heading == title { pg.url() } else { format!("{}#{}", pg.url(), slug(heading)) };
            hits.push((score, json!({"page": pg.path, "title": title, "section": heading, "url": url, "snippet": snippet(body, &words)})));
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    let total = hits.len();
    Ok(json!({"matches": total, "results": hits.into_iter().take(limit.clamp(1, 30)).map(|h| h.1).collect::<Vec<_>>()}))
}

/// About 300 characters of `body` around the first of `words` in it.
fn snippet(body: &str, words: &[String]) -> String {
    // Images say nothing an agent can use.
    let body: String = body.split_inclusive('\n').filter(|l| !l.trim_start().starts_with("![")).collect();
    let chars: Vec<char> = body.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let lower: String = lower.into_iter().collect();
    let first = words.iter().filter_map(|w| lower.find(w.as_str())).min().unwrap_or(0);
    // `find` gives a byte offset into `lower`; count the characters before it.
    let at = lower.get(..first).map_or(0, |p| p.chars().count());
    let start = at.saturating_sub(80);
    let text: String = chars.iter().skip(start).take(300).collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&text);
    if start + 300 < chars.len() {
        out.push('…');
    }
    out
}

/// A page, or one section of it (by heading, any case), as Markdown.
pub fn read(path: &str, section: Option<&str>) -> Result<Value, String> {
    let pg = page(path).ok_or_else(|| {
        let all: Vec<&str> = PAGES.iter().map(|p| p.path).collect();
        format!("no page '{path}' (one of {})", all.join(", "))
    })?;
    let (text, url) = match section.map(str::trim).filter(|s| !s.is_empty()) {
        None => (pg.text, pg.url()),
        Some(want) => {
            let want_slug = slug(want);
            let found =
                sections(pg.text).into_iter().find(|(h, _)| h.eq_ignore_ascii_case(want) || slug(h) == want_slug).ok_or_else(|| {
                    let hs: Vec<&str> = sections(pg.text).into_iter().map(|(h, _)| h).filter(|h| !h.is_empty()).collect();
                    format!("{} has no section '{want}' (its sections: {})", pg.path, hs.join("; "))
                })?;
            (found.1, format!("{}#{}", pg.url(), slug(found.0)))
        }
    };
    Ok(json!({"page": pg.path, "title": pg.title(), "url": url, "markdown": crate::tools::clip(text.trim(), MAX_READ)}))
}
