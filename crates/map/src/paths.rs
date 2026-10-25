//! Lexical local paths for portable projects; never touches the filesystem.

/// Strip a local file URI prefix, including the URI's extra slash before a Windows drive.
pub fn local_path(p: &str) -> &str {
    let Some(path) = p.strip_prefix("file://") else { return p };
    if let Some(disk) = path.strip_prefix('/')
        && drive(disk)
        && disk.as_bytes().get(2).is_some_and(|b| matches!(b, b'/' | b'\\'))
    {
        return disk;
    }
    path
}

fn drive(p: &str) -> bool {
    let b = p.as_bytes();
    b.first().is_some_and(u8::is_ascii_alphabetic) && b.get(1) == Some(&b':')
}

/// Whether a value names a local file rather than an address or database.
pub fn is_local(p: &str) -> bool {
    if p.is_empty() || p != p.trim() || p.len() > 4096 || p.chars().any(char::is_control) {
        return false;
    }
    let p = local_path(p);
    if p.is_empty() {
        return false;
    }
    let first = p.split(['/', '\\']).next().unwrap_or("");
    !first.contains(':') || (first.len() == 2 && drive(first) && p.as_bytes().get(2).is_some_and(|b| matches!(b, b'/' | b'\\')))
}

/// Absolute POSIX or drive path, excluding network shares.
pub fn is_absolute_local(p: &str) -> bool {
    let b = local_path(p);
    is_local(p)
        && ((b.starts_with('/') && !b.starts_with("//")) || (drive(b) && b.as_bytes().get(2).is_some_and(|c| matches!(c, b'/' | b'\\'))))
}

/// A linked path that needs the project's containing folder.
pub fn is_project_relative(p: &str) -> bool {
    is_local(p) && !p.starts_with("file://") && !p.starts_with(['/', '\\']) && !drive(p)
}

fn components(p: &str, windows: bool) -> impl Iterator<Item = &str> {
    p.split(move |c| c == '/' || (windows && c == '\\')).filter(|s| !s.is_empty() && *s != ".")
}

fn parts(p: &str) -> Option<(String, Vec<&str>)> {
    if !is_absolute_local(p) {
        return None;
    }
    let p = local_path(p);
    let (root, rest) = if drive(p) { (p.get(..2)?.to_ascii_uppercase(), p.get(2..)?) } else { ("/".to_string(), p) };
    let mut result = Vec::new();
    for c in components(rest, cfg!(windows) || root != "/") {
        if c == ".." {
            result.pop();
        } else {
            result.push(c);
        }
    }
    Some((root, result))
}

/// Relativize absolute paths on the same root, always using forward slashes.
pub fn relative_to(dir: &str, path: &str) -> Option<String> {
    let (root, d) = parts(dir)?;
    let (other, p) = parts(path)?;
    if root != other {
        return None;
    }
    let common = d.iter().zip(&p).take_while(|(a, b)| if root == "/" { a == b } else { a.eq_ignore_ascii_case(b) }).count();
    let up = d.len() - common;
    let rest = p.get(common..)?.join("/");
    Some(if up == 0 { if rest.is_empty() { ".".to_string() } else { format!("./{rest}") } } else { format!("{}{rest}", "../".repeat(up)) })
}

/// Resolve a relative path lexically, never traversing above the root.
pub fn resolve(dir: &str, rel: &str) -> Option<String> {
    if !is_project_relative(rel) {
        return None;
    }
    let (root, mut p) = parts(dir)?;
    for c in components(rel, cfg!(windows) || root != "/") {
        if c == ".." {
            p.pop();
        } else {
            p.push(c);
        }
    }
    Some(if root == "/" { format!("/{}", p.join("/")) } else { format!("{root}\\{}", p.join("\\")) })
}

/// Explain why an unbound relative source cannot be opened.
pub fn unknown_folder(path: &str) -> String {
    format!(
        "{path} is relative to the project's folder, which isn't known here: open the project from its .rgis file (the web app can't read files on this computer; add them with Add Data)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_relative_and_round_trip() {
        for (d, p, expected, normalized) in [
            ("/p/survey", "/p/survey/data/e.tif", "./data/e.tif", "/p/survey/data/e.tif"),
            ("/p/survey", "/p/shared/c.fgb", "../shared/c.fgb", "/p/shared/c.fgb"),
            ("/p/survey/deep", "/p/shared/c.fgb", "../../shared/c.fgb", "/p/shared/c.fgb"),
            ("/p", "/p/my data/ü.tif", "./my data/ü.tif", "/p/my data/ü.tif"),
            ("/p/survey", "file:///p/survey/t.pmtiles", "./t.pmtiles", "/p/survey/t.pmtiles"),
            ("/p/survey", "/p/./survey/data/../t.tif", "./t.tif", "/p/survey/t.tif"),
            ("/p", "/p", ".", "/p"),
            (r"C:\p", r"c:\p\d\x.tif", "./d/x.tif", r"C:\p\d\x.tif"),
            (r"C:\p", "file:///C:/p/x.tif", "./x.tif", r"C:\p\x.tif"),
        ] {
            let r = relative_to(d, p);
            assert_eq!(r.as_deref(), Some(expected));
            assert_eq!(resolve(d, expected).as_deref(), Some(normalized));
        }
        assert_eq!(relative_to(r"C:\p", r"D:\d\x.tif"), None);
        assert_eq!(relative_to("/p", r"\\server\x"), None);
        assert_eq!(relative_to("/p", "//server/x"), None);
        assert_eq!(resolve("/p", "../../../x").as_deref(), Some("/x"));
    }

    #[test]
    fn paths_classification() {
        for p in [
            "https://host/x",
            "s3://b/x",
            "rustgis-s3::x",
            "upload:x",
            "postgres:host=x",
            "md:db",
            "C:roads.gpkg",
            "",
            " x",
            "x\n",
            "file://",
        ] {
            assert!(!is_local(p), "{p:?}");
            assert!(!is_project_relative(p));
        }
        for p in ["/a/b:c.tif", r"C:\d\x.tif", "data/x.tif", "file:///p/x"] {
            assert!(is_local(p), "{p}");
        }
        for p in ["data/x.tif", "./x", "../x", r".\x"] {
            assert!(is_project_relative(p));
        }
        for p in ["/a", r"\a", r"C:\a", "file:///a", "//host/a", r"\\host\a"] {
            assert!(!is_project_relative(p));
        }
        assert!(!is_local(&"x".repeat(4097)));
    }
}
