//! `cargo xtask notices [--check]`: the third-party licence notices shipped with RustGIS.
//!
//! The crates RustGIS is built from are MIT, BSD, ISC, Apache and so on, and those licences ask
//! for their copyright and licence text to go with every copy, binaries included. This writes
//! them, from `cargo about` (see [`INSTALL`], configured by `about.toml`),
//! into a plain-text file next to each LICENSE the packages ship:
//!
//! - `THIRD-PARTY-LICENSES.txt`: every crate in the workspace (the desktop app, the command line
//!   and the web app);
//! - `apps/rustgis-python/THIRD-PARTY-LICENSES.txt`: the crates in the Python package.
//!
//! Each distinct licence text appears once, with the crates under it (by name, not version, so a
//! routine version bump leaves the files alone), followed by the crates' NOTICE files (Apache-2.0
//! section 4(d)). `--check` fails when a file is out of date instead of writing it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// How to install the cargo-about the files were generated with (CI pins the same version).
pub const INSTALL: &str = "cargo install --locked --features cli cargo-about@0.9.2";

/// The `--check` exit code for files that are out of date (other failures exit with 1).
pub const STALE: u8 = 2;

/// (output file, package manifest or "" for the whole workspace, what it covers).
const FILES: &[(&str, &str, &str)] = &[
    ("THIRD-PARTY-LICENSES.txt", "", "RustGIS (the desktop app, the command line and the web app)"),
    ("apps/rustgis-python/THIRD-PARTY-LICENSES.txt", "apps/rustgis-python/Cargo.toml", "the rustgis Python package"),
];

/// Text of `v[key]`, or "".
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A licence text with Windows line ends and trailing spaces gone, and no blank lines at either end.
fn tidy(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_matches('\n').to_string()
}

/// `names` as comma-separated lines no wider than 100 columns.
fn wrap(names: &[String]) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for (i, n) in names.iter().enumerate() {
        let item = if i + 1 < names.len() { format!("{n},") } else { n.clone() };
        if !line.is_empty() && line.len() + 1 + item.len() > 100 {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&item);
    }
    out.push_str(&line);
    out
}

/// The notices file for `cargo about`'s JSON report `about`; `what` names what it covers.
/// `notice` reads a crate's NOTICE files from its manifest path, as (file name, contents).
pub fn render(about: &Value, what: &str, notice: &dyn Fn(&str) -> Vec<(String, String)>) -> String {
    let empty = Vec::new();
    // Licence texts, keyed by (licence id, text with its whitespace collapsed): one entry for the
    // same text that differs only in wrapping. Value: (name, text, crate names).
    let mut texts: BTreeMap<(String, String), (String, String, Vec<String>)> = BTreeMap::new();
    for lic in about.get("licenses").and_then(Value::as_array).unwrap_or(&empty) {
        let body = tidy(text(lic, "text"));
        let key = (text(lic, "id").to_string(), body.split_whitespace().collect::<Vec<_>>().join(" "));
        let entry = texts.entry(key).or_insert_with(|| (text(lic, "name").to_string(), body, Vec::new()));
        for used in lic.get("used_by").and_then(Value::as_array).unwrap_or(&empty) {
            let krate = used.get("crate").unwrap_or(&Value::Null);
            // Path crates (`source` null) are RustGIS's own.
            if !krate.get("source").is_some_and(Value::is_null) {
                entry.2.push(text(krate, "name").to_string());
            }
        }
    }
    let mut texts: Vec<_> = texts.into_values().filter(|(_, _, names)| !names.is_empty()).collect();
    for (_, _, names) in &mut texts {
        names.sort();
        names.dedup();
    }
    texts.sort_by(|a, b| (&a.0, &a.2).cmp(&(&b.0, &b.2)));

    let mut notices: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for c in about.get("crates").and_then(Value::as_array).unwrap_or(&empty) {
        let p = c.get("package").unwrap_or(&Value::Null);
        if p.get("source").is_some_and(Value::is_null) {
            continue;
        }
        let found = notice(text(p, "manifest_path"));
        if !found.is_empty() {
            notices.entry(text(p, "name").to_string()).or_insert(found);
        }
    }

    let rule = "=".repeat(100);
    let mut out = format!(
        "Third-party software notices for {what}\n\n\
         RustGIS is licensed under the Apache License 2.0 (see LICENSE). It is built with the\n\
         open-source Rust crates below, each under its own licence, whose copyright notices and terms\n\
         are reproduced here, followed by the NOTICE files the crates ship. Generated from Cargo.lock\n\
         by `cargo xtask notices`.\n"
    );
    for (name, body, names) in &texts {
        out.push_str(&format!("\n{rule}\n{name}\nUsed by: {}\n{rule}\n\n{body}\n", wrap(names)));
    }
    for (krate, files) in &notices {
        for (file, body) in files {
            out.push_str(&format!("\n{rule}\n{file} of {krate}\n{rule}\n\n{}\n", tidy(body)));
        }
    }
    out
}

