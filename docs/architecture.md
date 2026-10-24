# Architecture

RustGIS is a layered Cargo workspace. Lower layers never depend on the UI, which keeps the
engine scriptable and the front end swappable.

| Layer | Crate | Job |
| --- | --- | --- |
| L0 | `rustgis-geom` | Coordinates, extents, Web Mercator, geodesic distance/area, scale |
| L1 | `rustgis-data` | Features, typed attributes, vector and spreadsheet I/O, SQL where clauses, LiDAR, rasters and EPSG reprojection; indexed file and remote data providers; live PostGIS, MySQL/MariaDB and SQL Server providers on the desktop; optional GDAL and DuckDB programs (see [Data formats](user-guide/data-formats.md)) |
| L2 | `rustgis-map`, `rustgis-analysis` | Layers, symbology, labeling, camera, basemaps and web tile layers; geoprocessing tools |
| L2 | `rustgis-tiles` | Web map tiles: XYZ/TMS/WMS raster tiles, Mapbox Vector Tiles, PMTiles and MBTiles archives and a subset of MapLibre styles; a cache that fetches in the background (worker threads natively, the browser's `fetch` on the web) |
| L3 | `rustgis-render` | tiny-skia renderer, label placement, font discovery, map and layout export, and UI-independent GPU scene preparation |
| L3 | `rustgis-geolibre` | The [GeoLibre tools](user-guide/analysis.md#geolibre-tools) as geoprocessing tools: run by the optional `geolibre` program natively, by geolibre-rust's WASI runner in the browser |
| L3 | `rustgis-plugins` | [Plugins](plugins.md): sandboxed WebAssembly modules, desktop process plugins and assistant skills packages; `.rgplugin` packages and the plugin registry |
| L4 | `rustgis-engine` | Session, undo, command registry, projects, sample project |
| L5 | `rustgis-assistant` | The [AI Assistant](user-guide/assistant.md): provider request/response formats, conversation loop, command tools, skills and agent integration; native HTTP transport and local agent/MCP connections, with browser I/O supplied by the web app |
| L6 | `rustgis-ui-egui` | Ribbon, panes, map view, dialogs and the optional wgpu map backend |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web`, `rustgis-python` | Desktop app, command line, browser (WebAssembly), and the PyO3-backed `rustgis` Python package |
| tooling | `xtask` | Workspace checks, third-party licence notices and web service-worker generation |

## Everything is a command

Engine actions are `CommandSpec`s with an id, label, ribbon location, shortcut, parameter
description, an `enabled` check and a `run` function. The ribbon, keyboard shortcuts, command
search, CLI and Python dispatch engine commands through `Session::run`. UI-only commands
(`ui.*`) live in `crates/ui-egui/src/lib.rs`; engine commands never open dialogs.

Mutating commands snapshot the project for undo and restore it on an error or escaped panic.
View-only commands do not take undo snapshots. Undo and rollback cover project state, not
external effects such as files already written.

Plugin commands (`plugin:<plugin>/<command>`) dispatch through `Session::run` too; their project
changes form one undo step. WebAssembly plugins reach the session through permission-checked
requests. Process plugins use the same host protocol but are not sandboxed (see [Plugins](plugins.md)).

## Never crash

Production code forbids unsafe Rust. Library crates deny Clippy's unwrap, expect, panic,
unimplemented, todo and unreachable lints. Readers treat file contents and command parameters as hostile.
`Session::run` and `analysis::run` catch escaped panics as a last resort. Hostile-parameter tests
exercise engine commands and built-in geoprocessing tools; some file-writing and network
commands are skipped.

## Secrets

`Services::secrets` is the only way to the OS credential store. The desktop app implements it
with the login Keychain on macOS, Windows Credential Manager (persistence Local), and the Secret
Service (GNOME Keyring, KWallet) over pure-Rust D-Bus on Linux and the BSDs. It is `None` in the
browser and the CLI, where callers keep a secret for the session only, as before. Crates below
`apps/` use only the `rustgis_data::secrets` trait and never an OS API.

- **Keys.** The service is `RustGIS`. The account is `owner/name` with a lowercase owner (for
  example `assistant/<provider>` or `connection/<id>`); each feature defines its own. Values are
  opaque strings in `Secret`.
- **Errors, not fallbacks.** A missing or locked store, or a refused prompt, returns a
  `SecretError` to show the user. Never write the secret anywhere else instead.
- **No leaks.** `Secret`'s `Debug` is redacted and it has no `Display`. Never log a secret, and
  never put one in command parameters, History, workspace recordings or assistant tool output.
- **Blocking.** A call can wait on an OS prompt or D-Bus. Make it on a user action or a worker
  thread, never every frame, and keep the value in memory for the session. Don't access one
  account from two threads at once.
  The Assistant queues legacy API-key migrations once at startup and serializes its keychain
  reads, writes and deletions on a worker. An existing keychain key takes precedence over a
  legacy preference. The preference is removed only after a successful read of that key or a
  successful migration write; new API keys are never written to desktop preferences.
- **macOS.** The system asks again for access when the app's code signature changes, which
  happens with unsigned or re-signed builds.
- **Tests.** Use `MemorySecrets`, and `MemorySecrets::failing(..)` for error paths. The desktop
  backend's smoke test is `#[ignore]`d: `cargo test -p rustgis keychain_round_trip -- --ignored`.

## Quality gates

```sh
cargo xtask ci        # fmt, clippy -D warnings, tests, layering check
```

`cargo xtask layers` checks that workspace dependencies point to lower layers and that crates
below L6 do not directly depend on UI crates such as egui, eframe, rfd, wgpu or winit.
The browser build is a separate CI gate:

```sh
cargo build --locked --target wasm32-unknown-unknown -p rustgis-web
```

Keep platform-specific I/O behind native target gates or the UI's `Services` callbacks. The web
app supplies asynchronous file pickers, downloads, HTTP requests and the GeoLibre WASI runner.
See [Contributing](contributing.md) for the remaining CI gates and developer tasks.
