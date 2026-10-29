//! Session-only connection logins, scoped to one exact HTTP(S) origin.
//!
//! Callers keep the original URL in caches and projects, and use [`apply`] only when preparing
//! a request. Applied URLs and headers contain secrets: never log them; use [`scrub`] for
//! diagnostics. Activation replaces the origin's previous login, while only its current owner
//! can deactivate it. Plain HTTP requires an explicit insecure opt-in.

use std::{
    collections::BTreeMap,
    fmt,
    net::Ipv6Addr,
    sync::{Mutex, MutexGuard, OnceLock},
};

use base64::Engine;
use rustgis_data::secrets::Secret;

/// Maximum header or query parameter name length, in bytes.
pub const MAX_NAME: usize = 64;
/// Maximum credential length, in bytes.
pub const MAX_VALUE: usize = 4096;

/// Canonical HTTP(S) origin: lowercase scheme and host, without a trailing host dot or
/// the scheme's default port. Paths, queries and fragments do not form part of the origin.
/// User information, whitespace, controls, backslashes and malformed authorities are refused.
pub fn origin(url: &str) -> Option<String> {
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = if scheme.eq_ignore_ascii_case("https") {
        "https"
    } else if scheme.eq_ignore_ascii_case("http") {
        "http"
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(ip) = authority.strip_prefix('[') {
        let (ip, tail) = ip.split_once(']')?;
        let host = format!("[{}]", ip.parse::<Ipv6Addr>().ok()?);
        let port = if tail.is_empty() { None } else { Some(tail.strip_prefix(':')?) };
        (host, port)
    } else {
        let (host, port) = authority.split_once(':').map_or((authority, None), |(h, p)| (h, Some(p)));
        let host = host.strip_suffix('.').unwrap_or(host);
        if host.is_empty() || host.contains(['[', ']', '%', '<', '>', '"', '^', '`', '{', '}', '|']) || host.split('.').any(str::is_empty) {
            return None;
        }
        (host.to_lowercase(), port)
    };
    let port = match port {
        Some(port) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some(port.parse::<u16>().ok()?)
        }
        None => None,
    };
    match port.filter(|p| *p != if scheme == "https" { 443 } else { 80 }) {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

/// An RFC 7230 token, excluding transport, browser-managed and proxy/security headers.
pub fn valid_header_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
        return false;
    }
    let blocked = [
        "host",
        "connection",
        "content-length",
        "transfer-encoding",
        "te",
        "upgrade",
        "keep-alive",
        "range",
        "accept-encoding",
        "cookie",
        "origin",
        "referer",
        "user-agent",
    ];
    !blocked.iter().any(|s| name.eq_ignore_ascii_case(s))
        && !name.get(..6).is_some_and(|s| s.eq_ignore_ascii_case("proxy-"))
        && !name.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("sec-"))
}

/// A nonempty ASCII unreserved query parameter name.
pub fn valid_query_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(unreserved)
}

/// A nonempty credential of at most [`MAX_VALUE`] bytes, without control characters.
pub fn valid_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_VALUE && !value.chars().any(char::is_control)
}

/// An HTTP Basic authorization value, using standard padded Base64.
pub fn basic(user: &str, password: &str) -> String {
    let input = format!("{user}:{password}");
    let mut result = String::with_capacity(6 + input.len().div_ceil(3) * 4);
    result.push_str("Basic ");
    base64::engine::general_purpose::STANDARD.encode_string(input, &mut result);
    result
}

/// How a connection credential is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Header(String),
    Query(String),
}

// Deliberately no Debug: neither credentials nor their derived redaction patterns are printable.
struct Entry {
    owner: String,
    method: Method,
    value: Secret,
    redactions: Vec<Redaction>,
}

struct Redaction {
    value: Secret,
    encoded: bool,
}

fn entries() -> MutexGuard<'static, BTreeMap<String, Entry>> {
    static ENTRIES: OnceLock<Mutex<BTreeMap<String, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Installs or replaces an origin's session-only login. The origin must already be canonical;
