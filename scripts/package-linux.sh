#!/bin/sh
# Builds dist/RustCraft-<version>-x86_64.AppImage from release binaries.
# Usage: scripts/package-linux.sh [path/to/appimagetool]
set -eu
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TOOL=${1:-appimagetool}
APPDIR=dist/AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/512x512/apps"
for bin in rustcraft rustcraft-client; do
  [ -f "target/release/$bin" ] && install -m 755 "target/release/$bin" "$APPDIR/usr/bin/$bin"
done
install -m 644 packaging/linux/rustcraft.desktop "$APPDIR/rustcraft.desktop"
install -m 644 packaging/linux/rustcraft.desktop "$APPDIR/usr/share/applications/rustcraft.desktop"
install -m 644 packaging/icons/rustcraft.png "$APPDIR/rustcraft.png"
install -m 644 packaging/icons/rustcraft.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/rustcraft.png"
install -m 644 LICENSE "$APPDIR/usr/share/LICENSE"
ln -sf usr/bin/rustcraft "$APPDIR/AppRun"
OUT="dist/RustCraft-$VERSION-x86_64.AppImage"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$APPDIR" "$OUT"
chmod +x "$OUT"
echo "$OUT"
