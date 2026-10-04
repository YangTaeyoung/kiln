#!/usr/bin/env bash
# Rebuild every platform icon from the editable vector source. Requires ImageMagick.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v magick >/dev/null
ICON_TMP=$(mktemp -d)
trap 'rm -rf "$ICON_TMP"' EXIT
magick -background none -density 384 assets/Kiln.svg -resize 1024x1024 assets/Kiln.png
mkdir "$ICON_TMP/Kiln.iconset"
for size in 16 32 128 256 512; do
  magick assets/Kiln.png -resize "${size}x${size}" "$ICON_TMP/Kiln.iconset/icon_${size}x${size}.png"
  double=$((size * 2))
  magick assets/Kiln.png -resize "${double}x${double}" "$ICON_TMP/Kiln.iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$ICON_TMP/Kiln.iconset" -o assets/Kiln.icns
magick assets/Kiln.png -define icon:auto-resize=256,128,64,48,32,16 assets/Kiln.ico
