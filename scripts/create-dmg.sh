#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
VERSION="${VERSION:-0.1.0}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
BUNDLE_DIR="${BUILD_DIR}/${APP_NAME}.app"
DMG_NAME="${APP_NAME}-${VERSION}-arm64.dmg"
SIGN_IDENTITY="${SIGN_IDENTITY:-}"

if [ ! -d "$BUNDLE_DIR" ]; then
    echo "Error: $BUNDLE_DIR not found. Run build-app-bundle.sh first."
    exit 1
fi

# Create DMG
echo "--- Creating DMG: $DMG_NAME ---"
DMG_TEMP="${BUILD_DIR}/dmg-temp"
rm -rf "$DMG_TEMP"
mkdir -p "$DMG_TEMP"
cp -R "$BUNDLE_DIR" "$DMG_TEMP/"
ln -s /Applications "$DMG_TEMP/Applications"

hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$DMG_TEMP" \
    -ov \
    -format UDZO \
    "${BUILD_DIR}/${DMG_NAME}"

rm -rf "$DMG_TEMP"

# Sign DMG if identity provided
if [ -n "$SIGN_IDENTITY" ]; then
    echo "--- Signing DMG ---"
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --timestamp \
        "${BUILD_DIR}/${DMG_NAME}"
fi

echo "Created: ${BUILD_DIR}/${DMG_NAME}"
