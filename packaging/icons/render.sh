#!/usr/bin/env bash
# Renders veta.svg into the icon files used by the builds. Run after editing
# the SVG; the outputs are committed so builds don't need these tools.
# Needs rsvg-convert, ImageMagick (convert) and python3.
set -euo pipefail
cd "$(dirname "$0")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for size in 16 24 32 48 64 128 256 512 1024; do
    rsvg-convert -w "$size" -h "$size" veta.svg -o "$tmp/$size.png"
done
cp "$tmp/256.png" veta-256.png
cp "$tmp/512.png" veta-512.png
# Windows icon with the usual sizes.
convert "$tmp"/{16,24,32,48,64,128,256}.png veta.ico
# Raw RGBA for the window icon (iced takes pixels, not PNG).
convert "$tmp/64.png" -depth 8 rgba:veta-64.rgba
# macOS icon: an ICNS container of PNGs.
python3 - "$tmp" <<'PY'
import struct, sys
tmp = sys.argv[1]
chunks = b""
for kind, size in [(b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024)]:
    data = open(f"{tmp}/{size}.png", "rb").read()
    chunks += kind + struct.pack(">I", len(data) + 8) + data
open("veta.icns", "wb").write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
PY
ls -l veta.ico veta.icns veta-256.png veta-512.png veta-64.rgba
