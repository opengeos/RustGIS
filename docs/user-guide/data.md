# Data

## Formats

| Format | Read | Write |
| --- | --- | --- |
| GeoJSON (`.geojson`, `.json`) | Yes | Yes |
| CSV (`.csv`, `.txt`) with longitude/latitude columns | Yes | Yes |
| Shapefile (`.shp` + `.dbf`) | Yes (desktop) | No |
| GeoPackage (`.gpkg`, the first feature table) | Yes (desktop) | No |
| GeoParquet (`.parquet`, `.geoparquet`, WKB geometry) | Yes (zstd-compressed files on the desktop only) | No |
| FlatGeobuf (`.fgb`) | Yes | No |
| KML, GPX, CSV with WKT geometry | No | Yes (Analysis › Workflows › Data Interop) |
| GeoTIFF and cloud-optimised GeoTIFF (`.tif`, `.tiff`) | Yes | Yes |
| ASCII grid (`.asc`, with an optional `.prj`) | Yes | No |
| Web tiles: XYZ/TMS/WMS rasters, Mapbox Vector Tiles, MapLibre styles | Yes | Yes (Imagery › Create Hosted Imagery) |
| RustGIS project (`.rgis`; older `.rgisx` files open too) | Yes | Yes |

Add data with **Map › Add Data**, by dropping files on the window, or by passing them on the
command line (`rustgis places.geojson`). A CSV without coordinate columns, or a layer without
geometry, opens as a standalone table, listed under the map in Contents. **Map › Add Data ›
Graphics Layer** adds a layer for [drawn graphics](cartography.md#graphics), and **Insert ›
Layer Templates** adds empty layers with ready-made schemas to draw into.

Vector coordinates are WGS 84 longitude/latitude and are displayed in Web Mercator. GeoPackage,
GeoParquet and FlatGeobuf files in another coordinate system (any EPSG code) are converted as they
are read. Rasters keep
their own coordinate system (any EPSG code, such as UTM zones or national grids) and are
reprojected on the fly as they draw.

## Rasters

![A shaded-relief elevation raster of the Sierra Nevada with the Raster Layer tab open](https://assets.rustgis.app/images/raster.webp)

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
