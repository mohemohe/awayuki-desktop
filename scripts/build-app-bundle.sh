#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
BUNDLE_NAME="${APP_NAME}.app"
BINARY_NAME="awayuki"

SIGN_IDENTITY="${SIGN_IDENTITY:-}"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$PROJECT_ROOT/Cargo.toml" | head -1)}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"

# Cargo may resolve `target-dir` from ~/.cargo/config.toml even when the
# CARGO_TARGET_DIR environment variable is unset. Use Cargo's own resolved
# directory for both the build and bundle assembly so that we never copy a
# stale binary from ./target.
CARGO_TARGET_DIR="$({
    cd "$PROJECT_ROOT"
    cargo metadata --locked --no-deps --format-version 1
} | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
if [ -z "$CARGO_TARGET_DIR" ]; then
    echo "Failed to resolve Cargo target directory" >&2
    exit 1
fi
export CARGO_TARGET_DIR

BUNDLE_DIR="${BUILD_DIR}/${BUNDLE_NAME}"
CONTENTS_DIR="${BUNDLE_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"

echo "=== Building ${APP_NAME} v${VERSION} ==="
echo "Cargo target directory: ${CARGO_TARGET_DIR}"

# Step 1: Build frontend assets
echo "--- bun run build ---"
cd "$PROJECT_ROOT"
bun run build

# Step 2: Build release binary
echo "--- cargo build --release ---"
cargo build --locked --release

# Step 3: Generate icns if needed
ICNS_FILE="${BUILD_DIR}/AppIcon.icns"
if [ ! -f "$ICNS_FILE" ]; then
    echo "--- Generating icns icon ---"
    bash "$SCRIPT_DIR/create-icns.sh"
fi

# Step 4: Assemble .app bundle
echo "--- Assembling .app bundle ---"
rm -rf "$BUNDLE_DIR"
mkdir -p "$MACOS_DIR"
mkdir -p "$RESOURCES_DIR"

cp "${CARGO_TARGET_DIR}/release/${BINARY_NAME}" "$MACOS_DIR/"
cp "$PROJECT_ROOT/resources/Info.plist" "$CONTENTS_DIR/"
cp "$ICNS_FILE" "$RESOURCES_DIR/AppIcon.icns"

# Step 4a: Update Info.plist version from VERSION env var
echo "--- Setting version to ${VERSION} ---"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" "$CONTENTS_DIR/Info.plist"

# Step 4b: Add the Swift runtime fallback required by FoundationModels.
echo "--- Setting Swift runtime rpath ---"
install_name_tool -add_rpath "/usr/lib/swift" "$MACOS_DIR/$BINARY_NAME" 2>/dev/null || true

# Step 5: Code sign
echo "--- Code signing ---"
ENTITLEMENTS="$PROJECT_ROOT/resources/Entitlements.plist"

if [ -n "$SIGN_IDENTITY" ]; then
    echo "Signing with identity: $SIGN_IDENTITY"

    # Sign main binary.
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --timestamp \
        "$MACOS_DIR/$BINARY_NAME"

    # Sign the entire bundle.
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --timestamp \
        "$BUNDLE_DIR"
else
    echo "Ad-hoc signing (no SIGN_IDENTITY provided)"
    /usr/bin/codesign --force --sign - \
        --entitlements "$ENTITLEMENTS" \
        "$MACOS_DIR/$BINARY_NAME"

    /usr/bin/codesign --force --sign - \
        --entitlements "$ENTITLEMENTS" \
        "$BUNDLE_DIR"
fi

echo "--- Verifying signature ---"
codesign -vv --display "$BUNDLE_DIR" 2>&1

echo "=== Bundle created: $BUNDLE_DIR ==="
