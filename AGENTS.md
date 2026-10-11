# RustGIS — instructions for agents

RustGIS is an open-source, pure-Rust desktop GIS with a ribbon workflow. Its architecture:
layered crates, everything is a command, a thin egui front end, headless screenshots for UI
work.

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
| L1 | `data` | features, typed attributes, GeoJSON/CSV/Shapefile I/O, SQL where clauses, rasters (GeoTIFF/ASCII grid, EPSG reprojection) |
| L2 | `map`, `analysis` | layers, symbology, labels, camera, basemaps; geoprocessing tools |
| L3 | `render` | tiny-skia map renderer, label placement, PNG export, font discovery |
| L3 | `plugins` | plugins: sandboxed WebAssembly modules (`wasmi`) adding commands and tools; `.rgplugin` packages, the plugin registry (see `docs/plugins.md`) |
| L3 | `geolibre` | the GeoLibre/Whitebox tool suite as geoprocessing tools (engine feature `geolibre`): run by the optional `geolibre` program natively, by geolibre-rust's WASI runner in the browser |
| L4 | `engine` | `Session`, undo, command registry, ribbon feature catalog, sample project |
| L6 | `ui-egui` | ribbon, panes, map view, attribute table, dialogs (swappable) |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web` | desktop app, command line, browser (WebAssembly) |

- **Everything is a command**: a `CommandSpec` in `crates/engine/src/cmd/*.rs`. UI-only
  commands (`ui.*`) live in `crates/ui-egui/src/lib.rs`. The ribbon, shortcuts, command search
  and CLI dispatch by id. Commands never open dialogs.
- **Never break wasm**: everything below `apps/` must build for `wasm32-unknown-unknown`
  (CI runs `cargo build --target wasm32-unknown-unknown -p rustgis-web`). Use `web_time`, not
  `std::time`; file access goes through `Services` on the web. `rustgis-geolibre` builds for
  both: natively it runs the `geolibre` program; in the browser the web app runs geolibre-rust's
  WASI runner asynchronously (`external_start` → `Services::geolibre_run` → `gp.finish`).
- **Interface text is translatable**: wrap user-visible literals in `tl!("…")`, use
  `i18n::fmt("… {name} …", &[("name", …)])` instead of `format!` for messages, and pass labels to
  the ribbon widgets (they translate). Then add the new strings to every catalog in
  `crates/ui-egui/src/i18n/*.tsv` (format in `docs/i18n.md`). Command ids, data and the CLI are
  never translated.
- **Layering** is enforced by `cargo xtask layers`: nothing below L6 depends on egui/eframe/rfd.
- **Coverage** is measured against a ribbon feature checklist (`crates/engine/src/catalog.rs`,
  `cargo xtask parity` → `docs/parity.md`). A test enforces a floor that only rises.

## Assets
- No third-party vendor icons, images, styles, fonts, help text or code. Icons are drawn in
  code (`icons.rs`). Every asset file has a row in `ATTRIBUTION.md`.
- Screenshots and other images are not committed here: they live in
  [opengeos/rustgis-assets](https://github.com/opengeos/rustgis-assets) (WebP, served at
  `https://assets.rustgis.app/images/…`) and docs link to them by URL.

## Quality gates
`cargo xtask ci` (fmt, clippy -D warnings, tests, layers) before every commit.

## Looking at the app
- `cargo run --release -p rustgis` (opens the World Atlas sample).
- Headless screenshots: `cargo run --release -p rustgis-ui-egui --example ui_shot -- script.jsonl`
  (see the example header). For UI work, look at the PNG.
- CLI: `rustgis-cli run --sample --cmd 'gp.run={"tool":"buffer","params":{"input":"Cities","distance":100}}' --cmd 'share.exportMap={"path":"out.png"}'`.
