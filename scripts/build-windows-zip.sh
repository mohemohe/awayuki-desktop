#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
BINARY_NAME="awayuki"
VERSION="${VERSION:-0.1.0}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"

ZIP_NAME="${APP_NAME}-${VERSION}-x86_64.zip"
STAGING_DIR="${BUILD_DIR}/windows-staging"

echo "=== Building ${APP_NAME} v${VERSION} for Windows ==="

# Step 1: Build release binary
echo "--- cargo build --release ---"
cd "$PROJECT_ROOT"
cargo build --release

# Step 2: Stage files
echo "--- Staging files ---"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"

cp "${CARGO_TARGET_DIR}/release/${BINARY_NAME}.exe" "$STAGING_DIR/"

# Step 3: Find and bundle WinSparkle.dll
echo "--- Bundling WinSparkle.dll ---"
WINSPARKLE_DLL=""

CARGO_HOME="${CARGO_HOME:-${USERPROFILE}/.cargo}"
if [ -z "$WINSPARKLE_DLL" ]; then
    WINSPARKLE_DLL=$(find "${CARGO_HOME}/git/checkouts" -name "WinSparkle.dll" -path "*/winsparkle-sys/*" 2>/dev/null | head -1)
fi

if [ -n "$WINSPARKLE_DLL" ]; then
    echo "Found WinSparkle.dll: $WINSPARKLE_DLL"
    cp "$WINSPARKLE_DLL" "$STAGING_DIR/"
else
    echo "WARNING: WinSparkle.dll not found in cargo checkouts"
fi

# Step 4: Create ZIP
echo "--- Creating ZIP: $ZIP_NAME ---"
cd "$STAGING_DIR"
if command -v 7z &>/dev/null; then
    7z a -tzip "${BUILD_DIR}/${ZIP_NAME}" ./*
elif command -v zip &>/dev/null; then
    zip -r "${BUILD_DIR}/${ZIP_NAME}" ./*
else
    echo "Error: No zip tool found (tried 7z, zip)"
    exit 1
fi

rm -rf "$STAGING_DIR"

echo "Created: ${BUILD_DIR}/${ZIP_NAME}"
