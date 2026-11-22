//! Hand-kept lists of command ids name real commands. Dozens of places outside the registry hold
//! command ids as string literals (the routing of Command Search, `ON_A_COPY`, the policy tables,
//! the background registries, the MCP read-ahead set, the ribbon): a command renamed or removed
//! leaves them naming nothing, and nothing fails to compile. This reads the non-test source of
//! every crate and app and fails on an id-shaped literal that is not a command.
//!
//! A literal counts as an id when it is dotted words (`layer.add`) in a namespace some registered
//! command has, or `ui`. It must then be a command the engine registers (`cmd::all()`), or, for
//! `ui.*`, one the app handles: a match arm `"ui.pane.catalog" => …`. The other dotted literals in
//! those namespaces (file names, host names) are told apart by their last word and
//! [`EXEMPT`]. This lives in the UI crate because only it can see both the engine's registry and
//! the app's own table; the scan looks at the files, so it needs no private access.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Last words of dotted literals that are file names or host names, not ids (`layer.json`,
/// `data.seattle.gov`). The test checks that no registered id ends in one, so this can't hide a
/// dangling id.
const NOT_IDS: &[&str] = &["json", "csv", "tif", "parquet", "shp", "wasm", "com", "gov", "org", "net", "us"];

/// Dotted literals in an id's namespace that are not commands, each with why. A literal listed
/// here that the scan no longer finds fails the test, so the list stays true.
const EXEMPT: &[(&str, &str)] = &[
    ("ui.layoutNudge", "an arrow key on a selected layout element, handled where the key is read (chrome.rs), not a command"),
    ("ui.undoHistory", "the toolbar's Undo History button: a menu, not a command"),
    ("ui.stac.planet", "a Web Data preset: `ui.stac.*` is resolved by `stacui::Preset::from_id`"),
    ("ui.stac.portolan", "a Web Data preset: `ui.stac.*` is resolved by `stacui::Preset::from_id`"),
    ("ui.stac.sentinel2", "a Web Data preset: `ui.stac.*` is resolved by `stacui::Preset::from_id`"),
    ("ui.stac.usgsLidar", "a Web Data preset: `ui.stac.*` is resolved by `stacui::Preset::from_id`"),
    ("ui.stac.vantor", "a Web Data preset: `ui.stac.*` is resolved by `stacui::Preset::from_id`"),
    ("data.cityofberkeley.info", "an open-data portal's host name (it ends in a word that `time.info` also ends in)"),
    ("stac.cut_note", "a key for a widget's remembered state in the STAC pane (stacui.rs), not a command"),
];

/// The repository's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `.rs` files of every crate's and app's `src`, leaving out test files and directories.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "benches" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") && !name.ends_with("_test.rs") {
            out.push(path);
        }
    }
}

/// A file's code up to its test module (`#[cfg(test)]` before a `mod`, whatever the line ends).
fn non_test(text: &str) -> &str {
    let at = text.match_indices("#[cfg(test)]").find(|(i, m)| text[i + m.len()..].trim_start().starts_with("mod "));
    at.map_or(text, |(i, _)| &text[..i])
}

