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

## Data

| Format | Read | Write |
| --- | --- | --- |
| GeoJSON (`.geojson`, `.json`) | Yes | Yes |
| CSV (`.csv`, `.txt`) with longitude/latitude columns | Yes | Yes |
| Shapefile (`.shp` + `.dbf`) | Yes | No |
| RustGIS project (`.rgisx`) | Yes | Yes |

Coordinates are WGS 84 longitude/latitude and are displayed in Web Mercator.

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