/// The NOTICE files next to the manifest at `manifest_path`.
fn notice_files(manifest_path: &str) -> Vec<(String, String)> {
    let Some(dir) = Path::new(manifest_path).parent() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ok = name.to_ascii_uppercase().starts_with("NOTICE") && e.file_type().is_ok_and(|t| t.is_file());
            ok.then(|| std::fs::read_to_string(e.path()).ok().map(|body| (name, body))).flatten()
        })
        .collect();
    found.sort();
    found
}

/// `cargo about`'s JSON report for `manifest` (the workspace when empty).
fn about(root: &Path, manifest: &str) -> Result<Value, String> {
    let mut cmd = Command::new("cargo");
    // --fail: a crate whose licence cargo-about can't work out is an error, not a gap in the file.
    cmd.args(["about", "generate", "--locked", "--fail", "--format", "json"]).current_dir(root);
    if !manifest.is_empty() {
        cmd.args(["--manifest-path", manifest]);
    }
    println!(
        "$ cargo about generate --locked --fail --format json{}",
        if manifest.is_empty() { String::new() } else { format!(" --manifest-path {manifest}") }
    );
    let out = cmd.output().map_err(|e| format!("cargo about: {e} (install it with `{INSTALL}`)"))?;
    if !out.status.success() {
        return Err(format!("cargo about failed (install it with `{INSTALL}`):\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo about's report: {e}"))
}

/// Writes the files, or with `check` reports the ones out of date: `Err((STALE, …))` for those,
/// `Err((1, …))` for any other failure.
pub fn notices(root: &Path, check: bool) -> Result<(), (u8, String)> {
    let mut stale = Vec::new();
    for (file, manifest, what) in FILES {
        let wanted = render(&about(root, manifest).map_err(|e| (1, e))?, what, &notice_files);
        let path = root.join(file);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(wanted.as_str()) {
            println!("notices: {file} is up to date");
        } else if check {
            stale.push(*file);
        } else {
            std::fs::write(&path, wanted).map_err(|e| (1, format!("{file}: {e}")))?;
            println!("notices: wrote {file}");
        }
    }
    if stale.is_empty() {
        Ok(())
    } else {
        Err((STALE, format!("out of date: {}; run `cargo xtask notices` and commit the result", stale.join(", "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_merges_texts_and_skips_rustgis_crates() {
        let krate = |name: &str, source: Value| serde_json::json!({"crate": {"name": name, "version": "1.0.0", "source": source}});
        let reg = Value::from("registry+https://github.com/rust-lang/crates.io-index");
        let about = serde_json::json!({
            "licenses": [
                {"name": "MIT License", "id": "MIT", "text": "Copyright A\r\n\r\nPermission is granted.  \r\n", "used_by": [krate("b", reg.clone()), krate("a", reg.clone())]},
                // The same text wrapped differently: one entry.
                {"name": "MIT License", "id": "MIT", "text": "Copyright A\n\nPermission\nis granted.", "used_by": [krate("a", reg.clone()), krate("c", reg.clone())]},
                {"name": "Apache License 2.0", "id": "Apache-2.0", "text": "Apache text", "used_by": [krate("rustgis-geom", Value::Null)]},
            ],
            "crates": [
                {"package": {"name": "a", "source": reg.clone(), "manifest_path": "/a/Cargo.toml"}},
                {"package": {"name": "rustgis-geom", "source": Value::Null, "manifest_path": "/g/Cargo.toml"}},
            ],
        });
        let notice = |m: &str| if m == "/a/Cargo.toml" { vec![("NOTICE".to_string(), "Notice of a\n".to_string())] } else { Vec::new() };
        let out = render(&about, "RustGIS", &notice);
        assert!(out.starts_with("Third-party software notices for RustGIS\n\nRustGIS is licensed"));
        assert_eq!(out.matches("MIT License\nUsed by: a, b, c\n").count(), 1);
        assert!(out.contains("Copyright A\n\nPermission is granted.\n"));
        // Only RustGIS's own crates use that Apache text, so it is left out.
        assert!(!out.contains("Apache text") && !out.contains("rustgis-geom"));
        assert!(out.ends_with(&format!("NOTICE of a\n{}\n\nNotice of a\n", "=".repeat(100))));
    }

    #[test]
    fn wrap_keeps_lines_short() {
        let names: Vec<String> = (0..40).map(|i| format!("crate-{i}")).collect();
        let w = wrap(&names);
        assert!(w.lines().all(|l| l.len() <= 100) && w.lines().count() > 1);
        assert!(w.ends_with("crate-39") && w.contains("crate-0, crate-1,"));
    }
}
