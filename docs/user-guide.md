# User Guide

## The ribbon

| Tab | Groups |
| --- | --- |
| **Project** | New, Open, Save, Save As, About (backstage) |
| **Map** | Navigate, Layer (Basemap, Add Data), Selection, Inquiry (Measure, Locate), Labeling |
| **Insert** | New Map, Toolbox, Import Project, Bookmarks, Fields |
| **Analysis** | Geoprocessing (History, Tools), tool gallery, Select By Attributes/Location |
| **View** | Windows (Contents, Catalog, Geoprocessing, Symbology, History, Details), Graticule, Dark Mode |
| **Edit** | Save, Undo/Redo, Create, Delete, Selection, Fields |
| **Share** | Export Map (PNG), Export Features (GeoJSON/CSV), Save Project As |
| **Feature Layer** | Visibility Range, Transparency, Symbology, Color, Outline, Zoom To Layer |
| **Labeling** | Label, Field, Text Symbol (size, colour, bold, halo) |
| **Data** | Definition Query, Attribute Table, Selection, Fields, Export |

The last three form the contextual *Feature Layer* group, shown while a layer is selected in
the Contents pane.

To give the map more room, collapse the ribbon to its tab row with the chevron at its right
end, by double-clicking a tab, or with Ctrl+F1 (⌘F1 on macOS, where F1 may need the Fn key).
Click a tab to peek at its commands while collapsed.

## Data

| Format | Read | Write |
| --- | --- | --- |
| GeoJSON (`.geojson`, `.json`) | Yes | Yes |
| CSV (`.csv`, `.txt`) with longitude/latitude columns | Yes | Yes |
| Shapefile (`.shp` + `.dbf`) | Yes | No |
| GeoTIFF and cloud-optimised GeoTIFF (`.tif`, `.tiff`) | Yes | Yes |
| ASCII grid (`.asc`, with an optional `.prj`) | Yes | No |
| RustGIS project (`.rgisx`) | Yes | Yes |

Vector coordinates are WGS 84 longitude/latitude and are displayed in Web Mercator. Rasters keep
their own coordinate system (any EPSG code, such as UTM zones or national grids) and are
reprojected on the fly as they draw.

## Projections

The flat map is drawn in Web Mercator by default. **View › Projection** switches it to another
projection, and clicking the coordinate system in the status bar opens the **Coordinate
System** dialog, which also takes any projected EPSG code (such as `3035`, `32633` for UTM zone
33N or `2056` for Switzerland):

| Preset | Kind |
| --- | --- |
| Equirectangular, Miller | World, cylindrical |
| Mollweide, Lambert Cylindrical Equal Area, Equal Earth, Eckert IV | World, equal-area |
| Robinson, Natural Earth, Winkel Tripel | World, compromise |
| North / South Polar Stereographic | Polar |
| Europe Equal Area (ETRS89 LAEA), USA Contiguous Albers Equal Area | Regional, equal-area |

Layers are projected on the fly. Each projection has an area of use: polar projections show
their hemisphere poleward of 30°, regional ones their region, transverse Mercator (UTM) zones
30° either side of their meridian, and data outside it isn't drawn. Switching keeps the centre
and scale, and the choice is saved with the map (`map.projection` from the command line). A
raster already in the map's coordinate system draws without being reprojected. Robinson, Equal
Earth, Natural Earth, Winkel Tripel and Eckert IV are computed by RustGIS itself on a sphere,
so EPSG:8857 (Equal Earth) works too.

## Globe

**View › Globe** shows the map as a 3D globe seen from space instead of the flat Web Mercator
map, and back again. Drag to turn the globe, scroll to zoom, and use **Full Extent** to see the
whole Earth. Every layer draws on the globe: vectors are clipped at the horizon and follow great
circles, rasters are draped onto the sphere, and labels, selection, identify, measuring and
editing work as on the flat map. The scale stays the same when you switch, and the choice is
saved with each map in the project (`map.projection` from the command line).

