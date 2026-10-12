#!/usr/bin/env bash
# Packages release builds of rustgis and rustgis-cli for one target into dist/.
#
#   packaging/package.sh <version> [rust-target]
#
# Linux: a .tar.gz and a .deb. macOS: a .dmg holding RustGIS.app, plus a .tar.gz of the CLI.
# Windows (Git Bash): a portable .zip and an .msix for the Microsoft Store. Build first with
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
      "$root/packaging/linux/rustgis.desktop" "$root/packaging/linux/rustgis-mime.xml" "$d/"
    logo "$d/rustgis.png" 512
    tar -C "$work" -czf "$dist/$name.tar.gz" "$name"

    # Debian package.
    case "$arch" in x86_64) debarch=amd64 ;; aarch64) debarch=arm64 ;; *) debarch="$arch" ;; esac
    p="$work/deb"
    install -Dm755 "$bin/rustgis" "$p/usr/bin/rustgis"
    install -Dm755 "$bin/rustgis-cli" "$p/usr/bin/rustgis-cli"
    install -Dm644 "$root/packaging/linux/rustgis.desktop" "$p/usr/share/applications/rustgis.desktop"
    install -Dm644 "$root/packaging/linux/rustgis-mime.xml" "$p/usr/share/mime/packages/rustgis.xml"
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
    printf 'Source: rustgis\n\nPackage: rustgis\nArchitecture: any\n' > "$work/shlibs/debian/control"
    shlibs="$(cd "$work/shlibs" && dpkg-shlibdeps -O "$p/usr/bin/rustgis" "$p/usr/bin/rustgis-cli" | sed -n 's/^shlibs:Depends=//p')" || shlibs=""
    if [ -z "$shlibs" ]; then
      # Off Debian/Ubuntu there is no dpkg database to consult. In CI that is an error: the
      # published package must carry the real list.
      if [ -n "${CI:-}" ]; then
        echo "dpkg-shlibdeps found no dependencies" >&2
        exit 1
      fi
      echo "dpkg-shlibdeps failed; using a minimal Depends list" >&2
    fi
    # Debian sorts a pre-release before its release only with ~ (0.2.0~rc.1).
    cat > "$p/DEBIAN/control" <<CONTROL
Package: rustgis
Version: ${version/-/\~}
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
    # Bundle versions must be dot-separated numbers: drop a pre-release suffix (0.2.0-rc.1).
    sed "s/@VERSION@/${version%%-*}/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
    iconset="$work/rustgis.iconset"
    mkdir -p "$iconset"
    for s in 16 32 128 256 512; do
      logo "$iconset/icon_${s}x${s}.png" "$s"
      logo "$iconset/icon_${s}x${s}@2x.png" "$((s * 2))"
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/rustgis.icns"
    # Ad-hoc signature: required to launch on Apple Silicon; not notarized.
    codesign --force --sign - "$app"

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
    # Portable zip: unzip anywhere and run, no installer or admin rights.
    d="$work/$name"
    mkdir -p "$d"
    cp "$bin/rustgis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/README.md" "$d/"
    sed "s/@VERSION@/$version/g" "$root/packaging/windows/README-portable.txt" > "$d/README.txt"
    (cd "$work" && 7z a -tzip -bso0 "$dist/$name-portable.zip" "$name")

    # MSIX package for the Microsoft Store, which signs it on ingestion. The identity must
    # match the product reserved in Partner Center (Product identity); override it for a
    # self-signed sideload build.
    case "$arch" in x86_64) msixarch=x64 ;; aarch64) msixarch=arm64 ;; *) msixarch="$arch" ;; esac
    m="$work/msix"
    mkdir -p "$m/Assets"
    cp "$bin/rustgis.exe" "$bin/rustgis-cli.exe" "$m/"
    logo "$m/Assets/Square44x44Logo.png" 44
    logo "$m/Assets/Square150x150Logo.png" 150
    logo "$m/Assets/StoreLogo.png" 50
    # MSIX versions are four numbers: drop a pre-release suffix (0.2.0-rc.1 -> 0.2.0.0).
    sed -e "s/@VERSION@/${version%%-*}.0/g" -e "s/@ARCH@/$msixarch/g" \
      -e "s/@NAME@/${MSIX_NAME:-OpenGeospatialSolutions.RustGIS}/g" \
      -e "s/@PUBLISHER@/${MSIX_PUBLISHER:-CN=E6AE8172-DC4F-4F79-844B-9D84204BF95A}/g" \
      -e "s/@PUBLISHER_DISPLAY_NAME@/${MSIX_PUBLISHER_DISPLAY_NAME:-Open Geospatial Solutions}/g" \
      "$root/packaging/windows/AppxManifest.xml" > "$m/AppxManifest.xml"
    makeappx="$(command -v makeappx.exe || true)"
    if [ -z "$makeappx" ]; then
      makeappx="$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/makeappx.exe 2>/dev/null | sort -V | tail -1)"
    fi
    [ -n "$makeappx" ] || { echo "makeappx.exe not found: install the Windows SDK" >&2; exit 1; }
    MSYS2_ARG_CONV_EXCL='*' "$makeappx" pack /d "$(cygpath -w "$m")" /p "$(cygpath -w "$dist/$name.msix")" /o
    ;;
esac

ls -l "$dist"
