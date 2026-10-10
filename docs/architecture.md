# Architecture

RustGIS is a layered Cargo workspace. Lower layers never depend on the UI, which keeps the
engine scriptable and the front end swappable.

| Layer | Crate | Job |
| --- | --- | --- |
| L0 | `rustgis-geom` | Coordinates, extents, Web Mercator, geodesic distance/area, scale |
| L1 | `rustgis-data` | Features and typed attributes; GeoJSON, CSV, Shapefile; SQL where clauses; rasters (GeoTIFF, ASCII grid) and their coordinate systems |
| L2 | `rustgis-map`, `rustgis-analysis` | Layers, symbology, labeling, camera, basemaps; geoprocessing tools |
| L3 | `rustgis-render` | tiny-skia renderer, label placement, PNG export |
| L4 | `rustgis-engine` | Session, undo, command registry, feature catalog, sample project |
| L6 | `rustgis-ui-egui` | The ribbon front end |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web` | Desktop app, command line, browser (WebAssembly) |

## Everything is a command

Each user-visible action is a `CommandSpec` with an id, label, ribbon location, shortcut,
parameter description, an `enabled` check and a `run` function. The ribbon, keyboard shortcuts,
command search and CLI all dispatch through `Session::run`, which also snapshots the project
for undo and restores it if a command fails.

## Never crash

Production code avoids `unwrap`, `expect` and panics, and treats file contents and command
parameters as hostile. Tests run every command and every geoprocessing tool with junk
parameters.

## Quality gates

```sh
cargo xtask ci        # fmt, clippy -D warnings, tests, layering check
cargo xtask parity    # regenerate the feature coverage report
```
