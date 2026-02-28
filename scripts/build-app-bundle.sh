#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

APP_NAME="Awayuki"
BUNDLE_NAME="${APP_NAME}.app"
BINARY_NAME="awayuki"

SIGN_IDENTITY="${SIGN_IDENTITY:-}"
VERSION="${VERSION:-0.1.0}"
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"

BUNDLE_DIR="${BUILD_DIR}/${BUNDLE_NAME}"
CONTENTS_DIR="${BUNDLE_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
FRAMEWORKS_DIR="${CONTENTS_DIR}/Frameworks"

echo "=== Building ${APP_NAME} v${VERSION} ==="

# Step 1: Build release binary
echo "--- cargo build --release ---"
cd "$PROJECT_ROOT"
cargo build --release

# Step 2: Generate icns if needed
ICNS_FILE="${BUILD_DIR}/AppIcon.icns"
if [ ! -f "$ICNS_FILE" ]; then
    echo "--- Generating icns icon ---"
    bash "$SCRIPT_DIR/create-icns.sh"
fi

# Step 3: Assemble .app bundle
echo "--- Assembling .app bundle ---"
rm -rf "$BUNDLE_DIR"
mkdir -p "$MACOS_DIR"
mkdir -p "$RESOURCES_DIR"
mkdir -p "$FRAMEWORKS_DIR"

cp "${CARGO_TARGET_DIR}/release/${BINARY_NAME}" "$MACOS_DIR/"
cp "$PROJECT_ROOT/resources/Info.plist" "$CONTENTS_DIR/"
cp "$ICNS_FILE" "$RESOURCES_DIR/AppIcon.icns"

# Step 3a: Update Info.plist version from VERSION env var
echo "--- Setting version to ${VERSION} ---"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" "$CONTENTS_DIR/Info.plist"

# Step 3b: Bundle Sparkle.framework
echo "--- Bundling Sparkle.framework ---"
SPARKLE_FRAMEWORK_SRC=""

# Search in cargo git checkouts (git dependency)
if [ -z "$SPARKLE_FRAMEWORK_SRC" ]; then
    SPARKLE_FRAMEWORK_SRC=$(find "${HOME}/.cargo/git/checkouts" -path "*/sparkle-sys/Sparkle.framework" -maxdepth 6 -type d 2>/dev/null | head -1)
fi

# Fallback: search in cargo registry (crates.io)
if [ -z "$SPARKLE_FRAMEWORK_SRC" ]; then
    SPARKLE_FRAMEWORK_SRC=$(find "${HOME}/.cargo/registry/src" -path "*/sparkle-sys-*/Sparkle.framework" -maxdepth 5 -type d 2>/dev/null | head -1)
fi

if [ -n "$SPARKLE_FRAMEWORK_SRC" ]; then
    echo "Found Sparkle.framework: $SPARKLE_FRAMEWORK_SRC"
    cp -R "$SPARKLE_FRAMEWORK_SRC" "$FRAMEWORKS_DIR/"
else
    echo "ERROR: Sparkle.framework not found in cargo checkouts or registry!"
    exit 1
fi

# Step 3c: Add rpath so the binary can find Sparkle.framework at runtime
echo "--- Setting rpath ---"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$MACOS_DIR/$BINARY_NAME" 2>/dev/null || true

# Step 4: Code sign
echo "--- Code signing ---"
ENTITLEMENTS="$PROJECT_ROOT/resources/Entitlements.plist"

if [ -n "$SIGN_IDENTITY" ]; then
    echo "Signing with identity: $SIGN_IDENTITY"

    # 4a: Sign fileop helper inside Autoupdate.app
    if [ -f "$FRAMEWORKS_DIR/Sparkle.framework/Versions/A/Resources/Autoupdate.app/Contents/MacOS/fileop" ]; then
        /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
            --options runtime \
            --timestamp \
            "$FRAMEWORKS_DIR/Sparkle.framework/Versions/A/Resources/Autoupdate.app/Contents/MacOS/fileop"
    fi

    # 4b: Sign Autoupdate.app bundle
    if [ -d "$FRAMEWORKS_DIR/Sparkle.framework/Versions/A/Resources/Autoupdate.app" ]; then
        /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
            --options runtime \
            --timestamp \
            "$FRAMEWORKS_DIR/Sparkle.framework/Versions/A/Resources/Autoupdate.app"
    fi

    # 4c: Sign Sparkle dylib
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --timestamp \
        "$FRAMEWORKS_DIR/Sparkle.framework/Versions/A/Sparkle"

    # 4d: Sign Sparkle.framework
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --timestamp \
        "$FRAMEWORKS_DIR/Sparkle.framework"

    # 4e: Sign main binary
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --timestamp \
        "$MACOS_DIR/$BINARY_NAME"

    # 4f: Sign the entire bundle
    /usr/bin/codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --timestamp \
        "$BUNDLE_DIR"
else
    echo "Ad-hoc signing (no SIGN_IDENTITY provided)"
    if [ -d "$FRAMEWORKS_DIR/Sparkle.framework" ]; then
        /usr/bin/codesign --force --sign - \
            "$FRAMEWORKS_DIR/Sparkle.framework"
    fi

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
