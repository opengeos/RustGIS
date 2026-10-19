# RustGIS

**A fast, free and open-source GIS in pure Rust for mapping, analyzing and sharing geospatial
data.**

RustGIS runs natively on Windows, macOS and Linux, and in your web browser. Your data stays on
your machine unless you send it to an AI model you choose, and every action is a command you can
script from the command line, a notebook or the AI Assistant.

<div class="hero-actions" markdown>

[Try it in your browser](https://web.rustgis.app){ .md-button .md-button--primary }
[Download](installation.md){ .md-button }
[Get started](getting-started.md){ .md-button }
[Compare](comparison/index.md){ .md-button }
[![Get it from Microsoft](https://get.microsoft.com/images/en-us%20dark.svg)](https://apps.microsoft.com/detail/9ppjw8h561lq?mode=direct)

</div>

<figure markdown="span">
  [![RustGIS with the Home tab of the ribbon open over a world map in the Equal Earth projection, countries shaded by population](https://assets.rustgis.app/images/hero.webp)](https://assets.rustgis.app/images/hero.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption>RustGIS with the World Atlas sample in the Equal Earth projection.</figcaption>
</figure>

## Highlights

<div class="grid cards" markdown>

-   **Your data, any source**

    GeoJSON, Shapefile, GeoPackage, GeoParquet, GeoTIFF and more, live PostGIS and MySQL
    databases, and public data from STAC catalogs, Overture Maps and open-data portals.
    [Data](user-guide/data.md) · [Web Data](user-guide/web-data.md) · [Formats](user-guide/data-formats.md)

-   **Cartography and 3D**

    Graduated colours, collision-free labels, page layouts and map series; a 3D globe, any
    projection, terrain and extrusions, and maps of the Moon, Mars and other worlds.
    [Cartography](user-guide/cartography.md) · [Maps](user-guide/maps.md) · [Layouts](user-guide/layouts.md)

-   **Analysis**

    SQL selection, overlay and proximity tools, guided workflows, ModelBuilder, imagery tools and
    over 1,000 GeoLibre and Whitebox tools for terrain, hydrology and LiDAR.
    [Analysis](user-guide/analysis.md)

-   **Editing**

    Create features and edit their vertices with snapping and templates, keep attributes valid
    with domains, subtypes and rules, check topology, and undo any change.
    [Editing](user-guide/editing.md)

-   **AI Assistant**

    Ask about your data or tell the map what to do. The assistant runs RustGIS commands with the
    model you choose, asks before going online, and every change can be undone.
    [AI Assistant](user-guide/assistant.md)

-   **Scriptable and extensible**

    `rustgis-cli` and the `rustgis` Python package run any command and render maps; the MCP
    server lets AI agents make maps; sandboxed WebAssembly plugins add tools.
    [Command Line](cli.md) · [Python](python.md) · [MCP](mcp.md) · [Plugins](plugins.md)

</div>

## A tour

<div class="grid" markdown>

<figure markdown="span">
  [![The Style tab and the Symbology pane, with the larger African countries selected](https://assets.rustgis.app/images/symbology-table.webp)](https://assets.rustgis.app/images/symbology-table.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Symbology and tables.</strong> Selecting a layer opens its contextual tabs: symbology, effects, labeling and data. Selection is synced between the map and the attribute table.</figcaption>
</figure>

<figure markdown="span">
  [![The AI Assistant selecting the five most populous African countries](https://assets.rustgis.app/images/assistant.webp)](https://assets.rustgis.app/images/assistant.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>AI Assistant.</strong> Ask about your data or tell the map what to do. The assistant runs RustGIS commands for you, and each change can be undone.</figcaption>
</figure>

<figure markdown="span">
  [![The Analyze tab's tool gallery and dissolved buffers around African cities](https://assets.rustgis.app/images/buffer-history.webp)](https://assets.rustgis.app/images/buffer-history.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Analysis.</strong> A tool gallery, the Tools pane and a run history, guided workflows, ModelBuilder, and 1,000+ GeoLibre and Whitebox tools.</figcaption>
</figure>

<figure markdown="span">
  [![ModelBuilder: a model of buffer, clip and dissolve after a run](https://assets.rustgis.app/images/modelbuilder.webp)](https://assets.rustgis.app/images/modelbuilder.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>ModelBuilder.</strong> Chain tools on a node canvas of inputs, tools and outputs, check the model as you build, and run it in one go.</figcaption>
</figure>

<figure markdown="span">
  [![A shaded-relief elevation model with the Style tab](https://assets.rustgis.app/images/raster.webp)](https://assets.rustgis.app/images/raster.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Rasters.</strong> GeoTIFF and COG of any pixel type, reprojected on the fly, with stretch, RGB, classified and shaded-relief symbology.</figcaption>
</figure>

<figure markdown="span">
  [![The World Atlas on the 3D globe](https://assets.rustgis.app/images/globe.webp)](https://assets.rustgis.app/images/globe.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Globe and projections.</strong> A 3D globe, Equal Earth, Robinson, polar and regional presets, or any EPSG code.</figcaption>
</figure>

<figure markdown="span">
  [![Mount Rainier in USGS imagery draped over terrain, tilted](https://assets.rustgis.app/images/terrain.webp)](https://assets.rustgis.app/images/terrain.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Terrain.</strong> Tilt and turn the map, and drape it over a DEM or elevation tiles.</figcaption>
</figure>

<figure markdown="span">
  [![Mars on the 3D globe, on the Viking mosaic](https://assets.rustgis.app/images/mars-globe.webp)](https://assets.rustgis.app/images/mars-globe.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Other worlds.</strong> Map the Moon, Mars and nine other bodies on their own basemaps, measured with their own radius.</figcaption>
</figure>

<figure markdown="span">
  [![Countries extruded by population on a tilted map](https://assets.rustgis.app/images/extrusion.webp)](https://assets.rustgis.app/images/extrusion.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Extrusion.</strong> Stand features up by a field or a height, lit and shaded, on a tilted map.</figcaption>
</figure>

<figure markdown="span">
  [![An OpenFreeMap vector style over Paris with the Basemap gallery](https://assets.rustgis.app/images/tiles.webp)](https://assets.rustgis.app/images/tiles.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Web basemaps.</strong> XYZ, TMS and WMS tiles, Mapbox Vector Tiles and MapLibre styles, on the flat map and the globe.</figcaption>
</figure>

<figure markdown="span">
  [![The Sentinel-2 Explorer in the Web Data pane, with scene footprints on the map](https://assets.rustgis.app/images/web-data.webp)](https://assets.rustgis.app/images/web-data.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Web Data.</strong> STAC catalogs, Overture Maps, OpenStreetMap, NASA Earthdata, open-data portals and satellite embeddings, searched where the map is.</figcaption>
</figure>

<figure markdown="span">
  [![The Time Slider showing dated events through 2018](https://assets.rustgis.app/images/time-slider.webp)](https://assets.rustgis.app/images/time-slider.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Time Slider.</strong> Step dated features and dated imagery through time, and export the steps as a GIF.</figcaption>
</figure>

<figure markdown="span">
  [![A page layout with a map frame, legend, north arrow and scale bar](https://assets.rustgis.app/images/layout.webp)](https://assets.rustgis.app/images/layout.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Layouts.</strong> Map frames, legends, scale bars, grids, map series and PDF export, plus reports and presentations.</figcaption>
</figure>

<figure markdown="span">
  [![The Plugin Manager browsing the plugin registry](https://assets.rustgis.app/images/plugins.webp)](https://assets.rustgis.app/images/plugins.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Plugins.</strong> Browse and install sandboxed plugins from <a href="https://plugins.rustgis.app">plugins.rustgis.app</a>, each showing the permissions it asks for.</figcaption>
</figure>

<figure markdown="span">
  [![Dark mode over Europe on the Midnight basemap](https://assets.rustgis.app/images/dark.webp)](https://assets.rustgis.app/images/dark.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Dark mode</strong> and the View tab: effects, scene settings, panes and linked views.</figcaption>
</figure>

<figure markdown="span">
  [![The Project backstage](https://assets.rustgis.app/images/backstage.webp)](https://assets.rustgis.app/images/backstage.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
  <figcaption><strong>Projects.</strong> Start blank, from the World Atlas sample, or open a <code>.rgis</code> project.</figcaption>
</figure>

</div>
