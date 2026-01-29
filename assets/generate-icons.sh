#!/bin/bash
# Generate icon files from the SVG logo
# Usage: ./generate-icons.sh
#
# Requires: Inkscape and ImageMagick (convert)

set -e
cd "$(dirname "$0")"

SVG="termstack-logo.svg"

# Check for required tools
if ! command -v inkscape &>/dev/null; then
    echo "Error: inkscape required for SVG export with transparency" >&2
    exit 1
fi
if ! command -v convert &>/dev/null; then
    echo "Error: ImageMagick (convert) required for BGRA conversion" >&2
    exit 1
fi

convert_svg() {
    local size=$1
    local output_png=$2
    local output_raw=$3

    # Use Inkscape for proper transparency handling
    inkscape "$SVG" --export-type=png --export-filename="$output_png" \
        --export-width="$size" --export-height="$size" 2>/dev/null

    # Convert PNG to raw BGRA for X11 _NET_WM_ICON
    if [ -n "$output_raw" ]; then
        convert "$output_png" -channel RGBA -separate -swap 0,2 -combine "BGRA:$output_raw"
    fi

    echo "Generated: $output_png"
}

# Generate icons for X11 _NET_WM_ICON (raw BGRA format)
for size in 48 64 128 256; do
    png_tmp="/tmp/termstack-icon-${size}.png"
    convert_svg "$size" "$png_tmp" "termstack-icon-${size}.raw"
done

# Generate PNG for desktop file installation (use 256px)
mkdir -p icons/hicolor/256x256/apps
convert_svg 256 "icons/hicolor/256x256/apps/termstack.png" ""

echo "Done. Rebuild termstack to use new icons."
