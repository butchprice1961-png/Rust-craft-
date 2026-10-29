#!/bin/sh
# Builds a universal (Apple Silicon + Intel) RustCraft.app and
# dist/RustCraft-<version>-macos.dmg. Signs with $MACOS_SIGNING_IDENTITY
# when set (Developer ID, hardened runtime), ad hoc otherwise.
set -eu
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=dist/RustCraft.app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
for bin in rustcraft rustcraft-client; do
  if [ -f "target/aarch64-apple-darwin/release/$bin" ]; then
    lipo -create -output "$APP/Contents/MacOS/$bin" \
      "target/aarch64-apple-darwin/release/$bin" "target/x86_64-apple-darwin/release/$bin"
  fi
done
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp packaging/icons/rustcraft.icns "$APP/Contents/Resources/rustcraft.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE"
if [ -n "${MACOS_SIGNING_IDENTITY:-}" ]; then
  codesign --force --deep --options runtime --timestamp -s "$MACOS_SIGNING_IDENTITY" "$APP"
else
  codesign --force --deep -s - "$APP"
fi
DMG="dist/RustCraft-$VERSION-macos.dmg"
rm -f "$DMG"
hdiutil create -volname RustCraft -srcfolder "$APP" -ov -format UDZO "$DMG"
echo "$DMG"
