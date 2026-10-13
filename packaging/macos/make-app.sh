#!/usr/bin/env bash
# Builds Veta.app and a .dmg from release binaries. Runs on macOS.
#   packaging/macos/make-app.sh <dir with veta and veta-cli> <version> <build> <out.dmg>
# veta-cli is placed in Veta.app/Contents/MacOS; link it into your PATH:
#   ln -s /Applications/Veta.app/Contents/MacOS/veta-cli /usr/local/bin/
set -euo pipefail
src=$1 version=$2 build=$3 out=$4
here="$(cd "$(dirname "$0")" && pwd)"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/Veta.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$src/veta" "$src/veta-cli" "$app/Contents/MacOS/"
cp "$here/../icons/veta.icns" "$app/Contents/Resources/veta.icns"
sed -e "s/@VERSION@/$version/" -e "s/@BUILD@/$build/" "$here/Info.plist" > "$app/Contents/Info.plist"
# Unsigned: ad-hoc sign so Apple Silicon runs it (first launch: right-click → Open).
codesign --force --deep --sign - "$app" || true
mkdir "$work/dmg"
mv "$app" "$work/dmg/"
ln -s /Applications "$work/dmg/Applications"
rm -f "$out"
hdiutil create -volname "Veta $version" -srcfolder "$work/dmg" -ov -format UDZO "$out"
