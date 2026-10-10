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
| Web browser | Planned (WebAssembly) |

## Fonts

RustGIS uses an installed sans-serif when it finds one (Segoe UI, San Francisco, Inter, Noto
Sans, Open Sans or DejaVu Sans) and falls back to a built-in face. Set
`RUSTGIS_BUILTIN_FONTS=1` to always use the built-in face.
