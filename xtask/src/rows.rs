//! `cargo xtask rows [--update] [--list]`: counts the places outside `rustgis-data` that use a
//! layer's rows directly (the row vector behind `Features`, or a `Feature`'s `id`, `geometry` and
//! `attrs`) instead of the row accessors, and fails when there are more than `xtask/row-uses.txt`
//! allows (#2431). Moving the rows to columns needs that number at zero; this stops it growing in
//! the meantime.
//!
//! The compiler finds them, not a pattern: `cargo check` of the workspace's libraries and programs
//! (not tests, examples or benches) with `rustgis-data`'s `count-row-uses` feature, which marks the
//! fields and the vector's methods deprecated, and every "use of deprecated" warning with their
//! note outside `crates/data/` is one use. Other types with fields of the same names are never
//! counted. Not counted: indexing (`features[i]`), iterating `&features` in a `for` loop and
//! lending it as a `&[Feature]`, unless the row's fields are read.
//!
//! When the count falls, `--update` writes the new one; it never raises it. `--list` prints each
//! use (`file:line:column`).

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

const FILE: &str = "xtask/row-uses.txt";

/// What the deprecation notes on the counted items end with, and nothing else's does. The message
/// names the item by the path it was reached through (`rustgis_engine::rustgis_data::Feature`).
const NOTE: &str = "see cargo xtask rows";

/// A counted use: the file (from the workspace root), line and column.
type Use = (String, u64, u64);

/// The uses in `cargo check --message-format=json` output.
fn uses(output: &str) -> BTreeSet<Use> {
    let mut found = BTreeSet::new();
    for line in output.lines() {
        let Ok(m) = serde_json::from_str::<Value>(line) else { continue };
        let msg = &m["message"];
        let text = msg["message"].as_str().unwrap_or("");
        if m["reason"] != "compiler-message" || msg["code"]["code"] != "deprecated" || !text.contains(NOTE) {
            continue;
        }
        let Some(span) = msg["spans"].as_array().and_then(|s| s.iter().find(|s| s["is_primary"] == true)) else { continue };
        let file = span["file_name"].as_str().unwrap_or("").replace('\\', "/");
        if file.starts_with("crates/data/") || file.is_empty() {
            continue;
        }
        found.insert((file, span["line_start"].as_u64().unwrap_or(0), span["column_start"].as_u64().unwrap_or(0)));
    }
    found
}

/// The package directory a file is in (`crates/engine`), for the per-package summary.
fn package(file: &str) -> String {
    file.split('/').take(2).collect::<Vec<_>>().join("/")
}

/// The count `text` (the contents of [`FILE`]) allows.
fn allowed(text: &str) -> Result<usize, String> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == "count").map(|(_, v)| v.trim().to_string()))
        .ok_or_else(|| format!("{FILE}: no `count = <number>` line"))?
        .parse()
        .map_err(|e| format!("{FILE}: count: {e}"))
}

/// [`FILE`] with its count set to `count`, comments kept.
fn with_count(text: &str, count: usize) -> String {
    let mut out: String = text.lines().take_while(|l| l.starts_with('#') || l.trim().is_empty()).map(|l| format!("{l}\n")).collect();
    out.push_str(&format!("count = {count}\n"));
    out
}

/// `RUSTFLAGS` without `-D warnings`, which would turn the counted warnings into errors and stop
/// the packages after the first one with a use from being checked at all.
fn flags_without_deny_warnings(flags: &str) -> String {
    let mut kept = Vec::new();
    let mut words = flags.split_whitespace().peekable();
    while let Some(w) = words.next() {
        if w == "-Dwarnings" || w == "--deny=warnings" {
            continue;
        }
        if (w == "-D" || w == "--deny") && words.peek() == Some(&"warnings") {
            words.next();
            continue;
        }
        kept.push(w);
    }
    kept.join(" ")
}

/// Runs the check build and returns the uses it reports.
fn measure(root: &Path) -> Result<BTreeSet<Use>, String> {
    let args =
        ["check", "--workspace", "--lib", "--bins", "--locked", "--features", "rustgis-data/count-row-uses", "--message-format=json"];
    println!("$ cargo {}", args.join(" "));
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(args).current_dir(root).stderr(Stdio::inherit());
    if let Ok(flags) = std::env::var("RUSTFLAGS") {
        cmd.env("RUSTFLAGS", flags_without_deny_warnings(&flags));
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("rows: the check build failed, so nothing was counted (the errors are above)".into());
    }
    Ok(uses(&String::from_utf8_lossy(&out.stdout)))
}

