#!/usr/bin/env bash
# Renders the README and docs screenshots into target/shots/*.webp, for
# opengeos/rustgis-assets (images/). Run from the repository root. Needs ImageMagick
# (`magick`) and network access (the tiles and plugins shots, the sample DEM).
#
# The assistant shot asks a real model: set RUSTGIS_ASSISTANT_PROVIDER and the
# provider's key (see docs/user-guide/assistant.md) or it shows the empty pane.
set -euo pipefail
cd "$(dirname "$0")/../../../.."
dir=crates/ui-egui/examples/shots
out=target/shots
mkdir -p "$out"
[ -f "$out/dem.tif" ] || curl -fsSL -o "$out/dem.tif" https://github.com/giswqs/data/raw/main/raster/srtm90.tif
cargo build --release -p rustgis-ui-egui --example ui_shot -p rustgis-cli
for script in "$dir"/*.jsonl; do
  target/release/examples/ui_shot "$script"
done
target/release/rustgis-cli run --sample \
  --cmd 'map.projection={"type":"europeEqualArea"}' \
  --cmd 'map.goToXY={"lon":14,"lat":52,"scale":22000000}' \
  --cmd 'share.exportMap={"path":"target/shots/cli-export.png","width":1200,"height":750,"dpi":192}'
for png in "$out"/*.png; do
  magick "$png" -quality 85 -define webp:method=6 "${png%.png}.webp"
done
ls "$out"/*.webp
