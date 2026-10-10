# RustGIS

**A fast, open-source desktop GIS in pure Rust, with a familiar ribbon workflow.**

Maps and layers, symbology, labeling, selection, editing, geoprocessing and export, in a native
GPU-rendered app for Windows, macOS and Linux. Every action is a command you can also run from
the command line.

![RustGIS with the Map tab of the ribbon open over a world map of countries shaded by population](https://assets.rustgis.app/images/hero.webp)

## Highlights

- **Ribbon workflow**: Map, Insert, Analysis, View, Edit, Share and Help tabs, plus a
  contextual *Feature Layer* tab group (Feature Layer, Labeling, Data) when a layer is selected.
- **Your data**: GeoJSON, CSV with longitude/latitude, and Shapefile in; GeoJSON and CSV out.
- **Cartography**: single symbol, unique values, graduated colours (natural breaks, equal
  interval, quantile) and proportional symbols; eight colour schemes; collision-free labels.
- **Analysis**: Buffer, Clip, Erase, Dissolve, Summarize Within, Summary Statistics and more,
  with a run history.
- **Editing**: create and delete features, edit attributes in the table, add and calculate
  fields, with undo for everything.
- **Scriptable**: `rustgis-cli` runs any command by id, renders maps to PNG and chains tools.

## A tour

<div class="grid" markdown>

![The Feature Layer tab and the Symbology pane, with African countries selected](https://assets.rustgis.app/images/symbology-table.webp)

![The Analysis tab's tool gallery and dissolved buffers around African cities](https://assets.rustgis.app/images/buffer-history.webp)

![Dark mode over Europe on the Midnight basemap](https://assets.rustgis.app/images/dark.webp)

![The Project backstage](https://assets.rustgis.app/images/backstage.webp)

</div>

[Try it in your browser](https://rustgis.app/app/){ .md-button .md-button--primary }
[Get started](getting-started.md){ .md-button }
[Install](installation.md){ .md-button }