## Rasters

Add a GeoTIFF or an ASCII grid with **Add Data**, by dropping it on the window, or in the web app
by picking it. An ASCII grid takes its coordinate system from a `.prj` file next to it that names an EPSG code;
otherwise, and always in the web app, which only sees the one file, it must be in
longitude/latitude degrees. Exporting a raster opened from an overview writes that overview.
GeoTIFFs may be grey, RGB(A), palette or multiband, of any pixel type, tiled or striped, and
compressed with Deflate, LZW, PackBits or JPEG. A raster too large to hold in memory (more than
120 million samples) opens from its largest overview that fits.

Selecting a raster layer in Contents shows the **Raster Layer** tab group:

| Tab › Group | Controls |
| --- | --- |
| Raster Layer › Visibility Range, Effects | Scale range, transparency |
| Raster Layer › Rendering | **Symbology**: Stretch, RGB, Classify, Unique Values, Colormap, Shaded Relief; **Stretch Type**: None, Minimum Maximum, Percent Clip, Standard Deviation, Histogram Equalize; **DRA** (stretch the pixels in view); **Resampling** (nearest or bilinear); **Band Combination** (natural colour, colour infrared, or any bands) |
| Raster Layer › Enhancement | Brightness, contrast, gamma |
| Data | Raster information (size, bands, statistics, coordinate system) and Export Raster (GeoTIFF) |

Clicking a raster with the Explore tool shows its pixel values. Projects remember a raster's file
path and reopen it; rasters added in the browser are stored inside the project.

## Symbology

- **Single Symbol**: one fill, outline and marker for every feature.
- **Unique Values**: one colour per distinct value of a field.
- **Graduated Colors**: classes of a numeric field by natural breaks (Jenks), equal interval
  or quantile.
- **Proportional Symbols**: point size grows with a numeric field.

## Labeling

Turn labels on per layer (Map › Labeling › Label, or the Labeling tab), then:

- **Visibility Range**: draw labels only between two scales (Out Beyond / In Beyond), or between
  two viewing distances (Closest / Farthest). Clear Limits removes all four.
- **Placement**: automatic (above points, centred on lines and polygons), centred, or one of
  eight positions around the feature.
- **Lock** keeps the labelled features fixed while you pan and zoom; **Pause** stops drawing
  labels for faster navigation; **View Unplaced** draws the labels that didn't fit in red.
- **Convert** turns a layer's labels into an annotation layer: one point per label, carrying its
  text in a `TEXT` field, which you can then edit, move or delete like any other features.

## Graphics

Insert › Graphics draws shapes in a graphics layer (Map › Add Data › Graphics Layer, or one is
made with the first graphic). Point, Line, Curve and Polygon are clicked vertex by vertex
(double-click to finish); Freehand, Lasso, Rectangle, Circle and Ellipse are dragged. Text places
a label where you click, and Dynamic Text adds tokens that update as the map changes: `{map}`,
`{scale}`, `{date}`, `{time}`, `{crs}`, `{layers}` and `{center}`. Picture: drag a rectangle,
then pick a PNG or JPEG image to stretch over it. Tools return to Explore after each graphic
unless Keep Tool is on. Each graphic keeps its own fill, outline, width and text size in the
layer's attributes, so you can edit them in the attribute table or with `graphics.style`.

## Layouts

Insert › New Layout adds a page layout (Letter to A0, landscape or portrait) with a map frame,
title, legend, north arrow, scale bar and a dynamic credits line. Layouts open as tabs next to
the maps, with a Layout tab on the ribbon:

- **Map frames**: rectangle, circle and ellipse frames, or polygon and lasso frames drawn on the
  page. Each frame shows a map of the project at its own centre and scale; Shift-drag a frame to
  pan its map, double-click it to match the map view, or set the scale in its properties.
  **Grid** draws a labelled graticule over a frame; **Extent Indicator** outlines another frame's
  extent (an overview map).
