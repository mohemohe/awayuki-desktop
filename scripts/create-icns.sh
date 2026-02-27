#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
INPUT_PNG="$PROJECT_ROOT/assets/icons/AppIcon.png"
ICONSET_DIR="$PROJECT_ROOT/build/AppIcon.iconset"
OUTPUT_ICNS="$PROJECT_ROOT/build/AppIcon.icns"

if [ ! -f "$INPUT_PNG" ]; then
    echo "Error: $INPUT_PNG not found"
    echo "Place a 1024x1024 PNG icon at assets/icons/AppIcon.png"
    exit 1
fi

mkdir -p "$ICONSET_DIR"

sips -z 16 16     "$INPUT_PNG" --out "$ICONSET_DIR/icon_16x16.png"
sips -z 32 32     "$INPUT_PNG" --out "$ICONSET_DIR/icon_16x16@2x.png"
sips -z 32 32     "$INPUT_PNG" --out "$ICONSET_DIR/icon_32x32.png"
sips -z 64 64     "$INPUT_PNG" --out "$ICONSET_DIR/icon_32x32@2x.png"
sips -z 128 128   "$INPUT_PNG" --out "$ICONSET_DIR/icon_128x128.png"
sips -z 256 256   "$INPUT_PNG" --out "$ICONSET_DIR/icon_128x128@2x.png"
sips -z 256 256   "$INPUT_PNG" --out "$ICONSET_DIR/icon_256x256.png"
sips -z 512 512   "$INPUT_PNG" --out "$ICONSET_DIR/icon_256x256@2x.png"
sips -z 512 512   "$INPUT_PNG" --out "$ICONSET_DIR/icon_512x512.png"
sips -z 1024 1024 "$INPUT_PNG" --out "$ICONSET_DIR/icon_512x512@2x.png"

iconutil -c icns "$ICONSET_DIR" -o "$OUTPUT_ICNS"

echo "Created: $OUTPUT_ICNS"
