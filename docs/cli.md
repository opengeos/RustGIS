# Command Line

Every ribbon action is a command with an id. `rustgis-cli` runs them headlessly.

```sh
rustgis-cli commands                  # every command id with its parameters
rustgis-cli tools                     # geoprocessing tools
rustgis-cli info data.geojson         # layer summary as JSON
rustgis-cli render data.geojson map.png --basemap ocean --width 1600 --height 900
rustgis-cli parity                    # ribbon feature coverage
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

![A map of Europe exported by rustgis-cli](https://assets.rustgis.app/images/cli-export.webp)
