# RustGIS — instructions for agents

RustGIS is an open-source, pure-Rust desktop GIS with a ribbon workflow. Its
architecture follows `../storytold/wordcraft`: layered crates, everything is a command, a thin
egui front end, headless screenshots for UI work.

## Never crash
- No `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!` in non-test code; no `unsafe`
  (`unsafe_code = "forbid"`). Crate roots carry the `#![deny(clippy::unwrap_used, …)]` block.
- Input from files and command parameters is hostile: `get()` over indexing, clamp sizes,
  bound recursion. `Session::run` and `analysis::run` catch escaped panics as a last resort.
- `crates/engine/src/tests.rs::hostile_params_never_panic` and the analysis equivalent run every
  command/tool with junk parameters. Keep them green; every crash fix gets a regression test.

## Architecture
| Layer | Crate | Job |
|---|---|---|
| L0 | `geom` | coordinates, extents, Web Mercator, geodesic measurement, scale |
| L1 | `data` | features, typed attributes, GeoJSON/CSV/Shapefile I/O, SQL where clauses |
| L2 | `map`, `analysis` | layers, symbology, labels, camera, basemaps; geoprocessing tools |
| L3 | `render` | tiny-skia map renderer, label placement, PNG export, font discovery |
| L4 | `engine` | `Session`, undo, command registry, ribbon feature catalog, sample project |
| L6 | `ui-egui` | ribbon, panes, map view, attribute table, dialogs (swappable) |
| apps | `rustgis`, `rustgis-cli` | desktop app, command line |

- **Everything is a command**: a `CommandSpec` in `crates/engine/src/cmd/*.rs`. UI-only
  commands (`ui.*`) live in `crates/ui-egui/src/lib.rs`. The ribbon, shortcuts, command search
  and CLI dispatch by id. Commands never open dialogs.
- **Layering** is enforced by `cargo xtask layers`: nothing below L6 depends on egui/eframe/rfd.
- **Coverage** is measured against a ribbon feature checklist (`crates/engine/src/catalog.rs`,
  `cargo xtask parity` → `docs/parity.md`). A test enforces a floor that only rises.

## Assets
- No third-party vendor icons, images, styles, fonts, help text or code. Icons are drawn in
  code (`icons.rs`). Every asset file has a row in `ATTRIBUTION.md`.

## Quality gates
`cargo xtask ci` (fmt, clippy -D warnings, tests, layers) before every commit.

## Looking at the app
- `cargo run --release -p rustgis` (opens the World Atlas sample).
- Headless screenshots: `cargo run --release -p rustgis-ui-egui --example ui_shot -- script.jsonl`
  (see the example header). For UI work, look at the PNG.
- CLI: `rustgis-cli run --sample --cmd 'gp.run={"tool":"buffer","params":{"input":"Cities","distance":100}}' --cmd 'share.exportMap={"path":"out.png"}'`.
