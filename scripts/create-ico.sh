#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
INPUT_PNG="$PROJECT_ROOT/assets/icons/AppIcon.png"
OUTPUT_ICO="$PROJECT_ROOT/build/AppIcon.ico"

if [ ! -f "$INPUT_PNG" ]; then
    echo "Error: $INPUT_PNG not found"
    echo "Place a 1024x1024 PNG icon at assets/icons/AppIcon.png"
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT_ICO")"

magick "$INPUT_PNG" \
    \( -clone 0 -resize 16x16 \) \
    \( -clone 0 -resize 32x32 \) \
    \( -clone 0 -resize 48x48 \) \
    \( -clone 0 -resize 64x64 \) \
    \( -clone 0 -resize 128x128 \) \
    \( -clone 0 -resize 256x256 \) \
    -delete 0 \
    "$OUTPUT_ICO"

echo "Created: $OUTPUT_ICO"