/// Runs `cargo xtask rows` with the arguments after `rows`.
pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: cargo xtask rows [--update] [--list]";
    let (mut update, mut list) = (false, false);
    for a in args {
        match a.as_str() {
            "--update" => update = true,
            "--list" => list = true,
            _ => return Err(USAGE.into()),
        }
    }
    let path = root.join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{FILE}: {e}"))?;
    let limit = allowed(&text)?;
    let found = measure(root)?;
    if list {
        found.iter().for_each(|(f, l, c)| println!("{f}:{l}:{c}"));
    }
    let mut by_package: BTreeMap<String, usize> = BTreeMap::new();
    found.iter().for_each(|(f, _, _)| *by_package.entry(package(f)).or_default() += 1);
    let summary: Vec<String> = by_package.iter().map(|(p, n)| format!("{p} {n}")).collect();
    let count = found.len();
    println!("rows: {count} direct uses of a layer's rows outside rustgis-data (allowed: {limit}): {}", summary.join(", "));
    if count > limit {
        return Err(format!(
            "rows: {} more direct uses of a layer's rows than {FILE} allows. Read and change rows through the accessors on \
             `Features` (`row`, `rows`, `value`, `geometry`, `column`, `set_value`, `set_geometry`, `push_row`, `remove_row`, \
             `retain_rows`, ...; crates/data/src/row_access.rs) rather than a `Feature`'s fields or the row vector. \
             `cargo xtask rows --list` prints every use.",
            count - limit
        ));
    }
    if count < limit {
        if update {
            std::fs::write(&path, with_count(&text, count)).map_err(|e| format!("{FILE}: {e}"))?;
            println!("rows: wrote {count} to {FILE}");
        } else {
            println!("rows: {} fewer than allowed: run `cargo xtask rows --update` and commit {FILE}", limit - count);
        }
    }
    println!("rows: ok");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(code: &str, text: &str, file: &str, line: u64) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": text,
                "code": { "code": code },
                "spans": [
                    { "file_name": "elsewhere.rs", "line_start": 1, "column_start": 1, "is_primary": false },
                    { "file_name": file, "line_start": line, "column_start": 9, "is_primary": true }
                ]
            }
        })
        .to_string()
    }

    #[test]
    fn counts_deprecated_row_uses_outside_the_data_crate_once_each() {
        let field = "use of deprecated field `rustgis_engine::rustgis_data::Feature::attrs`: read rows ...; see cargo xtask rows";
        let method = "use of deprecated method `rustgis_data::row_access::counted::<impl Features>::get`: ...; see cargo xtask rows";
        let output = [
            message("deprecated", field, "crates/engine/src/lib.rs", 10),
            // The same warning twice (a package's library and program share a file).
            message("deprecated", field, "crates/engine/src/lib.rs", 10),
            message("deprecated", method, "crates/map/src/lib.rs", 3),
            // In the data crate, which owns the rows.
            message("deprecated", field, "crates/data/src/io.rs", 5),
            // Something else deprecated, and another warning.
            message("deprecated", "use of deprecated function `egui::old`", "crates/ui-egui/src/lib.rs", 7),
            message("unused_variables", "unused variable: see cargo xtask rows", "crates/map/src/lib.rs", 4),
            "not json".to_string(),
            r#"{"reason":"compiler-artifact"}"#.to_string(),
        ]
        .join("\n");
        let found = uses(&output);
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [("crates/engine/src/lib.rs".to_string(), 10, 9), ("crates/map/src/lib.rs".to_string(), 3, 9)]
        );
    }

    #[test]
    fn the_stored_count_reads_and_rewrites_keeping_its_comments() {
        let text = "# why\n# how\n\ncount = 42\n";
        assert_eq!(allowed(text), Ok(42));
        assert_eq!(with_count(text, 40), "# why\n# how\n\ncount = 40\n");
        assert!(allowed("# nothing\n").is_err());
        assert!(allowed("count = many\n").is_err());
    }

    #[test]
    fn deny_warnings_is_taken_out_of_rustflags_and_nothing_else() {
        assert_eq!(flags_without_deny_warnings("-D warnings -C debuginfo=1"), "-C debuginfo=1");
        assert_eq!(flags_without_deny_warnings("-Dwarnings --deny=warnings --deny warnings -D unsafe_code"), "-D unsafe_code");
        assert_eq!(flags_without_deny_warnings(""), "");
    }

    #[test]
    fn the_stored_count_parses() {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("row-uses.txt")).unwrap();
        assert!(allowed(&text).is_ok());
    }
}
