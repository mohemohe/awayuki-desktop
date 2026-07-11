#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
BINARY_NAME="awayuki"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$PROJECT_ROOT/Cargo.toml" | head -1)}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"

APP_DIR="${BUILD_DIR}/AppDir"
APPIMAGE_NAME="${APP_NAME}-${VERSION}-x86_64.AppImage"
LINUXDEPLOY_VERSION="1-alpha-20251107-1"
LINUXDEPLOY_SHA256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/${LINUXDEPLOY_VERSION}/linuxdeploy-x86_64.AppImage"

verify_sha256() {
    local file="$1"
    local expected="$2"
    local actual

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    else
        echo "ERROR: sha256sum or shasum is required to verify downloaded tools" >&2
        return 1
    fi

    if [ "$actual" != "$expected" ]; then
        echo "ERROR: SHA-256 mismatch for $file" >&2
        echo "Expected: $expected" >&2
        echo "Actual:   $actual" >&2
        return 1
    fi
}

mkdir -p "$BUILD_DIR"

echo "=== Building ${APP_NAME} v${VERSION} AppImage ==="

# Step 1: Build frontend assets
echo "--- bun run build ---"
cd "$PROJECT_ROOT"
bun run build

# Step 2: Build release binary
echo "--- cargo build --release ---"
cargo build --locked --release

# Step 3: Prepare staging files
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

# Step 4: Download linuxdeploy if not present
LINUXDEPLOY="${BUILD_DIR}/linuxdeploy-x86_64.AppImage"
if [ ! -f "$LINUXDEPLOY" ]; then
    echo "--- Downloading linuxdeploy ${LINUXDEPLOY_VERSION} ---"
    LINUXDEPLOY_DOWNLOAD="${LINUXDEPLOY}.download"
    rm -f "$LINUXDEPLOY_DOWNLOAD"
    wget -q --https-only -O "$LINUXDEPLOY_DOWNLOAD" "$LINUXDEPLOY_URL"
    if ! verify_sha256 "$LINUXDEPLOY_DOWNLOAD" "$LINUXDEPLOY_SHA256"; then
        rm -f "$LINUXDEPLOY_DOWNLOAD"
        exit 1
    fi
    mv "$LINUXDEPLOY_DOWNLOAD" "$LINUXDEPLOY"
    chmod +x "$LINUXDEPLOY"
fi
verify_sha256 "$LINUXDEPLOY" "$LINUXDEPLOY_SHA256"
chmod +x "$LINUXDEPLOY"

# Step 5: Build AppImage
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
