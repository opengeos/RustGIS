# Getting Started

## Open the app

```sh
cargo run --release -p rustgis                  # opens the World Atlas sample
cargo run --release -p rustgis -- places.geojson
cargo run --release -p rustgis -- atlas.rgisx   # a saved project
```

## Five minutes with the World Atlas

1. **Explore.** Drag to pan, scroll to zoom, and click a country to see its attributes in a
   pop-up. Press ++home++ for the full extent.
2. **Style a layer.** Click *Countries* in the Contents pane. The *Feature Layer* tab group
   appears. Open **Feature Layer › Symbology**, choose *Graduated Colors* or *Unique Values*,
   and pick a colour scheme.
3. **Select.** **Map › Select By Attributes**, enter `continent = 'Asia'`, and press *Apply*.
   Open the attribute table with ++ctrl+t++. Rows and map features stay in sync.
4. **Analyse.** **Analysis › Buffer**: choose *Cities*, 300 km, *Dissolve: All*, and *Run*. The
   result is added as a new layer, and the run is listed under **Analysis › History**.
5. **Share.** **Share › Export Map** saves a PNG with labels and a scale bar. **Project › Save
   As** writes a `.rgisx` project.

!!! tip "Command search"
    Press ++ctrl+k++ to find any command or geoprocessing tool by name.