/// The id-shaped string literals of `text`, with their line: dotted words, the first a lower-case
/// word, none empty. A literal ending in a dot is a prefix (`"ui.pane."`), not an id.
fn literals(text: &str) -> Vec<(usize, &str, usize)> {
    let mut out = Vec::new();
    let mut line_start = 0;
    for (n, line) in text.split('\n').enumerate() {
        let start = line_start;
        line_start += line.len() + 1;
        if line.trim_start().starts_with("//") {
            continue;
        }
        let b = line.as_bytes();
        for (i, _) in line.match_indices('"') {
            let rest = &line[i + 1..];
            let Some(end) = rest.find('"') else { continue };
            let word = &rest[..end];
            let words_ok = word.split('.').all(|w| !w.is_empty() && w.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'));
            let first_ok = word.bytes().next().is_some_and(|c| c.is_ascii_lowercase());
            // A closing quote is no opening one: the byte before it isn't part of a word.
            let opening = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.');
            if opening && first_ok && words_ok && word.contains('.') {
                out.push((n + 1, word, start + i + 1 + end + 1));
            }
        }
    }
    out
}

/// Whether the literal whose closing quote ends before `after` in `text` is (one of the
/// alternatives of) a match arm: followed, through any more `| "…"` alternatives, by `=>` or an
/// `if` guard. A list in `matches!(…)` ends in a bracket and isn't one.
fn is_arm(text: &str, after: usize) -> bool {
    let mut rest = text[after..].trim_start();
    loop {
        if rest.starts_with("=>") || rest.starts_with("if ") {
            return true;
        }
        let Some(next) = rest.strip_prefix('|').map(str::trim_start).and_then(|r| r.strip_prefix('"')) else { return false };
        let Some(end) = next.find('"') else { return false };
        rest = next[end + 1..].trim_start();
    }
}

#[test]
fn command_ids_in_hand_kept_lists_are_registered() {
    let commands = rustgis_engine::cmd::all();
    let ids: HashSet<&str> = commands.iter().map(|c| c.id).collect();
    let mut namespaces: BTreeSet<&str> = ids.iter().filter_map(|id| id.split('.').next()).collect();
    namespaces.insert("ui");
    assert!(ids.len() > 500, "the registry has {} commands", ids.len());
    let ends_like_a_file = ids.iter().filter(|id| id.rsplit('.').next().is_some_and(|w| NOT_IDS.contains(&w))).collect::<Vec<_>>();
    assert!(ends_like_a_file.is_empty(), "registered ids end in a word NOT_IDS treats as a file or host name: {ends_like_a_file:?}");

    let root = root();
    let mut files = Vec::new();
    for group in ["crates", "apps"] {
        for package in std::fs::read_dir(root.join(group)).into_iter().flatten().flatten() {
            sources(&package.path().join("src"), &mut files);
        }
    }
    files.sort();
    assert!(files.len() > 200, "the scan found {} source files", files.len());
    let texts: Vec<(String, String)> = files
        .iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(f).ok()?;
            Some((f.strip_prefix(&root).unwrap_or(f).display().to_string().replace('\\', "/"), non_test(&text).to_string()))
        })
        .collect();

    // The `ui.*` commands the app handles: those with a match arm.
    let mut ui_arms = HashSet::new();
    for (_, text) in &texts {
        for (_, word, after) in literals(text) {
            if word.starts_with("ui.") && is_arm(text, after) {
                ui_arms.insert(word.to_string());
            }
        }
    }
    assert!(ui_arms.len() > 100, "the scan found {} ui commands with an arm", ui_arms.len());

    let (mut seen, mut exempt_seen, mut dangling) = (0, HashSet::new(), Vec::new());
    for (file, text) in &texts {
        for (line, word, _) in literals(text) {
            let namespace = word.split('.').next().unwrap_or("");
            let last = word.rsplit('.').next().unwrap_or("");
            if !namespaces.contains(namespace) || NOT_IDS.contains(&last) {
                continue;
            }
            seen += 1;
            if let Some((id, _)) = EXEMPT.iter().find(|(id, _)| *id == word) {
                exempt_seen.insert(*id);
            } else if !(ids.contains(word) || ui_arms.contains(word)) {
                dangling.push(format!("{file}:{line}: \"{word}\""));
            }
        }
    }
    assert!(seen > 2000, "the scan found {seen} command ids");
    assert!(
        dangling.is_empty(),
        "{} command ids in the source are not commands (a command renamed or removed? or add to EXEMPT with a reason):\n{}",
        dangling.len(),
        dangling.join("\n")
    );
    let stale: Vec<_> = EXEMPT.iter().filter(|(id, _)| !exempt_seen.contains(id)).map(|(id, _)| *id).collect();
    assert!(stale.is_empty(), "EXEMPT lists literals the source no longer has: {stale:?}");
}
