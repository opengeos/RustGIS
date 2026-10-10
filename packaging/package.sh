#!/usr/bin/env bash
# Packages release builds of rustgis and rustgis-cli for one target into dist/.
#
#   packaging/package.sh <version> [rust-target]
#
# Linux: a .tar.gz and a .deb. macOS: a .dmg holding RustGIS.app, plus a .tar.gz of the CLI.
# Windows (Git Bash): a portable .zip. Build first with
# `cargo build --release --locked -p rustgis -p rustgis-cli [--target T]`.
set -euo pipefail

version="${1:?usage: package.sh <version> [rust-target]}"
version="${version#v}"
target="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/${target:+$target/}release"
dist="$root/dist"
mkdir -p "$dist"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  Linux*) os=linux ;;
  Darwin*) os=macos ;;
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
arch="${target%%-*}"
[ -n "$arch" ] || arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
esac
exe=""
[ "$os" = windows ] && exe=".exe"
for b in rustgis rustgis-cli; do
  [ -f "$bin/$b$exe" ] || { echo "missing $bin/$b$exe: build it first" >&2; exit 1; }
done
logo() { "$bin/rustgis-cli$exe" logo "$1" --size "$2"; }
name="RustGIS-$version-$os-$arch"

case "$os" in
  linux)
    # Portable tarball.
    d="$work/$name"
    mkdir -p "$d"
    cp "$bin/rustgis" "$bin/rustgis-cli" "$root/LICENSE" "$root/README.md" \
      "$root/packaging/linux/rustgis.desktop" "$d/"
    logo "$d/rustgis.png" 512
    tar -C "$work" -czf "$dist/$name.tar.gz" "$name"

    # Debian package.
    case "$arch" in x86_64) debarch=amd64 ;; aarch64) debarch=arm64 ;; *) debarch="$arch" ;; esac
    p="$work/deb"
    install -Dm755 "$bin/rustgis" "$p/usr/bin/rustgis"
    install -Dm755 "$bin/rustgis-cli" "$p/usr/bin/rustgis-cli"
    install -Dm644 "$root/packaging/linux/rustgis.desktop" "$p/usr/share/applications/rustgis.desktop"
    for s in 32 64 128 256 512; do
      mkdir -p "$p/usr/share/icons/hicolor/${s}x${s}/apps"
      logo "$p/usr/share/icons/hicolor/${s}x${s}/apps/rustgis.png" "$s"
    done
    install -Dm644 "$root/LICENSE" "$p/usr/share/doc/rustgis/copyright"
    mkdir -p "$p/DEBIAN"
    # Libraries the binaries link against, worked out by dpkg-shlibdeps (it wants a
    # debian/control in the working directory). The windowing and GPU libraries are loaded at
    # run time, so they are listed by hand below.
    mkdir -p "$work/shlibs/debian"
    touch "$work/shlibs/debian/control"
    if ! shlibs="$(cd "$work/shlibs" && dpkg-shlibdeps -O "$p/usr/bin/rustgis" "$p/usr/bin/rustgis-cli" | sed -n 's/^shlibs:Depends=//p')"; then
      # Off Debian/Ubuntu there is no dpkg database to consult; fall back to the core libraries.
      echo "dpkg-shlibdeps failed; using a minimal Depends list" >&2
      shlibs=""
    fi
    cat > "$p/DEBIAN/control" <<CONTROL
Package: rustgis
Version: $version
Section: science
Priority: optional
Architecture: $debarch
Maintainer: Qiusheng Wu <giswqs@gmail.com>
Homepage: https://rustgis.app
Installed-Size: $(du -sk "$p/usr" | cut -f1)
Depends: ${shlibs:-libc6, libgcc-s1}, libxkbcommon0, libwayland-client0 | libx11-6
Recommends: libxkbcommon-x11-0, libxcursor1, libxi6, libxrandr2, libvulkan1 | libegl1
Description: Desktop GIS with a ribbon workflow, written in Rust
 Maps and layers, symbology, labeling, selection, editing, geoprocessing and
 export. Includes rustgis-cli for scripting every command.
CONTROL
    dpkg-deb --root-owner-group --build "$p" "$dist/rustgis_${version}_${debarch}.deb"
    ;;

  macos)
    app="$work/RustGIS.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$bin/rustgis" "$app/Contents/MacOS/rustgis"
    sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
    iconset="$work/rustgis.iconset"
    mkdir -p "$iconset"
    for s in 16 32 128 256 512; do
      logo "$iconset/icon_${s}x${s}.png" "$s"
      logo "$iconset/icon_${s}x${s}@2x.png" "$((s * 2))"
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/rustgis.icns"
    # Ad-hoc signature: required to launch on Apple Silicon; not notarized.
    codesign --force --deep --sign - "$app"

    stage="$work/dmg"
    mkdir -p "$stage"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -volname "RustGIS $version" -srcfolder "$stage" -ov -format UDZO "$dist/$name.dmg"

    d="$work/rustgis-cli-$version-$os-$arch"
    mkdir -p "$d"
    cp "$bin/rustgis-cli" "$root/LICENSE" "$d/"
    tar -C "$work" -czf "$dist/rustgis-cli-$version-$os-$arch.tar.gz" "$(basename "$d")"
    ;;

  windows)
    d="$work/$name"
    mkdir -p "$d"
    cp "$bin/rustgis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/README.md" "$d/"
    (cd "$work" && 7z a -tzip -bso0 "$dist/$name.zip" "$name")
    ;;
esac

ls -l "$dist"
