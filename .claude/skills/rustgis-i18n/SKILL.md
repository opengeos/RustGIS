---
name: rustgis-i18n
description: "Audit RustGIS's interface catalogs (crates/ui-egui/src/i18n/*.tsv), translate every missing string in every language, and open a PR. Use for i18n review, untranslated UI strings, adding a language, or after a feature added user-visible text."
---

# RustGIS i18n: audit and fill the catalogs

RustGIS's UI strings are English in the code and **are the lookup keys**: `tl!("Explore")`,
`i18n::fmt("{n} classes", …)`, and labels passed to the ribbon widgets (`big`, `small`, `group`,
`menu_item`, …), which translate internally. Each language has a catalog
`crates/ui-egui/src/i18n/<code>.tsv` of `English<TAB>Translation` lines (`\n`/`\t` escapes, `#`
comments). A string with no entry renders in English, so nothing fails when catalogs fall behind a
feature. That drift is invisible until the audit runs.

Engine strings are keys too: command labels and ribbon locations (`C::new("id", "Label",
"Tab › Group")`), geoprocessing tool names, summaries, toolboxes, parameter labels and choices, and
enum labels (basemaps, colour ramps, classification methods, markers, renderer kinds, tools,
geometry and field types). The extractor finds them all.

Everything goes through one script:

```bash
S=.claude/skills/rustgis-i18n/scripts/i18n.py
python3 $S keys            # every translatable English string
python3 $S audit [-v]      # per-locale coverage, missing/stale keys, placeholder problems
python3 $S missing all -o <scratch>/missing.json
python3 $S insert <scratch>/translations.json [--prune]
```

## Ground rules

- **Never commit to `main`.** Branch, then open a PR. Commit as `giswqs`.
- **No em dashes** in commits, PR bodies or comments.
- Run `pre-commit run --files <changed files>` before pushing (`pre-commit` lives in the user's
  conda env; activate it first and never mention the env in public text).
- The repo is `opengeos/RustGIS`. Report a finished PR as its URL.

## Phase 1: audit

```bash
python3 .claude/skills/rustgis-i18n/scripts/i18n.py audit -v
```

How to read it:

- **missing**: keys with no entry. These are the work.
- **stale**: entries whose English key no longer exists in the code (the string was reworded or
  removed). Re-translate the new wording, then `insert --prune` drops the stale lines.
- **placeholder**: the translation's `{name}` set differs from the English one. Always a bug; fix
  it. The Rust test `i18n::tests::every_catalog_parses_cleanly_and_keeps_placeholders` fails on it.
- **line-break**: the English has a `\n` (ribbon labels split over two lines, e.g. `Add\nData`)
  and the translation has none. Usually wrong for ribbon buttons: keep two short lines.

The extractor is heuristic. If a visible string never shows up in `keys`, it is not wrapped:
wrap it with `tl!()` (or `i18n::fmt` for templates) in the UI code first. `format!()` text is
never looked up, so user-visible messages built with `format!` must move to `i18n::fmt`.

## Phase 2: terminology before translating

Do not translate cold. For each locale, read its existing catalog first and reuse its vocabulary:
the same English word must get the same translation everywhere (Layer, Feature, Field, Basemap,
Symbology, Geoprocessing, Selection, Attribute Table, Buffer, Clip, Dissolve…). Follow the GIS
terms each language's GIS community uses (the vocabulary of QGIS's translation is a good
reference). Keep consistent across:

| Concept | Keep consistent |
|---|---|
| Ribbon tabs | Map, Insert, Analysis, View, Edit, Share, Help, Feature Layer, Labeling, Data, Project |
| Panes | Contents, Catalog, Geoprocessing, Symbology, History, Details |
| Tools | Buffer, Clip, Erase, Dissolve, Summarize Within, Feature To Point, Simplify, … |
| Selection | Select By Attributes / By Location, Clear, Switch, Zoom To |

### Rules that bite

- **Keep every `{placeholder}` exactly** (`{n}`, `{total}`, `{name}`, `{layer}`, …); they may move.
- **Keep `\n` in ribbon button labels** (two short lines, e.g. `Add\nData` → `Daten\nhinzufügen`).
  Ribbon labels must stay short: a long label widens the whole ribbon.
- **Leave untranslated**: RustGIS, GeoJSON, CSV, Shapefile, PNG, SQL, WGS 84, Web Mercator, Natural
  Earth, `.rgisx`, keyboard keys (Ctrl, Shift, Esc), and SQL inside examples
  (`continent = 'Europe'` keeps the field names and SQL keywords; translate only `e.g.`).
- Match each language's quotation marks: `„…“` (de), `«…»` (es/fr/it/ru/pt), `‘…’` (nl),
  `「…」` (ja), `“…”` (zh).
- Language names in the picker are written in their own language and are not catalog entries.

## Phase 3: translate and merge

```bash
python3 .claude/skills/rustgis-i18n/scripts/i18n.py missing all -o <scratch>/missing.json
```

This writes `{code: {english: ""}}`. Fill in the values (fan out one agent per two or three
languages when there are many; give each the rules above and its existing catalog for
terminology), producing `<scratch>/translations.json` in the same shape. Then:

```bash
python3 .claude/skills/rustgis-i18n/scripts/i18n.py insert <scratch>/translations.json --prune
```

`insert` skips values whose placeholders don't match and keys that no longer exist, never
overwrites existing translations (add `--overwrite` to replace them), and keeps each catalog
sorted by key, so diffs stay readable.

## Phase 4: verify

```bash
python3 .claude/skills/rustgis-i18n/scripts/i18n.py audit        # expect 100% everywhere
cargo test -q -p rustgis-ui-egui                                  # catalog parse + placeholder test, UI smoke test
```

Look at the result: render a screenshot in a CJK language and a long-word language (German) with
the headless harness and check that ribbon labels fit:

```bash
printf '{"cmd":"ui.language","params":{"code":"de"}}\n{"shot":"/tmp/de.png"}\n' > /tmp/de.jsonl
cargo run --release -p rustgis-ui-egui --example ui_shot -- /tmp/de.jsonl
```

CJK languages need a CJK font: the desktop app uses an installed one (Noto Sans CJK, Microsoft
YaHei, Yu Gothic, Malgun Gothic, PingFang, Hiragino, Apple SD Gothic Neo); the web app downloads
Noto Sans SC/JP/KR from `https://assets.rustgis.app/fonts/`.

## Phase 5: PR

Branch, commit as `giswqs`, `pre-commit run --files crates/ui-egui/src/i18n/*.tsv`, push,
`gh pr create`. The PR body should give per-locale coverage before and after, and invite
native-speaker corrections: these translations are machine-authored against the catalogs'
existing terminology.

## Adding a language

1. Add `crates/ui-egui/src/i18n/<code>.tsv` with a `#` header line.
2. Add a row to `LANGUAGES` in `crates/ui-egui/src/i18n/mod.rs` (code, native name, English name,
   script). Scripts egui can't yet lay out right to left (Arabic, Persian, Hebrew) or shape well
   (Devanagari, Thai, Georgian) stay out until it can.
3. Run phases 3 and 4 for it.