- **Map surrounds**: north arrow, scale bar, legend, table frame (a layer's attribute table) and
  chart frame (a bar chart of a numeric field), plus a neatline or backdrop.
- **Text** and **Dynamic Text**: `{date}`, `{time}`, `{scale}`, `{map}`, `{crs}`, `{page}`,
  `{pages}`, `{pageName}` and `{layout}` fill in when the page is drawn. **Picture** places a PNG
  or JPEG.
- **Map Series**: one page per feature of an index layer, with the frame zoomed to each feature;
  step through pages, refresh, and export every page into one PDF.
- Click an element to select it, drag to move, drag its handles to resize, and edit it in the
  properties panel. The wheel zooms the page.
- **Export** writes PDF (every map series page) or PNG; **Print** opens the PDF in the system
  viewer; **Save Layout File** and **Import Layout** move layouts between projects
  (`.rgislayout`).

## Project items

RustGIS saves pieces of a project as files you can share and reuse:

| Item | Extension | Made with | Opened with |
| --- | --- | --- | --- |
| Map file (a map with its data) | `.rgismap` | Share › Save As › Map File | Insert › Import |
| Layer file (a layer with its symbology) | `.rgislyr` | Share › Save As › Layer File | Add Data |
| Styles (saved symbology) | `.rgisstyle` | Insert › Styles › New Style, then Share › Save As › Style Items | Insert › Styles › Add Style File |
| Project template (the setup without features) | `.rgistemplate` | Insert › Templates, or Share › Save As | Insert › Templates › Open Custom Template |
| Project package (everything, rasters included) | `.rgispkg` | Share › Package › Project | Project › Open |
| Map package / mobile map package | `.rgismpk` | Share › Package › Map or Mobile Map | Insert › Import |
| Results package (geoprocessing history and outputs) | `.rgisgpk` | Share › Package › Results | any zip tool |
| Task (guided command steps) | `.rgistask` | Insert › Task, Share › Save As › Task Item | Insert › Task › Import Task |
| Layout | `.rgislayout` | Layout › Save Layout File | Insert › New Layout › Import Layout |

Packages are zip files. A mobile map package keeps only the visible layers and simplifies their
geometry, for phones and tablets. Styles can also be imported from the layers of a layer file,
map file or project. Insert › Layer Templates adds empty layers with ready-made schemas (places,
routes, areas, observations…) to draw into. Add Folder connects a folder to the Catalog, which
also lists the project's layouts, styles and tasks. A task's steps each run a command; New Task
From History turns the geoprocessing history into a task you can replay.

Share › Output also prints the map (through the system's PDF viewer), captures it to the
clipboard, and exports a layer to a 3D model (Wavefront OBJ, polygons extruded by a field).

## Editing and data design

The Edit tab's **Edit** button turns editing on or off for the map; with it off, the map is
read-only. The Editing pane (Settings, Templates, Status, Modify) holds the rest:

- **Modify**: move the selected features by metres east and north, rotate them, scale them,
  merge them (polygons are unioned), explode multipart features, split a line where you click,
  and flip line direction. Edit › Tools lists every editing tool.
- **Feature templates**: named default values for new features, made from a selected feature;
  the Create tool uses the chosen template.
- **Snapping**: new vertices snap to nearby vertices, edges and line ends (magenta marker).
- **Elevation**: new features get a `z` value, constant or sampled from a raster surface.
- **Ground to grid**: a scale factor and rotation applied to entered distances and directions
  (Move, and `edit.traverse`, which draws a line or polygon from distances and bearings).
- **Topology**: rules (must not overlap, must not have gaps, must not self-intersect, must not
  have dangles, must be covered by); Validate lists the errors, and clicking one zooms to it.
- **Status**: features added, changed and deleted per layer since the last save.

The Data tab's Data Design pane keeps attributes valid, on every edit (creating, pasting,
attribute edits, Calculate Field):

- **Domains**: coded values (`code=description`) or a numeric range for a field.
- **Subtypes**: an integer field's codes, each with a name and default values for new features;
  the layer is symbolized by subtype.
- **Attribute rules**: calculations (a field set from an expression on every insert and update)
  and constraints (a SQL condition every feature must meet).
- **Contingent values**: the combinations of values allowed across fields.
- **Archiving**: keeps every edit with its time; restore the layer as it was minutes ago.
- **Relationships**: link two layers by key fields; **Related Data** selects the rows related to
  the selection.

## Clipboard and pop-ups

Copy (++ctrl+c++) and Cut (++ctrl+x++) take the selected features. Paste (++ctrl+v++) adds them to
the layer selected in Contents, matching fields by name; Paste Special adds them as a new layer.
Copied features also go on the system clipboard as GeoJSON, so they can be pasted into another
RustGIS window or a text editor, and GeoJSON copied elsewhere pastes into RustGIS. Copy Path copies
the selected layer's data source path.

Pop-ups (Map › Navigate) choose what a click with Explore identifies: the topmost feature, every
visible layer's feature (step through them with ‹ ›), only selectable layers, only the layer
selected in Contents, or nothing.

## Selection and queries

Select By Attributes and definition queries use SQL `WHERE` clauses:

```sql
pop_est > 100000000 AND continent = 'Asia'
name LIKE 'S%'
iso_a3 IN ('FRA', 'DEU', 'ITA')
gdp_md IS NULL
"OR" = 1          -- double quotes name a field, even a keyword
```

## Geoprocessing tools

| Tool | Toolbox |
| --- | --- |
| Buffer | Analysis › Proximity |
| Clip, Erase | Analysis › Extract / Overlay |
| Dissolve | Data Management › Generalization |
| Minimum Bounding Geometry, Feature To Point, Calculate Geometry Attributes | Data Management › Features |
| Simplify | Cartography › Generalization |
| Select Layer By Location / Attribute | Data Management › Layers and Table Views |
| Summarize Within, Summary Statistics | Analysis › Summarize / Statistics |
| Export Features | Conversion |

Tools honour the current selection: with features selected, only those are processed.

### GeoLibre tools

The desktop app and the command line can also run the
[GeoLibre](https://github.com/opengeos/geolibre-rust) tool suite: over 1,000 tools from
Whitebox and GeoLibre for terrain analysis (slope, aspect, hillshade, curvature), hydrology
(fill depressions, flow direction and accumulation, watersheds), image processing,
interpolation and vector work. They run on the map's layers and add their outputs as new
layers.

The suite is an optional download, so RustGIS itself stays small: the **GeoLibre runner**, a
single `geolibre` program (about 20 MB to download). In the Geoprocessing pane, click
**Install GeoLibre tools** under *GeoLibre tools*: RustGIS downloads the latest runner for your
platform from the [geolibre-rust releases](https://github.com/opengeos/geolibre-rust/releases/latest),
checks it against its published SHA-256 checksum (which catches a corrupted download; the
release itself is trusted as when you download it by hand) and installs it in its data folder
(`~/.local/share/rustgis/geolibre` on Linux, `~/Library/Application Support/RustGIS/geolibre`
on macOS, `%LOCALAPPDATA%\RustGIS\geolibre` on Windows). From the command line,
`gp.installGeolibre` does the same.

To install it yourself instead, download the archive for your platform
(`geolibre-<version>-<platform>.tar.gz`, or `.zip` on Windows), unpack it anywhere, and click
**Locate…** to pick the `geolibre` program; RustGIS remembers it.

RustGIS also finds the runner without asking when it is next to the RustGIS program (or in a
`geolibre` folder beside it), on your `PATH`, or named by the `RUSTGIS_GEOLIBRE` environment
variable (the command line uses these). The tools then appear under **GeoLibre › Raster,
Vector, Lidar, Hydrology, Terrain, Conversion** in the Geoprocessing pane and the Catalog's
toolboxes, and Find Tools searches them.

- Raster inputs are passed as their own files; vector layers (or their selection) are written
  in the first raster input's coordinate system, or the UTM zone around the data, so distances
  and areas are in metres. Data spanning more than 40° of longitude stays in degrees.
- Raster outputs are kept inside the project, so they save and reopen with it. Text and table
  reports appear in the tool's messages.
- LiDAR and other inputs RustGIS doesn't open as layers take a file path, read like Add Data
  reads one. Outputs are written to a fresh scratch folder and loaded from there.
- Each run is a separate `geolibre` process, so a tool that fails can't take RustGIS down.
- From the command line: `gp.run={"tool":"geolibre:slope","params":{"input":"dem"}}`;
  `gp.tools` lists every tool's parameters, `gp.geolibre` shows the runner in use and
  `gp.setGeolibre={"path":"…"}` picks one.

In the web app, click **Load GeoLibre tools** in the Geoprocessing pane instead. RustGIS
loads geolibre-rust's WebAssembly runner (the `geolibre-wasm` package, from jsDelivr, about
25 MB the first time, then from the browser's cache) and the tools run in the page, in an
in-memory folder: nothing is uploaded. The web app remembers this and loads them again on your
next visit. The runner's code comes from jsDelivr, pinned to one geolibre-wasm release, the
same trust as installing that package. Tools that read files by path (LiDAR files, for instance) need the desktop app,
since a web page has no file system.

In the desktop app and on the command line, RustGIS waits while a tool runs, so a long run on
a large raster or point cloud keeps the window busy until it finishes. In the web app the run
is started in the background and the result is added when it ends, but the tool itself runs
in the page, so the page is busy while it computes.

## Analysis workflows

The Analysis tab's Workflows pane holds guided analyses. Where one needs points, it asks you to
click them on the map:

| Workflow | What it does |
| --- | --- |
| Raster Functions | Slope, aspect, hillshade, NDVI, band math (`(b4-b3)/(b4+b3)`), thresholds, resampling |
| Function Editor | Chains raster functions, each on the previous output |
| Suitability Modeler | Scores rasters 1–10 and adds them with weights |
| Visibility Analysis | A viewshed from a point on an elevation raster |
| Exploratory 3D Analysis | A line of sight between two points, split into seen and hidden parts |
| Geostatistical Wizard | An IDW surface from point values |
| Network Analysis | The shortest (or cheapest, by a cost field) route over a line layer |
| Neighborhood Explorer | What lies within a distance of a point, with sums of numeric fields |
| Simulation | Random points in polygons (a fixed number, or from a field × rate) |
| Data Engineering | Counts, nulls, distinct values, min, max and mean of every field |
| Exploratory Analysis | Histograms and correlations of numeric fields |
| Embeddings | The features most like the selected one, by their numeric attributes |
| Data Interop | KML, GPX, CSV with WKT, GeoJSON or CSV |

Rasters these make are kept in the project. **Environments** limit tools to the map view, the full
extent or everywhere, and set the cell size of new rasters. **ModelBuilder** chains tools into
models (a parameter `%1` is step 1's output), and New Model From History turns the
geoprocessing you've run into a model. The **Script** pane runs commands from text, one per line
(`id {json}`).

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| ++ctrl+k++ | Command search |
| ++ctrl+n++ / ++ctrl+o++ / ++ctrl+s++ | New / Open / Save project |
| ++ctrl+z++ / ++ctrl+y++ | Undo / Redo |
| ++ctrl+t++ | Attribute table |
| ++home++ | Full extent |
| ++esc++ | Back to Explore; cancel a sketch |
| ++delete++ | Delete selected features |
| ++ctrl+c++ / ++ctrl+x++ / ++ctrl+v++ | Copy / Cut / Paste features |
