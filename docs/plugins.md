# Plugins

Plugins add commands and geoprocessing tools to RustGIS. A plugin is a WebAssembly module with
a `plugin.json` manifest, packaged as a `.rgplugin` file. The same file runs in the desktop app,
on the command line and in the web app.

Browse the registry at **[plugins.rustgis.app](https://plugins.rustgis.app)**. Plugin sources,
the SDK and the registry live in
[opengeos/rustgis-plugins](https://github.com/opengeos/rustgis-plugins).

## Using plugins

Open **Plugins › Plugin Manager**:

- **Browse** lists the plugins in the registry. **Install** downloads the plugin, checks it
  against the SHA-256 the registry published, and shows what the plugin may do before it loads.
- **Installed** lists your plugins: turn them on or off, update them, or uninstall them.
- **Settings** installs a `.rgplugin` file. You can also drop the file on the window. It also
  sets the registry address (empty means the RustGIS registry). On the desktop, it can load a
  plugin from its folder while you develop it.

A plugin's commands and tools appear on the **Plugins** tab, in a group named after the
plugin, or on the plugin's own tab if it has one. Its commands are also in command search, and
its tools are in the **Geoprocessing** pane under **Plugins ›**.

The desktop app keeps installed packages in its data folder (`plugins/` beside the GeoLibre
runner, or `$RUSTGIS_PLUGINS_DIR`) and loads them at startup. The web app remembers the
plugins you installed from the registry and downloads them again on your next visit. Packages
installed from files are kept too, if they are small enough for the browser's storage.

On the command line:

```sh
rustgis-cli plugins                          # installed plugins
rustgis-cli plugins browse                   # the registry
rustgis-cli plugins get vector-toolkit       # install from the registry
rustgis-cli plugins install my.rgplugin      # install a file (or a plugin folder)
rustgis-cli plugins disable vector-toolkit   # enable / disable / uninstall <id>

rustgis-cli run --sample \
  --cmd 'gp.run={"tool":"plugin:vector-toolkit/hexagon-grid","params":{"input":"Countries","size":300}}' \
  --cmd 'share.exportMap={"path":"hex.png"}'
rustgis-cli run --plugin ./dev-plugin/ --sample --cmd 'plugin:my-plugin/hello'   # load for one run
```

## Safety

Plugins run in a sandbox: an interpreter (`wasmi`) with no access to files, the network, the
clock or anything else outside the module. A plugin reaches RustGIS only through the requests
described below, and RustGIS checks each one:

- Every plugin can **read the map**: its layers, their features and the view.
- With the **`edit`** permission, it can change the project. It can run commands that edit it and
  add layers. A plugin command that changes the project is one undo step, and it is rolled back
  if the command fails.
- With the **`files`** permission, it can run commands that read or write files the user names.
- No plugin may run the user's scripts, tasks or models, install or locate other software,
  manage plugins, replace the project, or step through undo history.

The install prompt lists these permissions. Each call has a fuel budget, so a plugin stuck in a
loop is stopped. Memory is capped at 512 MB, and requests and replies are size-limited. A plugin
that traps, panics or returns garbage produces an error message, never a crash.

The registry is curated: plugins are reviewed before they are listed, and packages are
hash-pinned. A `blocklist.json` beside the registry withdraws a plugin (every version, or one
package) from every install. RustGIS checks it when you browse the registry and at startup.

## Writing a plugin

Start from [`plugins/hello`](https://github.com/opengeos/rustgis-plugins/tree/main/plugins/hello)
in the plugins repository. It uses the `rustgis-plugin` SDK crate:

```rust
use rustgis_plugin::{Kind, Request, ToolOutput, Value, geojson, host, json};

fn handle(req: Request) -> Result<Value, String> {
    match (req.kind, req.id.as_str()) {
        (Kind::Command, "layer-report") => {
            let n = host::layers()?.as_array().map_or(0, Vec::len);
            host::notify(&format!("{n} layers"));
            Ok(json!({"layers": n}))
        }
        (Kind::Tool, "bounding-boxes") => {
            let boxes: Vec<Value> = req.features("input")?.iter().filter_map(|f| {
                let b = geojson::bbox(&geojson::positions(&f["geometry"]))?;
                Some(geojson::feature(geojson::polygon(&[[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]), json!({})))
            }).collect();
            Ok(ToolOutput::new().layer("Boxes", geojson::collection(boxes)).into())
        }
        (_, other) => Err(format!("unknown '{other}'")),
    }
}

rustgis_plugin::export!(handle);
```

```sh
cargo build --release --target wasm32-unknown-unknown
```

### `plugin.json`

```json
{
  "id": "hello-rustgis",
  "name": "Hello RustGIS",
  "version": "0.1.0",
  "description": "What it does, in a sentence or two.",
  "author": "You",
  "homepage": "https://github.com/you/your-plugin",
  "license": "MIT",
  "entry": "plugin.wasm",
  "abi": 1,
  "minRustgisVersion": "0.1.0",
  "permissions": ["edit"],
  "commands": [
    { "id": "layer-report", "label": "Layer\nReport", "help": "Shown on hover.", "icon": "stats", "params": "{}" }
  ],
  "tools": [
    {
      "id": "bounding-boxes", "name": "Bounding Boxes", "summary": "One line.", "toolbox": "Hello",
      "params": [
        { "name": "input", "label": "Input features", "kind": "vectorLayer", "required": true },
        { "name": "whole", "label": "One box for the whole layer", "kind": "bool", "default": false }
      ]
    }
  ],
  "i18n": { "de": { "Layer\nReport": "Ebenen-\nbericht" } }
}
```

- `id`: lower-case letters, digits and hyphens. Commands and tools get ids
  `plugin:<id>/<command>`, which the ribbon, command search, scripts and `rustgis-cli` use.
- `version` and `minRustgisVersion` look like `1.2.3`. RustGIS refuses a plugin that needs a
  newer RustGIS.
- `permissions`: `edit` and `files` (see [Safety](#safety)). Ask only for what you use.
- `commands[].icon`: the name of one of RustGIS's own icons (`stats`, `zoom_full`, `bookmark`,
  `buffer`…), or a puzzle piece when it isn't one.
- Tool parameter `kind`s: `layer`, `vectorLayer`, `polygonLayer`, `pointLayer`, `field`,
  `numericField`, `number`, `text`, `choice` (with `choices`) and `bool`.
- `i18n` translates the plugin's own labels, help, names and summaries into the interface
  language, by language code (`de`, or `zh-CN` with a fallback to `zh`).

A package is a zip archive named `<id>-<version>.rgplugin`. It holds `plugin.json` and the module
at its root, or inside one top-level folder. Packages are at most 64 MB, and modules at most 48 MB.

### A tab of its own

By default a plugin's commands and tools get a group on the **Plugins** tab. A plugin can have a
ribbon tab of its own instead. The tab sits at the end of the tab strip, and **Help › Ribbon**
can hide it like any other. Declare it in `plugin.json`:

```json
"tab": {
  "label": "Explorer",
  "groups": [
    {
      "label": "Go To",
      "items": [
        { "menu": "Continent", "icon": "zoom_world", "help": "Go to a continent.", "items": ["go-africa", "go-europe"] },
        "random-place"
      ]
    },
    { "label": "Inspect", "items": ["layer-report", "tool:field-summary"] }
  ]
}
```

- `items` lists the buttons. An item is a command id, `tool:<id>` (which opens that tool in the
  Geoprocessing pane), or a dropdown `menu` whose `items` are command ids or `tool:<id>`.
- A tab has 1 to 8 groups, a group has 1 to 16 items, and a menu has 1 to 32 entries. Every
  entry must name one of the plugin's own commands or tools.
- The label can't be the name of one of RustGIS's tabs (Map, Insert, Analysis, Plugins…). The
  `i18n` translations apply to the tab, group and menu labels too.

[Map Explorer](https://github.com/opengeos/rustgis-plugins/tree/main/plugins/map-explorer) is
an example.

### What a plugin can ask for

Inside a request, `host::call(op, params)` sends one of these to RustGIS:

| Request | Answer |
|---|---|
| `host.info` | RustGIS's version, the ABI, the plugin's id and permissions |
| `host.layers` | `[{id, name, raster, visible, geometry, features, selected, active, fields: [{name, type}]}]` |
| `host.features` `{layer, selected?, limit?}` | the layer's features as GeoJSON; each feature has its `row` |
| `host.view` | `{center: [lon, lat], scale, projection}` |
| `host.notify` `{text, error?}` | shows a message in the status bar |
| `host.addLayer` `{name, geojson}` | adds a layer (`edit`) |
| any command id (`layer.zoomTo`, `select.byAttributes`, `layer.symbology`…) | what the command returns, within the plugin's permissions |

`rustgis-cli commands` lists every command id and its parameters.

Tools work differently. A tool receives its layer parameters' features as GeoJSON (only the
selected features when a layer has a selection). It returns new layers, standalone tables or a
selection, plus messages for the Geoprocessing history. RustGIS adds the results as it does for
built-in tools, with undo. Tools can log, but they make no host requests.

### The ABI (version 1)

The SDK implements this; it is described here for other languages.

The module is `wasm32` with no imports except these three, in module `rustgis`:

- `call(ptr: i32, len: i32) -> i32` runs the host request at `ptr`
  (`{"op": "...", "params": {...}}`). It returns the length of the reply, which is
  `{"ok": value}` or `{"error": "text"}`, or `-1` when there is none.
- `result(ptr: i32)` copies that reply to `ptr`. Allocate the length `call` returned first.
- `log(ptr: i32, len: i32)` logs a UTF-8 line.

The module exports:

- `memory`.
- `rustgis_alloc(len: i32) -> i32`, which returns room for `len` bytes. RustGIS writes the
  request there.
- `rustgis_call(ptr: i32, len: i32) -> i64`, which handles the request and returns
  `reply_ptr << 32 | reply_len`. The reply is `{"ok": value}` or `{"error": "text"}`.

Requests are `{"abi": 1, "kind": "command" | "tool", "id": "...", "params": {...}, "layers": {...}}`.
Tools also get `layers`, which maps each layer's name to its FeatureCollection. A tool's `ok`
value is `{"layers": [{"name", "geojson"}], "tables": [...], "selection": {"layer", "rows"},
"messages": [...]}`.

Each call gets a fresh instance, so a module keeps no state between calls and need not free
memory. The fuel budget is about 2×10⁹ instructions for a command and 4×10¹⁰ for a tool.

## Publishing a plugin

Open a pull request on
[opengeos/rustgis-plugins](https://github.com/opengeos/rustgis-plugins) that adds
`registry/<id>.json`:

- For a plugin whose source is in that repository: `{"id": "...", "path": "plugins/<dir>", "categories": [...]}`.
- For one you publish elsewhere: `{"id": "...", "source": {"url": "https://…/<id>-<version>.rgplugin", "sha256": "…"}, "categories": [...]}`.

CI builds or downloads the package and checks its manifest and module. A maintainer then
reviews it. Once merged, it is published at `plugins.rustgis.app` with its hash.