/// an HTTP origin is accepted only with `insecure`. Errors are fixed, secret-free sentences.
pub fn activate(origin: &str, owner: &str, method: Method, value: &str, insecure: bool) -> Result<(), &'static str> {
    if self::origin(origin).as_deref() != Some(origin) {
        return Err("A login requires a canonical HTTP or HTTPS origin.");
    }
    if origin.starts_with("http://") && !insecure {
        return Err("An HTTP login requires explicit permission to send credentials insecurely.");
    }
    let valid_name = match &method {
        Method::Header(name) => valid_header_name(name),
        Method::Query(name) => valid_query_name(name),
    };
    if !valid_name {
        return Err("The login header or query parameter name is not allowed.");
    }
    if !valid_value(value) {
        return Err("The login credential must be nonempty, within the size limit, and contain no controls.");
    }
    let mut redactions = Vec::with_capacity(4);
    add_redactions(&mut redactions, value);
    if let Some(payload) = value.get(..6).filter(|s| s.eq_ignore_ascii_case("Basic ")).and_then(|_| value.get(6..))
        && !payload.is_empty()
    {
        add_redactions(&mut redactions, payload);
    }
    entries().insert(origin.to_string(), Entry { owner: owner.to_string(), method, value: Secret::new(value), redactions });
    Ok(())
}

fn add_redactions(redactions: &mut Vec<Redaction>, value: &str) {
    redactions.push(Redaction { value: Secret::new(value), encoded: false });
    let encoded = percent_encode(value);
    if encoded != value {
        redactions.push(Redaction { value: Secret::new(encoded), encoded: true });
    }
}

/// Removes a login only if it is still owned by this connection.
pub fn deactivate(origin: &str, owner: &str) -> bool {
    let mut entries = entries();
    if entries.get(origin).is_some_and(|entry| entry.owner == owner) {
        entries.remove(origin);
        true
    } else {
        false
    }
}

/// The connection currently owning this canonical origin's login.
pub fn owner(origin: &str) -> Option<String> {
    entries().get(origin).map(|entry| entry.owner.clone())
}

