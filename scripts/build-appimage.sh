#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
BINARY_NAME="awayuki"
VERSION="${VERSION:-0.1.0}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"

APP_DIR="${BUILD_DIR}/AppDir"
APPIMAGE_NAME="${APP_NAME}-${VERSION}-x86_64.AppImage"

mkdir -p "$BUILD_DIR"

echo "=== Building ${APP_NAME} v${VERSION} AppImage ==="

# Step 1: Build release binary
echo "--- cargo build --release ---"
cd "$PROJECT_ROOT"
cargo build --release

# Step 2: Prepare staging files
echo "--- Preparing staging files ---"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR"

DESKTOP_FILE="${BUILD_DIR}/${BINARY_NAME}.desktop"
cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=${APP_NAME}
GenericName=Mastodon Client
Comment=A lightweight Mastodon client
Exec=${BINARY_NAME}
Icon=${BINARY_NAME}
Categories=Network;
Terminal=false
StartupWMClass=${APP_NAME}
EOF

# linuxdeploy expects the icon basename to match the desktop entry's Icon=,
# and rejects resolutions outside its allowlist (max 512x512). The source icon
# is 1024x1024, so resize on the way in.
ICON_FILE="${BUILD_DIR}/${BINARY_NAME}.png"
SOURCE_ICON="$PROJECT_ROOT/assets/icons/AppIcon.png"
if command -v magick >/dev/null 2>&1; then
    magick "$SOURCE_ICON" -resize 512x512 "$ICON_FILE"
elif command -v convert >/dev/null 2>&1; then
    convert "$SOURCE_ICON" -resize 512x512 "$ICON_FILE"
else
    echo "ERROR: ImageMagick (magick or convert) is required to resize the icon" >&2
    exit 1
fi

# Step 3: Download linuxdeploy if not present
LINUXDEPLOY="${BUILD_DIR}/linuxdeploy-x86_64.AppImage"
if [ ! -f "$LINUXDEPLOY" ]; then
    echo "--- Downloading linuxdeploy ---"
    wget -q -O "$LINUXDEPLOY" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
    chmod +x "$LINUXDEPLOY"
fi

# Step 4: Build AppImage
# GitHub-hosted runners no longer ship FUSE2; the AppImage must self-extract
# rather than mounting via libfuse.
echo "--- Running linuxdeploy ---"
cd "$BUILD_DIR"
APPIMAGE_EXTRACT_AND_RUN=1 \
OUTPUT="$APPIMAGE_NAME" \
    "$LINUXDEPLOY" \
        --appdir "$APP_DIR" \
        --executable "${CARGO_TARGET_DIR}/release/${BINARY_NAME}" \
        --desktop-file "$DESKTOP_FILE" \
        --icon-file "$ICON_FILE" \
        --output appimage

echo "Created: ${BUILD_DIR}/${APPIMAGE_NAME}"
