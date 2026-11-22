//! `cargo xtask attribution`: every image and font file in the repository has a row in
//! `ATTRIBUTION.md`.
//!
//! AGENTS.md says every asset file has a row. The table also lists test data and fixtures, but
//! not by any rule that a program could apply to every file, so this checks the classes the rule
//! names: images, icons and fonts. A file is covered when a backticked path in `ATTRIBUTION.md`
//! (one with a `/`) names it, names a directory above it (a trailing `/` is optional), or matches
//! it as a pattern, in which `*` stands for any characters, `/` included. A bare file name (or a
//! pattern with no `/`) covers only a file at the repository root, so a row can't cover a
//! same-named file somewhere else.

use std::path::Path;

/// The extensions of image, icon and font files.
const ASSET_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "icns", "svg", "ttf", "otf", "ttc", "woff", "woff2", "eot"];

/// Whether `text` matches `pattern`, in which `*` stands for any run of characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, rest @ ..] = parts.as_slice() else { return false };
    let Some(mut at) = text.strip_prefix(first) else { return false };
    let Some((last, middle)) = rest.split_last() else { return at.is_empty() };
    for part in middle {
        let Some(found) = at.find(part) else { return false };
        at = &at[found + part.len()..];
    }
    at.ends_with(last)
}

/// The paths and patterns `ATTRIBUTION.md` names: everything in backticks that has a `/` in it (the
/// path of a file may hold spaces) or is a single word with a `.` (a file at the root), each with
/// a trailing `/` taken off and whether it had a `/` at all.
fn named(attribution: &str) -> Vec<(&str, bool)> {
    attribution
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| !t.contains(['\n', '\t']) && !t.starts_with('/'))
        .filter(|t| t.contains('/') || (t.contains('.') && !t.contains(' ')))
        .map(|t| (t.trim_end_matches('/'), t.contains('/')))
        .filter(|(t, _)| !t.is_empty())
        .collect()
}

/// The asset files in `files` that no row of `attribution` covers.
fn uncovered<'a>(files: &[&'a str], attribution: &str) -> Vec<&'a str> {
    let named = named(attribution);
    files
        .iter()
        .copied()
        .filter(|f| f.rsplit_once('.').is_some_and(|(_, ext)| ASSET_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())))
        .filter(|f| {
            !named.iter().any(|(n, path)| {
                if *path { f == n || f.starts_with(&format!("{n}/")) || wildcard(n, f) } else { !f.contains('/') && wildcard(n, f) }
            })
        })
        .collect()
}

/// Runs `cargo xtask attribution`: fails listing each image or font file that is tracked (or new
/// and not ignored) without a row.
pub fn run(root: &Path) -> Result<(), String> {
    let attribution = std::fs::read_to_string(root.join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let listed = super::changed::git(root, &["ls-files", "--cached", "--others", "--exclude-standard", "-z"])?;
    // A file removed from the working tree but not yet from the index doesn't need a row.
    let files: Vec<&str> = listed.split('\0').filter(|f| !f.is_empty() && root.join(f).is_file()).collect();
    let missing = uncovered(&files, &attribution);
    if missing.is_empty() {
        println!("attribution: ok");
        return Ok(());
    }
    Err(format!(
        "{}\nattribution: the files above are images or fonts with no row in ATTRIBUTION.md. Add a row \
         that names each one (or its directory, or a pattern like `dir/*.png`) in backticks, with its author or source and its licence",
        missing.iter().map(|f| format!("{f}: no row in ATTRIBUTION.md")).collect::<Vec<_>>().join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "| Icon (`assets/logo.png`) | us | MIT |\n\
        | Fonts (`assets/fonts/*.otf`) | a foundry | OFL |\n\
        | Spaced (`assets/fonts/My Font.ttf`) | a foundry | OFL |\n\
        | Top level (`icons/`) | us | MIT |\n\
        | Test corpus (`crates/map/testdata/generated/`: styles and sample data) | us | MIT |\n\
        | Test images (`crates/x/tests/a.png`, `b.png`) | us | MIT |\n\
        | Root logo (`logo.svg`) | us | MIT |\n\
        | Prose (`cargo xtask notices`, `tl!(\"x\")`, `/etc/hosts`) | n/a | n/a |\n";

    #[test]
    fn a_file_a_row_names_is_covered() {
        let files = [
            "assets/logo.png",
            "assets/fonts/hack.otf",
            "assets/fonts/My Font.ttf",
            "icons/a.svg",
            "icons/deep/b.png",
            "crates/map/testdata/generated/data/marker.png",
            "crates/x/tests/a.png",
        ];
        assert_eq!(uncovered(&files, TABLE), Vec::<&str>::new());
    }

    #[test]
    fn an_image_or_font_without_a_row_is_found() {
        let files =
            ["assets/new-logo.png", "assets/fonts/hack.ttf", "assets/icon.SVG", "crates/map/testdata/other/marker.png", "docs/shot.webp"];
        assert_eq!(uncovered(&files, TABLE), files.to_vec());
    }

    #[test]
    fn a_bare_file_name_covers_only_a_file_at_the_root() {
        // `b.png` and `logo.svg` are named in rows: they cover the root's files, not same-named ones below.
        let files = ["crates/y/tests/b.png", "b.png", "logo.svg", "assets/logo.svg", "other.svg"];
        assert_eq!(uncovered(&files, TABLE), vec!["crates/y/tests/b.png", "assets/logo.svg", "other.svg"]);
    }

    #[test]
    fn other_files_and_lookalike_directories_are_not_asset_files() {
        let files = [
            "crates/data/src/lib.rs",
            "crates/data/tests/data.geojson",
            "README.md",
            "crates/map/testdata/generated-more/x.png",
            "iconsx/a.png",
        ];
        assert_eq!(uncovered(&files, TABLE), vec!["crates/map/testdata/generated-more/x.png", "iconsx/a.png"]);
    }

    #[test]
    fn a_pattern_matches_the_whole_path() {
        assert!(wildcard("a/*.png", "a/b/c.png"));
        assert!(wildcard("a/b*", "a/b"));
        assert!(!wildcard("a/*.png", "a/b.png.bak"));
        assert!(!wildcard("a/*.png", "b/a/c.png"));
        assert!(wildcard("x*y*z", "x1y2z"));
        assert!(!wildcard("x*y*z", "x1z"));
        assert!(!wildcard("ab*b", "ab"));
    }
}
