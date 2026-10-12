# Roadmap

Every feature on the ribbon checklist (`crates/engine/src/catalog.rs`, printed by
`rustgis-cli parity`) has a command today. Many are first versions: there are no full 3D scenes
or enterprise portal sharing yet, so real depth is lower than the checklist suggests.

## Done
- Layered workspace, command registry with undo, project files (`.rgis`), `rustgis-cli`, the
  web app (WebAssembly), 15 interface languages.
- Data: GeoJSON, CSV (lon/lat), Shapefile, GeoPackage, GeoParquet and FlatGeobuf in (large files
  read in place, a view at a time), and the vector formats GDAL reads when it is installed;
  GeoJSON, CSV, FlatGeobuf, KML, GPX and WKT out; standalone tables; layer, map, style, template,
  task and package files.
- Rasters: GeoTIFF/COG and ASCII grids, EPSG reprojection, stretch/RGB/classified/shaded-relief
  symbology, raster functions, indices, change detection, classification, georeferencing.
- Maps: multiple maps, a 3D globe, projection presets and EPSG codes, bookmarks, web tile
  layers (XYZ/TMS/WMS, Mapbox Vector Tiles, MapLibre styles), worldviews.
- Cartography: single, unique values, graduated colours and proportional symbols; labels with
  placement and annotation; masking, display filters, aggregation, extrusion; graphics.
- Editing: create, move, rotate, scale, merge, split, explode; snapping, templates, elevation,
  topology; domains, subtypes, attribute rules, contingent values, relationships, archiving.
- Analysis: selection by attributes and location; overlay, proximity and summary tools; guided
  workflows (viewshed, line of sight, IDW, shortest path, suitability); ModelBuilder and
  scripts; the GeoLibre and Whitebox suite (1,000+ tools) natively and in the browser.
- Layouts (map frames, surrounds, map series, PDF), reports, presentations and notebooks.
- Sharing as open formats (web maps, OGC API Features collections, MapLibre styles, glTF
  scenes, locators), offline areas with sync, deep learning pixel classification, hosted
  imagery tiles.
- The AI Assistant (Anthropic, OpenAI, Gemini, OpenRouter, Ollama, OpenAI-compatible) in the
  app, the web app and `rustgis-cli ask`.
- Sandboxed WebAssembly plugins with a registry at plugins.rustgis.app.
- Desktop packages: Windows installers (setup.exe, .msi), a portable zip and a Store MSIX; a
  signed and notarized macOS DMG; Linux .deb, .rpm, AppImage and tarballs; a download mirror at
  downloads.rustgis.app.

## Next
- Data: GeoPackage and GeoParquet out; Shapefile out; Shapefiles and GeoPackages in the web app.
- Editing: vertex editing (move, add and delete vertices of existing features).
- Analysis: spatial join, intersect and union, near, heat maps.
- 3D: full scenes with terrain, beyond the globe and 2.5D extrusion.
- Agents: an MCP server exposing RustGIS commands.
