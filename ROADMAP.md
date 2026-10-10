# Roadmap

Ribbon feature coverage: **56 of 300 features (18%)** have a command
today ([docs/parity.md](docs/parity.md)). Counting depth (no layouts, 3D, raster or enterprise
sharing yet), real parity is lower.

## Done (M0–M1)
- Layered workspace, command registry with undo, project files (`.rgisx`), CLI.
- Vector data: GeoJSON, CSV (lon/lat), Shapefile in; GeoJSON, CSV out.
- Map: Web Mercator view, five basemaps, graticule, bookmarks, extent history, Go To XY.
- Symbology: single, unique values, graduated colours (Jenks/equal/quantile), proportional
  symbols; eight colour ramps; transparency; scale ranges; definition queries.
- Labeling with collision-free placement and halos.
- Selection: click, box, by attributes (SQL), by location; attribute table synced with the map.
- Editing: create points/lines/polygons, delete, edit attributes, add/delete/calculate fields.
- Geoprocessing: Buffer, Clip, Erase, Dissolve, Minimum Bounding Geometry, Feature To Point,
  Simplify, Select By Location/Attribute, Summarize Within, Calculate Geometry, Summary
  Statistics, Export Features; history.
- Measure distance and area (geodesic); identify pop-ups; PNG export with scale bar.

## Next
- M2 Layouts: page layout view, map frames, legend, north arrow, scale bar, PDF export (krilla).
- M3 Data: GeoPackage and FlatGeobuf, coordinate systems (PROJ-free transforms for common
  CRSs), raster layers (GeoTIFF), XYZ tile basemaps behind an opt-in network setting.
- M4 Editing: vertex editing, move/rotate, snapping, attribute rules.
- M5 Analysis: spatial join, intersect/union, near, heat maps, ModelBuilder-style chains.
- M6 Charts and reports; MCP server for agents (as in WordCraft).
