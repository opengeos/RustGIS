# Installation

RustGIS builds from source with a recent stable Rust toolchain (1.90 or later).

## From source

```sh
git clone https://github.com/opengeos/RustGIS.git
cd RustGIS
cargo run --release -p rustgis            # the desktop app
cargo build --release -p rustgis-cli      # the command line tool
```

The build downloads the Natural Earth sample data (a few hundred KB) from
[rustgis-assets](https://github.com/opengeos/rustgis-assets). To build offline, download the
four files from `https://assets.rustgis.app/data/` once and point the build at them:

```sh
RUSTGIS_DATA_DIR=/path/to/data cargo build --release
```

## Platforms

| Platform | Status |
| --- | --- |
| Linux (X11 and Wayland) | Supported |
| macOS | Supported |
| Windows | Supported |
| Web browser | Supported: [web.rustgis.app](https://web.rustgis.app) (WebGPU or WebGL2) |

## Web app

RustGIS also runs in the browser, compiled to WebAssembly: open
**[web.rustgis.app](https://web.rustgis.app)**. It is the same app with three differences:

- **Open** and **Add Data** use the browser's file picker, and you can drop files on the page.
  GeoJSON, CSV and `.rgisx` projects work; Shapefiles need the desktop app.
- **Save** and **Export** download the file.
- The interface uses the built-in font, because a web page can't read installed fonts.

Add `?blank` to the URL to start with an empty project, or `?webgl` to force WebGL2.

To build it yourself, install [Trunk](https://trunkrs.dev) and the `wasm32-unknown-unknown`
target, then:

```sh
cd apps/rustgis-web
trunk serve          # http://127.0.0.1:8772
trunk build --release   # output in dist/web
```

## Fonts

RustGIS uses an installed sans-serif when it finds one (Segoe UI, San Francisco, Inter, Noto
Sans, Open Sans or DejaVu Sans) and falls back to a built-in face. Set
`RUSTGIS_BUILTIN_FONTS=1` to always use the built-in face.
