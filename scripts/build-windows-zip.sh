#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
BINARY_NAME="awayuki"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$PROJECT_ROOT/Cargo.toml" | head -1)}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"

ZIP_NAME="${APP_NAME}-${VERSION}-windows-x86_64.zip"
STAGING_DIR="${BUILD_DIR}/windows-staging"

echo "=== Building ${APP_NAME} v${VERSION} for Windows ==="

# Step 1: Build frontend assets
echo "--- bun run build ---"
cd "$PROJECT_ROOT"
bun run build

# Step 2: Build release binary
echo "--- cargo build --release ---"
cargo build --locked --release

# Step 3: Stage files
echo "--- Staging files ---"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"

cp "${CARGO_TARGET_DIR}/release/${BINARY_NAME}.exe" "$STAGING_DIR/"

# Step 4: Bundle WinSparkle.dll. CI places the official x64 DLL in build/ after
# replacing the x86 import library shipped by winsparkle-sys.
echo "--- Bundling WinSparkle.dll ---"
WINSPARKLE_DLL=""
if [ -f "${BUILD_DIR}/WinSparkle.dll" ]; then
    WINSPARKLE_DLL="${BUILD_DIR}/WinSparkle.dll"
elif [ -n "${CARGO_HOME:-}" ]; then
    WINSPARKLE_DLL="$(find "${CARGO_HOME}/git/checkouts" \
        -name "WinSparkle.dll" -path "*/winsparkle-sys/*" \
        -print -quit 2>/dev/null || true)"
fi
if [ -z "$WINSPARKLE_DLL" ]; then
    echo "Error: WinSparkle.dll not found" >&2
    exit 1
fi
cp "$WINSPARKLE_DLL" "$STAGING_DIR/"

# Step 5: Create ZIP.
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
