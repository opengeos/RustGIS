# Command Line

Every ribbon action is a command with an id. `rustgis-cli` runs them headlessly.

```sh
rustgis-cli commands                  # every command id with its parameters
rustgis-cli tools                     # geoprocessing tools
rustgis-cli info data.geojson         # layer summary as JSON
rustgis-cli render data.geojson map.png --basemap ocean --width 1600 --height 900
rustgis-cli render data.geojson map.png --tiles 'https://tile.openstreetmap.org/{z}/{x}/{y}.png'
rustgis-cli parity                    # ribbon feature coverage
rustgis-cli logo icon.png --size 256  # the app icon as a PNG
rustgis-cli plugins                   # installed plugins (also: browse, get, install, uninstall…)
```

## Chaining commands

`run` loads data or a project, runs `--cmd 'id={json params}'` in order, and can save the
result:

```sh
rustgis-cli run --sample \
  --cmd 'gp.run={"tool":"countPoints","params":{"polygons":"Countries","points":"Cities"}}' \
  --cmd 'layer.symbology={"layer":"Countries_Summarize","type":"graduated","field":"point_count"}' \
  --cmd 'bookmark.go={"name":"Europe"}' \
  --cmd 'share.exportMap={"path":"europe.png","dpi":192}' \
  --save atlas.rgisx
```

Installed [plugins](plugins.md) load first (`--no-plugins` skips them), and `--plugin <file or
folder>` loads one more for this run only. Their commands and tools have ids like
`plugin:vector-toolkit/hexagon-grid`:

```sh
rustgis-cli run --sample --plugin vector-toolkit-0.1.0.rgplugin \
  --cmd 'gp.run={"tool":"plugin:vector-toolkit/hexagon-grid","params":{"input":"Countries","size":300}}' \
  --cmd 'share.exportMap={"path":"hexagons.png"}'
```

Rasters work the same way:

```sh
rustgis-cli render dem.tif dem.png
rustgis-cli run --data dem.tif \
  --cmd 'raster.symbology={"type":"shadedRelief","ramp":"Terrain","zFactor":1.5}' \
  --cmd 'raster.info={}' \
  --cmd 'share.exportMap={"path":"relief.png","dpi":192}'
```

![A map of Europe exported by rustgis-cli](https://assets.rustgis.app/images/cli-export.webp)
