<p align="center">
  <img src="https://assets.rustgis.app/images/logo.svg" alt="RustGIS logo: a rust-coloured cog framing a segmented globe with an orbit" width="160">
</p>

<h1 align="center">RustGIS</h1>

<p align="center">
  <b>A fast, open-source desktop GIS in pure Rust, with a familiar ribbon workflow.</b><br>
  Maps and layers, symbology, labeling, selection, editing, geoprocessing and export: native
  egui on the GPU, one codebase for Windows, macOS and Linux, and every action scriptable.
</p>

<p align="center">
  <img alt="Written in Rust" src="https://img.shields.io/badge/written%20in-Rust-b84a2c?style=flat-square&logo=rust&logoColor=white">
  <img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-2563eb?style=flat-square">
  <img alt="Ribbon coverage" src="https://img.shields.io/badge/ribbon%20coverage-18%25-0e9384?style=flat-square">
</p>

<p align="center">
  <a href="https://web.rustgis.app"><img alt="Launch the RustGIS web app" src="https://img.shields.io/badge/Launch-web%20app-b84a2c?style=for-the-badge&logo=webassembly&logoColor=white" height="36"></a>
</p>

<p align="center">
  <img src="https://assets.rustgis.app/images/hero.webp" alt="RustGIS with the Map tab of the ribbon open. The Contents pane lists Cities, Rivers, Lakes and Countries with their legends; the map shows countries shaded by population, sized city dots and country labels." width="100%">
</p>

## A tour

<table>
<tr>
<td width="50%" valign="top"><img src="https://assets.rustgis.app/images/symbology-table.webp" alt="The Feature Layer contextual tab with the Symbology pane showing graduated colours and colour schemes; African countries are selected in cyan on the map and highlighted in the attribute table below" width="100%"><p align="center"><sub><b>Feature Layer tabs.</b> A contextual tab group appears when a layer is selected: symbology, visibility range, transparency, labeling and data. Selection is synced between the map and the attribute table.</sub></p></td>
<td width="50%" valign="top"><img src="https://assets.rustgis.app/images/buffer-history.webp" alt="The Analysis tab with a gallery of geoprocessing tools; 300 km dissolved buffers around African cities drawn semi-transparent, and the History pane listing the Buffer run" width="100%"><p align="center"><sub><b>Analysis.</b> A tool gallery and Geoprocessing pane: buffer, clip, erase, dissolve, summarize within, statistics and more, with a run history.</sub></p></td>
</tr>
<tr>
<td width="50%" valign="top"><img src="https://assets.rustgis.app/images/dark.webp" alt="RustGIS in dark mode with the View tab open over a map of Europe on the Midnight basemap, cities labelled with halos" width="100%"><p align="center"><sub><b>Dark mode</b> and the View tab: panes, graticule, basemaps.</sub></p></td>
<td width="50%" valign="top"><img src="https://assets.rustgis.app/images/backstage.webp" alt="The Project backstage with cards to start a blank map, the World Atlas sample, or open a project" width="100%"><p align="center"><sub><b>Project.</b> Start blank, from the World Atlas sample, or open a <code>.rgisx</code> project.</sub></p></td>
</tr>
</table>

Every screenshot is RustGIS itself, rendered offscreen by `cargo run -p rustgis-ui-egui --example ui_shot`.

## What works today

| Area | Highlights |
|---|---|
| **Data** | GeoJSON, CSV with longitude/latitude, Shapefile in; GeoJSON and CSV out; standalone tables |
| **Map** | Smooth pan/zoom, five basemaps, graticule, bookmarks, previous/next extent, Go To XY, scale box, multiple maps per project |
| **Symbology** | Single symbol, unique values, graduated colours (natural breaks, equal interval, quantile), proportional symbols, eight colour schemes, marker shapes, outlines, transparency, visibility ranges |
| **Labeling** | Per-layer labels with field, size, colour, bold and halo; collision-free placement |
| **Selection** | Click and box select (add/remove), Select By Attributes with live SQL validation, Select By Location, switch/clear/zoom to selection, layer from selection |
| **Editing** | Create points, lines and polygons; delete; edit cells in the attribute table; add, delete and calculate fields; undo/redo for everything |
| **Analysis** | Buffer, Clip, Erase, Dissolve, Minimum Bounding Geometry, Feature To Point, Simplify, Summarize Within, Calculate Geometry, Summary Statistics, Export Features |
| **Inquiry** | Identify pop-ups, geodesic distance and area measurement, live coordinates (DD or DMS) |
| **Share** | PNG export with labels and a scale bar at any DPI; export layers to GeoJSON/CSV |
| **Productivity** | Command search (Ctrl+K), keyboard shortcuts, dark mode, Catalog pane with folder browsing |
| **Languages** | The interface in 15 languages (English, German, Spanish, French, Italian, Dutch, Portuguese, Russian, Turkish, Azerbaijani, Indonesian, Vietnamese, Chinese, Japanese, Korean), following the system language or chosen under View › Language |

The honest picture is in [ROADMAP.md](ROADMAP.md) and the generated [coverage report](docs/parity.md).

## Try it in the browser

RustGIS also runs as a WebAssembly app at **[web.rustgis.app](https://web.rustgis.app)**: the
same ribbon and tools, with the browser's file picker for GeoJSON, CSV and projects, and
downloads for Save and Export.

## Quick start

```sh
cargo run --release -p rustgis                       # the desktop app with the World Atlas sample
cargo run --release -p rustgis -- data.geojson       # open data (or a .rgisx project)
```

Command line: every ribbon action is a command with an id.

```sh
rustgis-cli commands                                 # all command ids and parameters
rustgis-cli render countries.geojson map.png --basemap ocean
rustgis-cli run --sample \
  --cmd 'gp.run={"tool":"countPoints","params":{"polygons":"Countries","points":"Cities"}}' \
  --cmd 'layer.symbology={"layer":"Countries_Summarize","type":"graduated","field":"point_count"}' \
  --cmd 'share.exportMap={"path":"out.png","dpi":192}' --save atlas.rgisx
rustgis-cli parity                                   # ribbon feature coverage checklist
```

## Architecture

Inspired by [WordCraft](https://github.com/storytold/wordcraft)'s layering:

| Layer | Crate | Job |
|---|---|---|
| L0 | `rustgis-geom` | coordinates, extents, Web Mercator, geodesic distance/area, scale |
| L1 | `rustgis-data` | features and typed attributes; GeoJSON, CSV, Shapefile; SQL where clauses |
| L2 | `rustgis-map`, `rustgis-analysis` | layers, symbology, labeling, camera, basemaps; geoprocessing (on `geo`) |
| L3 | `rustgis-render` | tiny-skia renderer, label placement, PNG export |
| L4 | `rustgis-engine` | session, undo, command registry, parity catalog, sample project |
| L6 | `rustgis-ui-egui` | the ribbon front end (swappable) |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web` | desktop, command line, browser (WebAssembly) |

`cargo xtask ci` runs formatting,
clippy, the tests and the layering check. Contributor and agent notes: [AGENTS.md](AGENTS.md).

## License and credits

Licensed under the [Apache License 2.0](LICENSE). Sample data from [Natural Earth](https://www.naturalearthdata.com/) (public
domain).
