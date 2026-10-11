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
