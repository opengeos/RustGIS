//! Which addresses tiles and documents may be fetched from.
//!
//! A tile layer's own address is the user's choice (a project or a command names it), so it may
//! point at this computer or the local network: a self-hosted tile server. Addresses that come
//! from somewhere else (inside a style or TileJSON document, or a redirect) may not reach into
//! the local network unless the document itself came from there, so a style found on the web
//! can't make the app probe local services. Link-local addresses, where cloud metadata services
//! answer, are never fetched.
//!
//! Hosts are judged by name or literal address; names are not resolved, so a public name that
//! resolves to a local address is not caught here.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// How far an address reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// The internet.
    Public,
    /// This computer or the local network (loopback, private and unique-local addresses).
    Local,
    /// Never fetched: link-local and unspecified addresses, cloud metadata hosts.
    Forbidden,
}

/// The host of an HTTP(S) address, without user info, port or IPv6 brackets.
fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = authority.strip_prefix('[') { v6.split(']').next()? } else { authority.split(':').next()? };
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

fn ip_reach(ip: IpAddr) -> Reach {
    match ip {
        IpAddr::V4(v4) => v4_reach(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4_reach(v4);
            }
            let seg0 = v6.segments().first().copied().unwrap_or(0);
            if v6.is_unspecified() || (seg0 & 0xffc0) == 0xfe80 {
                Reach::Forbidden
            } else if v6 == Ipv6Addr::LOCALHOST || (seg0 & 0xfe00) == 0xfc00 {
                Reach::Local
            } else {
                Reach::Public
            }
        }
    }
}

fn v4_reach(v4: Ipv4Addr) -> Reach {
    if v4.is_unspecified() || v4.is_link_local() || v4.is_broadcast() {
        Reach::Forbidden
    } else if v4.is_loopback() || v4.is_private() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64) {
        // 100.64.0.0/10 (carrier-grade NAT) counts as local too.
        Reach::Local
    } else {
        Reach::Public
    }
}

/// How far `url` reaches, judged by its host (names are not resolved).
pub fn reach(url: &str) -> Reach {
    let Some(h) = host(url) else { return Reach::Forbidden };
    if let Ok(ip) = h.parse::<IpAddr>() {
        return ip_reach(ip);
    }
    if ["metadata.google.internal", "metadata", "instance-data"].contains(&h.as_str()) {
        Reach::Forbidden
    } else if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") || h.ends_with(".internal") {
        Reach::Local
    } else {
        Reach::Public
    }
}

/// Whether `url` may be fetched. `from` is where it was found (a document, or the address that
/// redirected to it), or `None` when the user gave it.
pub fn check(url: &str, from: Option<&str>) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(format!("{url}: only http:// and https:// addresses are fetched"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{url}: the address has spaces in it"));
    }
    match (reach(url), from) {
        (Reach::Forbidden, _) => Err(format!("{url}: that address is never fetched")),
        (Reach::Local, Some(origin)) if reach(origin) == Reach::Public => {
            Err(format!("{url}: a document on the web can't point into the local network"))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judges_addresses() {
        assert_eq!(reach("https://tile.openstreetmap.org/1/0/0.png"), Reach::Public);
        assert_eq!(reach("http://localhost:3000/tiles"), Reach::Local);
        assert_eq!(reach("http://127.0.0.1/x"), Reach::Local);
        assert_eq!(reach("http://192.168.1.20/x"), Reach::Local);
        assert_eq!(reach("http://[::1]:8080/x"), Reach::Local);
        assert_eq!(reach("http://[fd00::1]/x"), Reach::Local);
        assert_eq!(reach("http://169.254.169.254/latest/meta-data/"), Reach::Forbidden);
        assert_eq!(reach("http://[::ffff:169.254.169.254]/"), Reach::Forbidden);
        assert_eq!(reach("http://metadata.google.internal/"), Reach::Forbidden);
        assert_eq!(reach("http://0.0.0.0/"), Reach::Forbidden);
        assert_eq!(reach("http://user@localhost:80@example.com/"), Reach::Public);
        assert_eq!(reach("garbage"), Reach::Forbidden);
    }

    #[test]
    fn checks_where_addresses_come_from() {
        // The user's own choice may be local; a web document's may not.
        assert!(check("http://localhost:3000/style.json", None).is_ok());
        assert!(check("http://localhost:3000/{z}/{x}/{y}.pbf", Some("http://localhost:3000/style.json")).is_ok());
        assert!(check("http://10.0.0.5/{z}/{x}/{y}.pbf", Some("https://example.com/style.json")).is_err());
        assert!(check("https://tiles.example.com/{z}", Some("https://example.com/style.json")).is_ok());
        assert!(check("http://169.254.169.254/", None).is_err());
        assert!(check("file:///etc/passwd", None).is_err());
    }
}
