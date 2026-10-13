# Installation

## Download

Each [GitHub release](https://github.com/opengeos/RustGIS/releases/latest) carries ready-to-run
builds of the desktop app and `rustgis-cli`. The same files, grouped by platform, are mirrored at
**[downloads.rustgis.app](https://downloads.rustgis.app)**:

| Platform | File |
| --- | --- |
| Windows x64 (installer, no admin rights) | `RustGIS-<version>-windows-x86_64-setup.exe` |
| Windows x64 (installer for all users) | `RustGIS-<version>-windows-x86_64.msi` |
| Windows x64 (no install) | `RustGIS-<version>-windows-x86_64-portable.zip` (unzip and run `rustgis.exe`) |
| Windows x64 (Microsoft Store package) | `RustGIS-<version>-windows-x86_64.msix` |
| macOS Apple Silicon | `RustGIS-<version>-macos-aarch64.dmg` |
| macOS Intel | `RustGIS-<version>-macos-x86_64.dmg` |
| Linux x64 / arm64 (Debian, Ubuntu) | `rustgis_<version>_amd64.deb` / `rustgis_<version>_arm64.deb` |
| Linux x64 / arm64 (Fedora, RHEL, openSUSE) | `rustgis-<version>-1.x86_64.rpm` / `rustgis-<version>-1.aarch64.rpm` |
| Linux x64 / arm64 (any distribution, no install) | `RustGIS-<version>-linux-x86_64.AppImage` / `RustGIS-<version>-linux-aarch64.AppImage` |
| Linux x64 / arm64 (other) | `RustGIS-<version>-linux-x86_64.tar.gz` / `RustGIS-<version>-linux-aarch64.tar.gz` |

On macOS the command line tool is a separate `rustgis-cli-<version>-macos-<arch>.tar.gz`;
the other packages include it (the Windows installers and the MSIX add `rustgis-cli` to every
new terminal). `SHA256SUMS` lists the checksum of every file.

On Windows, `setup.exe` installs for your account in `%LOCALAPPDATA%\Programs\RustGIS` and needs
no admin rights; the `.msi` installs for every user in `Program Files` and suits managed
deployment (`msiexec /i RustGIS-<version>-windows-x86_64.msi /qn`). Both add a Start menu
entry, open `.rgis` projects and uninstall from **Settings › Apps**.

The `.msix` is the package submitted to the Microsoft Store, which signs it. It is unsigned
on the release page, so Windows only installs it from the Store; download an installer or the
portable zip instead.

The macOS DMG and command line tool are signed with a Developer ID and notarized by Apple, so
they open without a warning. From the release after 0.1.0, the notarization ticket is also
stapled to RustGIS.app, so it opens even when the Mac cannot reach Apple's servers. If macOS
says Apple could not verify RustGIS 0.1.0, connect to the internet and open it again, or run
`xattr -dr com.apple.quarantine /Applications/RustGIS.app`. The Windows builds are not yet
signed: choose **More info › Run anyway** if SmartScreen warns about an unknown publisher.

## Package managers

RustGIS is also published to these package managers, which keep it up to date:

| Platform | Command |
| --- | --- |
| Windows ([winget](https://learn.microsoft.com/windows/package-manager/)) | `winget install OpenGeos.RustGIS` |
| macOS ([Homebrew](https://brew.sh)) | `brew install --cask opengeos/rustgis/rustgis` |
| macOS and Linux, command line tool only (Homebrew) | `brew install opengeos/rustgis/rustgis-cli` |
| Arch Linux ([AUR](https://aur.archlinux.org/packages/rustgis-bin)) | `yay -S rustgis-bin` (or any AUR helper) |
| Fedora ([COPR](https://copr.fedorainfracloud.org/coprs/giswqs/rustgis/)) | `sudo dnf copr enable giswqs/rustgis && sudo dnf install rustgis` |

winget installs for your account with the setup.exe; add `--scope machine` for the `.msi`. On
Homebrew 5.1 and later, run `brew trust opengeos/rustgis` once before installing from the tap.
Each package is the same build as the release page, and a new release reaches the package
managers shortly after it is published (winget after Microsoft's review).

## From source

RustGIS builds from source with a recent stable Rust toolchain (1.90 or later).

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
  GeoJSON, CSV, GeoTIFF, ASCII grids and `.rgis` projects work; Shapefiles need the desktop
  app.
- **Save** and **Export** download the file.
- The interface uses the built-in font, because a web page can't read installed fonts.

The web app opens the way **Settings › Start-up** says (remembered in the browser). The URL can
override that for one visit: add `?blank` to start with an empty project,
`?projection=EPSG:3010` (any projected EPSG code, a preset id such as `robinson`, or `globe`) to
start in another projection, `?extent=10.5,55,24.5,69.5` (west, south, east, north in degrees) to
open on an area, `?lang=de` (or any language code) to pick the interface language, or `?webgl` to
force WebGL2. Combine them with `&`, so a self-hosted build can link to
`?blank&projection=EPSG:3010&extent=10.5,55,24.5,69.5`.

Links can carry data too: `?data=<address>` adds a file by its web address (any format Add Data
reads, downloaded in the background; repeat it for up to 16, and the map zooms to the first that
lands with something to show), `?style=<address>` adds a MapLibre style, and `?tiles=<template>`
a tile layer. Percent-encode an address holding `&`, `#` or spaces (`encodeURIComponent`); a `+`
stays a `+`. The server must allow the browser to fetch the file (CORS); a file that can't be
added is said in a notice. For example,
`?blank&data=https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_week.geojson` opens
the past week's earthquakes.

`?embed` runs the app inside another page, as the [Python package's notebook widget](python.md)
does: it starts blank, keeps its settings in memory rather than in the browser, and exchanges
the project with the page that frames it through `postMessage` (the messages are described in
`apps/rustgis-web/src/embed.rs`).

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

## If RustGIS closes unexpectedly

The desktop app writes its internal errors to `crash.log` in its settings folder: one that closed
the app, and also one it recovered from (an error in the AI Assistant, for example, stops the
request and says so in the chat instead of closing the app). The folder is `%APPDATA%\RustGIS` on
Windows, `~/Library/Application Support/RustGIS` on macOS, and `$XDG_CONFIG_HOME/rustgis` on
Linux (`~/.config/rustgis` when that isn't set). The next launch moves the log to
`crash-last.log` and names the file in a notice.

Attach it to a [bug report](https://github.com/opengeos/RustGIS/issues/new?template=bug_report.yml).
It holds the version, the operating system, the error message and where in RustGIS it happened.
An error message can quote what RustGIS was working on, such as a file path or a layer name, so
read the log first and remove anything you don't want to share.
