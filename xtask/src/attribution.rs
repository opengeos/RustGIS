//! `cargo xtask attribution`: every image and font file in the repository has a row in
//! `ATTRIBUTION.md`.
//!
//! AGENTS.md says every asset file has a row. The table also lists test data and fixtures, but
//! not by any rule that a program could apply to every file, so this checks the classes the rule
//! names: images, icons and fonts. A file is covered when a backticked path in `ATTRIBUTION.md`
//! names it, names a directory above it, matches it as a pattern (`*` stands for any characters,
//! `/` included), or, with no `/`, names it or matches it by its file name alone.

use std::path::Path;

/// The extensions of image, icon and font files.
const ASSET_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns", "svg", "ttf", "otf", "ttc", "woff", "woff2"];

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
    at.ends_with(last) && at.len() >= last.len()
}

/// The paths and patterns `ATTRIBUTION.md` names: everything in backticks that looks like a file
/// or a directory, with a trailing `/` taken off.
fn named(attribution: &str) -> Vec<&str> {
    attribution
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| !t.is_empty() && !t.contains(char::is_whitespace) && (t.contains('/') || t.contains('.')))
        .map(|t| t.trim_end_matches('/'))
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
            let name = f.rsplit('/').next().unwrap_or(f);
            !named.iter().any(|n| if n.contains('/') { f.starts_with(&format!("{n}/")) || wildcard(n, f) } else { wildcard(n, name) })
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
        | Test corpus (`crates/map/testdata/generated/`: styles and sample data) | us | MIT |\n\
        | Test images (`crates/x/tests/a.png`, `b.png`) | us | MIT |\n";

    #[test]
    fn a_file_a_row_names_is_covered() {
        let files = [
            "assets/logo.png",
            "assets/fonts/hack.otf",
            "crates/map/testdata/generated/data/marker.png",
            "crates/x/tests/a.png",
            "crates/y/tests/b.png",
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
    fn other_files_and_lookalike_directories_are_not_asset_files() {
        let files = ["crates/data/src/lib.rs", "crates/data/tests/data.geojson", "README.md", "crates/map/testdata/generated-more/x.png"];
        assert_eq!(uncovered(&files, TABLE), vec!["crates/map/testdata/generated-more/x.png"]);
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
