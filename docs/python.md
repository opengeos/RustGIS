# Python

The `rustgis` package runs the RustGIS engine inside Python, headless. It opens data and
projects, runs any command and geoprocessing tool, styles layers, renders maps and saves projects
that the desktop app opens as they are. There is no app to install and no server to run: the
engine is a native module built for Windows, macOS and Linux, for Python 3.12 and later.

```sh
pip install rustgis               # pip install "rustgis[geopandas]" for GeoDataFrames
```

## A first session

```python
import rustgis

s = rustgis.Session(sample=True)                    # the World Atlas sample
s.layers()                                          # id, name, type, features, fields…
s.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 100})
s.run("layer.symbology", layer="Countries", type="graduated", field="pop_est")
s.to_png("map.png", width=1600, height=900)
s.save("atlas.rgis")
```

In Jupyter, a session on the last line of a cell shows its map as a picture. For the app itself,
use a `Map`.

## The app in a notebook

`rustgis.Map` shows the RustGIS app (ribbon, panes, map) in the cell, kept in step with its
session: what Python adds or styles appears in the app, and what you do in the app (an edit, a new
layer, a pan) is back in the session for Python to read. It needs
[anywidget](https://anywidget.dev), and works in JupyterLab, Notebook, VS Code and Colab.

```sh
pip install "rustgis[jupyter]"
```

```python
m = rustgis.Map(sample=True, height="600px")
m                                                   # the app, in the cell

m.add_geojson("sites.geojson", name="Sites")        # appears in the app
m.run("layer.symbology", layer="Sites", type="unique", field="name")
m.set_center(10, 50, scale=5_000_000)
m.add_tiles("https://tile.openstreetmap.org/{z}/{x}/{y}.png", name="OpenStreetMap")

m.info()["center"]                                  # where the app's map is now
m.save("sites.rgis")                                # with what was done in the app
m.run_in_app("ui.language", code="de")              # a command only the app has
```

`Map` has the `Session` methods (`run`, `add_data`, `layers`, `to_png`, ...); `m.session` is the
session itself. Files of up to 100 MB travel with the project, so the app shows them; a larger
file is read from its path, which only Python can see, so it draws in `to_png` but not in the app.

The app is the [web app](installation.md), loaded from https://web.rustgis.app, so the
notebook needs to be online. `Map(url=...)` or the `RUSTGIS_WEB_URL` environment variable points
it at another copy (a self-hosted one, or `trunk serve` in `apps/rustgis-web`). Settings you
change in the widget last until the cell is run again: an embedded app keeps nothing in the
browser.

## Commands

Every action in RustGIS is a command with an id and JSON parameters: the same ids the ribbon,
the [command line](cli.md) and the [AI Assistant](user-guide/assistant.md) use.
`Session.run(id, ...)` runs any of them, with the parameters as keyword arguments, a dict, or
both. It returns the command's result and raises `rustgis.RustGISError` when the command fails.

```python
s.commands()                                        # every command, with its parameters
s.tools()                                           # the geoprocessing tools
s.run("select.byAttributes", where="pop_max > 10000000", layer="Cities")
s.run("map.goToXY", {"lon": 114.1, "lat": 22.4, "scale": 2_000_000})
s.notices()                                         # what the last commands reported
s.undo()
```

The GeoLibre and Whitebox tools run as they do in the app, through the `geolibre` program
(see [Analysis](user-guide/analysis.md)). `rustgis.load_plugins()`, or
`Session(plugins=True)`, loads the [plugins](plugins.md) installed for the desktop app.

## Data in and out

```python
s = rustgis.Session(data=["parcels.gpkg", "roads.fgb"])   # or s.add_data(path, name=...)
s.add_geojson({"type": "FeatureCollection", "features": [...]}, name="Sites")

gdf = s.layer("Cities").to_geopandas()             # a GeoDataFrame in EPSG:4326
s.add_geopandas(gdf[gdf.pop_max > 5_000_000], name="Big cities")
s.layer("Cities").to_geojson()                     # a FeatureCollection dict
```

Every format the app reads works here: GeoJSON, CSV, Shapefile, GeoPackage, GeoParquet,
FlatGeobuf, KML/KMZ, GPX, GeoTIFF and the rest of [Supported Data
Formats](user-guide/data-formats.md). A large file read in place keeps only the features of the
last view in memory; run `layer.loadAll` on it before `to_geojson` or `to_geopandas` to get all
of them.

## From source

The package lives in `apps/rustgis-python`, built with [maturin](https://www.maturin.rs):

```sh
pip install ./apps/rustgis-python                   # or: cd apps/rustgis-python && maturin develop
python -m pytest apps/rustgis-python/tests
```

Its version is the RustGIS version, and each release publishes wheels to PyPI.