/// Lowercase, sorted, distinct query names of currently active logins.
pub fn query_names() -> Vec<String> {
    let mut names: Vec<_> = entries()
        .values()
        .filter_map(|entry| match &entry.method {
            Method::Query(name) => Some(name.to_ascii_lowercase()),
            Method::Header(_) => None,
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// A request-only URL and optional header. Debug never reveals either field.
pub struct Applied {
    pub url: String,
    pub header: Option<(String, String)>,
}

impl fmt::Debug for Applied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Applied(<redacted>)")
    }
}

/// Applies a login only to its exact canonical origin (scheme, host and effective port).
/// Existing query names are compared case-insensitively, including percent-escaped names;
/// a credential is not appended when the URL already supplies that parameter.
pub fn apply(url: &str) -> Option<Applied> {
    let origin = origin(url)?;
    let entries = entries();
    let entry = entries.get(&origin)?;
    match &entry.method {
        Method::Header(name) => Some(Applied { url: url.to_string(), header: Some((name.clone(), entry.value.expose().to_string())) }),
        Method::Query(name) => {
            let (address, fragment) = url.split_once('#').map_or((url, None), |(address, fragment)| (address, Some(fragment)));
            if address.split_once('?').is_some_and(|(_, query)| {
                query.split('&').any(|parameter| query_name_matches(parameter.split('=').next().unwrap_or_default(), name))
            }) {
                return Some(Applied { url: url.to_string(), header: None });
            }
            let value = entry.value.expose();
            let encoded_len: usize = value.bytes().map(|b| if unreserved(b) { 1 } else { 3 }).sum();
            let mut applied = String::with_capacity(url.len() + name.len() + encoded_len + 2);
            applied.push_str(address);
            if !address.contains('?') {
                applied.push('?');
            } else if !address.ends_with(['?', '&']) {
                applied.push('&');
            }
            applied.push_str(name);
            applied.push('=');
            append_percent_encoded(&mut applied, value);
            if let Some(fragment) = fragment {
                applied.push('#');
                applied.push_str(fragment);
            }
            Some(Applied { url: applied, header: None })
        }
    }
}

fn query_name_matches(encoded: &str, name: &str) -> bool {
    let mut bytes = encoded.bytes();
    for expected in name.bytes() {
        let Some(mut actual) = bytes.next() else { return false };
        if actual == b'%' {
            let Some(high) = bytes.next().and_then(hex) else { return false };
            let Some(low) = bytes.next().and_then(hex) else { return false };
            actual = high * 16 + low;
        }
        if !actual.eq_ignore_ascii_case(&expected) {
            return false;
        }
    }
    bytes.next().is_none()
}

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

fn percent_encode(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    append_percent_encoded(&mut result, value);
    result
}

fn append_percent_encoded(result: &mut String, value: &str) {
    for b in value.bytes() {
        if unreserved(b) {
            result.push(char::from(b));
        } else {
            result.push('%');
            let digit = |n| char::from(if n < 10 { b'0' + n } else { b'A' + n - 10 });
            result.push(digit(b >> 4));
            result.push(digit(b & 15));
        }
    }
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Redacts active raw credentials, percent-encoded credentials (with either hex case), and
/// the bare Base64 payload of Basic credentials. Matching the original input in one pass
/// prevents overlapping values or a credential resembling `[redacted]` from leaking secrets.
pub fn scrub(text: &str) -> String {
    let entries = entries();
    let mut patterns: Vec<_> = entries.values().flat_map(|entry| &entry.redactions).collect();
    patterns.sort_unstable_by_key(|pattern| std::cmp::Reverse(pattern.value.expose().len()));
    if patterns.is_empty() {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(pattern) = patterns.iter().find(|pattern| {
            let value = pattern.value.expose();
            rest.get(..value.len())
                .is_some_and(|prefix| if pattern.encoded { encoded_matches(prefix.as_bytes(), value.as_bytes()) } else { prefix == value })
        }) {
            result.push_str("[redacted]");
            rest = rest.get(pattern.value.expose().len()..).unwrap_or_default();
        } else if let Some(c) = rest.chars().next() {
            result.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or_default();
        }
    }
    result
}

fn encoded_matches(actual: &[u8], expected: &[u8]) -> bool {
    let mut actual = actual.iter();
    let mut expected = expected.iter();
    while let Some(b) = expected.next() {
        if actual.next() != Some(b) {
            return false;
        }
        if *b == b'%' {
            for _ in 0..2 {
                match (actual.next(), expected.next()) {
                    (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => {}
                    _ => return false,
                }
            }
        }
    }
    actual.next().is_none()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn canonical_origins() {
        for (url, expected) in [
            ("HTTPS://Example.COM.:443/a?b=c#d", "https://example.com"),
            ("HTTP://Example.COM.:80/a", "http://example.com"),
            ("https://Example.COM:00443", "https://example.com"),
            ("https://Example.COM.:8443/", "https://example.com:8443"),
            ("https://[2001:0DB8::1]:443/", "https://[2001:db8::1]"),
            ("http://localhost:8080", "http://localhost:8080"),
        ] {
            assert_eq!(origin(url).as_deref(), Some(expected));
        }
    }

    #[test]
    fn refuses_unsafe_origins() {
        for url in [
            "https://user:password@example.com",
            "https://@example.com",
            "https://example.com/a b",
            " https://example.com",
            "https://example.com/\n",
            "https://example.com/\u{a0}",
            "ftp://example.com",
            "https://",
            "https:///path",
            "https://:443",
            "https://example.com:",
            "https://example.com:65536",
            "https://example.com:-1",
            "https://example.com:abc",
            "https://example.com\\other",
            "https://example.com/a\\b",
            "https://[bad]:443",
            "https://[::1]extra",
            "https://host:443:80",
            "https://example.com%2fother",
            "https://example..com",
        ] {
            assert_eq!(origin(url), None, "{url:?}");
        }
    }

    #[test]
    fn validates_names_and_values() {
        assert!(valid_header_name("Authorization"));
        assert!(valid_header_name("X-API_Key!"));
        assert!(valid_header_name(&"a".repeat(MAX_NAME)));
        for name in [
            "",
            "Host",
            "CONNECTION",
            "content-length",
            "Transfer-Encoding",
            "TE",
            "Upgrade",
            "Keep-Alive",
            "Range",
            "Accept-Encoding",
            "Cookie",
            "Origin",
            "Referer",
            "User-Agent",
            "Proxy-Authorization",
            "SEC-Fetch-Site",
            "x y",
            "x:y",
            "ümlaut",
        ] {
            assert!(!valid_header_name(name), "{name}");
        }
        assert!(!valid_header_name(&"a".repeat(MAX_NAME + 1)));
        assert!(valid_query_name("AZaz09-_.~"));
        assert!(valid_query_name(&"a".repeat(MAX_NAME)));
        for name in ["", "x y", "x&y", "x=y", "%61", "ümlaut"] {
            assert!(!valid_query_name(name));
        }
        assert!(!valid_query_name(&"a".repeat(MAX_NAME + 1)));
        assert!(valid_value(" a password with spaces é "));
        assert!(valid_value(&"a".repeat(MAX_VALUE)));
        for value in ["", "bad\rvalue", "bad\nvalue", "bad\tvalue", "bad\0value", "bad\u{7f}value", "bad\u{85}value"] {
            assert!(!valid_value(value));
        }
        assert!(!valid_value(&"é".repeat(MAX_VALUE / 2 + 1)));
    }

    #[test]
    fn standard_basic_authorization() {
        assert_eq!(basic("Aladdin", "open sesame"), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert_eq!(basic("", ""), "Basic Og==");
        assert_eq!(basic("user", "p:a:ss"), "Basic dXNlcjpwOmE6c3M=");
    }

    #[test]
    fn scopes_login_to_exact_origin() {
        let target = "https://exact-login.example:8443";
        activate(target, "exact", Method::Header("Authorization".into()), "exact-login-secret", false).unwrap();
        let applied = apply("HTTPS://EXACT-LOGIN.EXAMPLE.:8443/tiles?x=1").unwrap();
        assert_eq!(applied.url, "HTTPS://EXACT-LOGIN.EXAMPLE.:8443/tiles?x=1");
        assert_eq!(applied.header, Some(("Authorization".into(), "exact-login-secret".into())));
        for url in [
            "http://exact-login.example:8443/tiles",
            "https://exact-login.example/tiles",
            "https://exact-login.example:8444/tiles",
            "https://sub.exact-login.example:8443/tiles",
            "https://exact-login.example.evil:8443/tiles",
        ] {
            assert!(apply(url).is_none());
        }
        assert!(deactivate(target, "exact"));
    }

    #[test]
    fn gates_http_and_requires_canonical_registration() {
        let target = "http://insecure-login.example";
        assert!(activate(target, "insecure", Method::Header("X-Key".into()), "insecure-login-secret", false).is_err());
        assert!(owner(target).is_none());
        activate(target, "insecure", Method::Header("X-Key".into()), "insecure-login-secret", true).unwrap();
        assert!(apply("http://insecure-login.example:80/tiles").is_some());
        assert!(apply("https://insecure-login.example/tiles").is_none());
        for noncanonical in [
            "HTTP://insecure-login.example",
            "http://insecure-login.example/",
            "http://insecure-login.example:80",
            "http://INSECURE-LOGIN.EXAMPLE",
            "http://insecure-login.example.",
        ] {
            assert!(activate(noncanonical, "insecure", Method::Header("X-Key".into()), "insecure-login-secret", true).is_err());
        }
        assert!(deactivate(target, "insecure"));
    }

    #[test]
    fn replacement_transfers_ownership() {
        let target = "https://ownership-login.example";
        activate(target, "old", Method::Header("X-Key".into()), "ownership-old-secret", false).unwrap();
        assert_eq!(owner(target).as_deref(), Some("old"));
        activate(target, "new", Method::Header("X-Key".into()), "ownership-new-secret", false).unwrap();
        assert_eq!(owner(target).as_deref(), Some("new"));
        assert!(!deactivate(target, "old"));
        assert_eq!(apply(target).unwrap().header.unwrap().1, "ownership-new-secret");
        assert!(deactivate(target, "new"));
        assert!(owner(target).is_none());
        assert!(apply(target).is_none());
        assert!(!deactivate(target, "new"));
    }

    #[test]
    fn query_encoding_fragment_and_deduplication() {
        let target = "https://query-login.example";
        activate(target, "query", Method::Query("API_Key".into()), "query value/+?&=é", false).unwrap();
        for (url, expected) in [
            (
                "https://query-login.example/tiles#fragment?ignored",
                "https://query-login.example/tiles?API_Key=query%20value%2F%2B%3F%26%3D%C3%A9#fragment?ignored",
            ),
            (
                "https://query-login.example/tiles?x=1#f",
                "https://query-login.example/tiles?x=1&API_Key=query%20value%2F%2B%3F%26%3D%C3%A9#f",
            ),
            ("https://query-login.example/tiles?", "https://query-login.example/tiles?API_Key=query%20value%2F%2B%3F%26%3D%C3%A9"),
            ("https://query-login.example/tiles?x=1&", "https://query-login.example/tiles?x=1&API_Key=query%20value%2F%2B%3F%26%3D%C3%A9"),
        ] {
            let applied = apply(url).unwrap();
            assert_eq!(applied.url, expected);
            assert!(applied.header.is_none());
        }
        for url in [
            "https://query-login.example/?api_KEY=existing#f",
            "https://query-login.example/?%61PI%5fKey=existing",
            "https://query-login.example/?API_Key",
        ] {
            assert_eq!(apply(url).unwrap().url, url);
        }
        assert!(query_names().iter().any(|name| name == "api_key"));
        assert!(query_names().iter().all(|name| name == &name.to_ascii_lowercase()));
        assert!(deactivate(target, "query"));
    }

    #[test]
    fn debug_and_errors_never_include_secrets() {
        let target = "https://debug-login.example";
        let secret = "debug-login-private-secret";
        activate(target, "debug", Method::Query("token".into()), secret, false).unwrap();
        let debug = format!("{:?}", apply(target).unwrap());
        assert!(!debug.contains(secret));
        assert!(!debug.contains(target));
        assert!(!debug.contains("token"));
        let header = Applied { url: format!("{target}/?secret={secret}"), header: Some((secret.into(), secret.into())) };
        assert_eq!(format!("{header:?}"), "Applied(<redacted>)");
        for error in [
            activate(&format!("https://{secret}/path"), "debug", Method::Header("X-Key".into()), secret, false).unwrap_err(),
            activate(target, "debug", Method::Header(format!("{secret}:")), secret, false).unwrap_err(),
            activate(target, "debug", Method::Header("X-Key".into()), &format!("{secret}\n"), false).unwrap_err(),
        ] {
            assert!(!error.contains(secret));
        }
        assert_eq!(owner(target).as_deref(), Some("debug"));
        assert!(deactivate(target, "debug"));
    }

    #[test]
    fn scrubs_raw_encoded_and_basic_payloads() {
        let target = "https://scrub-login.example";
        let value = "scrub-private/+ é";
        activate(target, "scrub", Method::Query("key".into()), value, false).unwrap();
        assert_eq!(
            scrub("raw scrub-private/+ é encoded scrub-private%2F%2B%20%C3%A9 lowercase scrub-private%2f%2b%20%c3%a9"),
            "raw [redacted] encoded [redacted] lowercase [redacted]"
        );
        let basic_target = "https://scrub-basic-login.example";
        let value = basic("scrub-user", "scrub-password");
        let payload = value.strip_prefix("Basic ").unwrap();
        activate(basic_target, "scrub-basic", Method::Header("Authorization".into()), &value, false).unwrap();
        assert_eq!(scrub(&format!("{value} {payload} {}", percent_encode(&value))), "[redacted] [redacted] [redacted]");
        assert!(deactivate(target, "scrub"));
        assert!(deactivate(basic_target, "scrub-basic"));
        assert_eq!(scrub("ordinary unrelated diagnostic"), "ordinary unrelated diagnostic");
    }

    #[test]
    fn scrubs_longest_overlapping_values_without_rescanning_replacements() {
        let target = "https://scrub-overlap-login.example";
        let other = "https://scrub-overlap-other-login.example";
        activate(target, "overlap", Method::Query("key".into()), "overlap-private", false).unwrap();
        activate(other, "overlap-other", Method::Query("key".into()), "overlap-private-longer", false).unwrap();
        assert_eq!(scrub("overlap-private-longer overlap-private"), "[redacted] [redacted]");
        assert!(deactivate(target, "overlap"));
        assert!(deactivate(other, "overlap-other"));
    }

    #[test]
    fn unicode_host_is_lowercased_without_idna_conversion() {
        assert_eq!(origin("HTTPS://BÜCHER.Example.:443/path").as_deref(), Some("https://bücher.example"));
        let target = "https://unicode-bücher-login.example";
        activate(target, "unicode", Method::Header("X-Key".into()), "unicode-login-private", false).unwrap();
        assert!(apply("https://UNICODE-BÜCHER-LOGIN.EXAMPLE/tiles").is_some());
        assert!(deactivate(target, "unicode"));
    }

    #[test]
    fn query_names_are_lowercase_sorted_and_distinct() {
        let first = "https://query-names-first-login.example";
        let second = "https://query-names-second-login.example";
        activate(first, "names-first", Method::Query("Unique_Login_Names".into()), "query-names-first-private", false).unwrap();
        activate(second, "names-second", Method::Query("UNIQUE_LOGIN_NAMES".into()), "query-names-second-private", false).unwrap();
        let names = query_names();
        assert_eq!(names.iter().filter(|name| name.as_str() == "unique_login_names").count(), 1);
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(names.iter().all(|name| name == &name.to_ascii_lowercase()));
        assert!(deactivate(first, "names-first"));
        assert!(deactivate(second, "names-second"));
    }
}
