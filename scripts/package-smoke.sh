#!/usr/bin/env bash
set -euo pipefail

platform="${1:?usage: package-smoke.sh PLATFORM ARTIFACT}"
artifact="${2:?usage: package-smoke.sh PLATFORM ARTIFACT}"

project_root="$(cd "$(dirname "$0")/.." && pwd)"
artifact_dir="$(cd "$(dirname "$artifact")" && pwd)"
artifact="$artifact_dir/$(basename "$artifact")"
report="${PACKAGE_SMOKE_REPORT:-$project_root/build/package-smoke-$platform.json}"

if [[ ! -s "$artifact" ]]; then
    echo "Package is missing or empty: $artifact" >&2
    exit 1
fi
if ! command -v bun >/dev/null 2>&1; then
    echo "Bun is required by the deterministic SQLite package fixture" >&2
    exit 1
fi

scratch="$(mktemp -d)"
mounted=""
app_pid=""
security_server_pid=""
security_smoke_url=""
install_root="$scratch/install"
state_root="$scratch/state"
launch_log="$scratch/launch.log"
mkdir -p "$install_root" "$state_root/home" "$state_root/data" \
    "$state_root/config" "$state_root/cache"

cleanup() {
    stop_app
    if [[ -n "${security_server_pid:-}" ]] && kill -0 "$security_server_pid" 2>/dev/null; then
        kill "$security_server_pid" 2>/dev/null || true
        wait "$security_server_pid" 2>/dev/null || true
    fi
    if [[ -n "$mounted" ]]; then
        hdiutil detach "$mounted" -quiet 2>/dev/null || true
    fi
    rm -rf "$scratch"
}
trap cleanup EXIT

stop_app() {
    if [[ -n "${app_pid:-}" ]] && kill -0 "$app_pid" 2>/dev/null; then
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    fi
    app_pid=""
}

assert_clean_tree() {
    local root="$1"
    local forbidden
    forbidden="$(find "$root" \
        \( -name node_modules -o -name target -o -name .git -o -name .env \
           -o -name '*.p12' -o -name '*.pem' -o -name awayuki.db \
           -o -name frontend -o -name mock.ts \) -print -quit)"
    if [[ -n "$forbidden" ]]; then
        echo "Forbidden build/source/private file in package: $forbidden" >&2
        exit 1
    fi
}

assert_starryeyes_license() {
    local license_file="$1"
    if [[ ! -s "$license_file" ]]; then
        echo "StarryEyes MIT notice is missing: $license_file" >&2
        exit 1
    fi
    if ! cmp -s "$project_root/LICENSES/StarryEyes-MIT.txt" "$license_file"; then
        echo "Packaged StarryEyes MIT notice differs from the source notice" >&2
        exit 1
    fi
    grep -Fqx \
        'Audited revision: a2c4c9b68287c9058d82a15cd28c6615863a626f' \
        "$license_file"
    grep -Fqx 'The MIT License (MIT)' "$license_file"
    grep -Fqx 'Copyright (c) 2013 Karno.' "$license_file"
}

start_app() {
    local label="$1"
    echo "=== $label ===" >> "$launch_log"
    HOME="$state_root/home" \
    USERPROFILE="$state_root/home" \
    XDG_DATA_HOME="$state_root/data" \
    XDG_CONFIG_HOME="$state_root/config" \
    XDG_CACHE_HOME="$state_root/cache" \
    APPDATA="$state_root/data" \
    LOCALAPPDATA="$state_root/data" \
    AWAYUKI_RELEASE_SECURITY_SMOKE=1 \
    AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL="$security_smoke_url" \
        "${launch_command[@]}" >> "$launch_log" 2>&1 &
    app_pid="$!"
    sleep 2
    if ! kill -0 "$app_pid" 2>/dev/null; then
        wait "$app_pid" || true
        echo "Packaged application exited before $label became observable:" >&2
        sed -n '1,200p' "$launch_log" >&2
        return 1
    fi
}

assert_release_security_attestation() {
    local expected='AWAYUKI_RELEASE_SECURITY_REPORT release_build=true csp_deny_default=true csp_external_connect=false csp_remote_media=true'
    if ! grep -Fq "$expected" "$launch_log"; then
        echo "Packaged release did not attest the expected CSP policy:" >&2
        sed -n '1,240p' "$launch_log" >&2
        return 1
    fi
}

start_release_webview_smoke_server() {
    local port_file="$scratch/security-smoke.port"
    bun "$project_root/scripts/release-webview-smoke-server.mjs" "$port_file" \
        >>"$launch_log" 2>&1 &
    security_server_pid="$!"
    for _ in $(seq 1 100); do
        if [[ -s "$port_file" ]]; then
            security_smoke_url="http://127.0.0.1:$(tr -d '[:space:]' < "$port_file")"
            return 0
        fi
        if ! kill -0 "$security_server_pid" 2>/dev/null; then
            echo "Release WebView smoke server exited before publishing its port" >&2
            sed -n '1,240p' "$launch_log" >&2
            return 1
        fi
        sleep 0.05
    done
    echo "Timed out starting release WebView smoke server" >&2
    return 1
}

assert_release_webview_smoke() {
    local expected=(
        'AWAYUKI_WEBVIEW_SECURITY_REPORT'
        '"imageLoaded":true'
        '"protocolMediaLoaded":true'
        '"customEmojiLoaded":true'
        '"videoLoaded":true'
        '"sidecarCreated":true'
        '"sidecarHiddenDuringPreview":true'
        '"sidecarRestored":true'
        '"sidecarClosed":true'
        '"cspViolationCount":0'
    )
    for _ in $(seq 1 120); do
        local complete=true
        for marker in "${expected[@]}"; do
            if ! grep -Fq "$marker" "$launch_log"; then
                complete=false
                break
            fi
        done
        if [[ "$complete" == true ]]; then
            return 0
        fi
        if ! kill -0 "$app_pid" 2>/dev/null; then
            break
        fi
        sleep 0.25
    done
    echo "Packaged WebView media/sidecar smoke did not complete:" >&2
    sed -n '1,300p' "$launch_log" >&2
    return 1
}

locate_database() {
    find "$state_root" -type f -name awayuki.db -print -quit
}

wait_for_database() {
    local mode="$1"
    local attempts="${2:-180}"
    local database=""
    for ((attempt = 1; attempt <= attempts; attempt += 1)); do
        database="$(locate_database)"
        if [[ -n "$database" ]] && \
            bun "$project_root/scripts/package-db-fixture.mjs" \
                "$mode" "$database" >/dev/null 2>&1; then
            printf '%s\n' "$database"
            return 0
        fi
        if ! kill -0 "$app_pid" 2>/dev/null; then
            wait "$app_pid" || true
            echo "Packaged application exited during $mode:" >&2
            sed -n '1,200p' "$launch_log" >&2
            return 1
        fi
        sleep 1
    done
    echo "Timed out waiting for package database during $mode" >&2
    sed -n '1,240p' "$launch_log" >&2
    return 1
}

case "$platform" in
    macos)
        mounted="$scratch/mount"
        mkdir -p "$mounted"
        hdiutil attach -nobrowse -readonly -mountpoint "$mounted" "$artifact" >/dev/null
        source_app="$mounted/Awayuki.app"
        mkdir -p "$install_root/Applications"
        ditto "$source_app" "$install_root/Applications/Awayuki.app"
        app="$install_root/Applications/Awayuki.app"
        executable="$app/Contents/MacOS/awayuki"
        test -x "$executable"
        codesign -vv --deep --strict "$app"
        xcrun stapler validate "$app"
        assert_clean_tree "$app"
        assert_starryeyes_license \
            "$app/Contents/Resources/LICENSES/StarryEyes-MIT.txt"
        launch_command=("$executable")
        ;;
    windows)
        mkdir -p "$install_root/package"
        7z x -y "-o$install_root/package" "$artifact" >/dev/null
        executable="$install_root/package/awayuki.exe"
        test -s "$executable"
        assert_clean_tree "$install_root/package"
        assert_starryeyes_license \
            "$install_root/package/LICENSES/StarryEyes-MIT.txt"
        launch_command=("$executable")
        ;;
    linux)
        cp "$artifact" "$install_root/Awayuki.AppImage"
        chmod +x "$install_root/Awayuki.AppImage"
        (
            cd "$install_root"
            ./Awayuki.AppImage --appimage-extract >/dev/null
        )
        root="$install_root/squashfs-root"
        executable="$root/AppRun"
        test -x "$executable"
        assert_clean_tree "$root"
        assert_starryeyes_license \
            "$root/usr/share/licenses/awayuki/StarryEyes-MIT.txt"
        launch_command=(xvfb-run -a "$executable")
        ;;
    *)
        echo "Unsupported package smoke platform: $platform" >&2
        exit 1
        ;;
esac

start_release_webview_smoke_server

# 1. A clean profile must create and migrate exactly one functional SQLite DB.
start_app "fresh database launch"
database="$(wait_for_database verify-fresh)"
assert_release_security_attestation
assert_release_webview_smoke
stop_app
bun "$project_root/scripts/package-db-fixture.mjs" verify-fresh "$database"
test "$(find "$state_root" -type f -name '*.db' | wc -l | tr -d ' ')" -eq 1

# 2. Replace only awayuki.db with a migration-019 fixture. No side backup or
# OS credential store participates in the upgrade.
bun "$project_root/scripts/package-db-fixture.mjs" create-legacy "$database"
start_app "legacy database upgrade"
database="$(wait_for_database verify-upgraded)"
stop_app
bun "$project_root/scripts/package-db-fixture.mjs" verify-upgraded "$database"

# 3. Restart once more and prove the upgraded state and SQLite credential row
# survive without a recovery copy.
start_app "upgraded database restart"
database="$(wait_for_database verify-upgraded)"
stop_app
bun "$project_root/scripts/package-db-fixture.mjs" verify-upgraded "$database"

# 4. Model uninstall by removing the installed payload, not the isolated user
# data directory. The package binary must disappear while awayuki.db remains.
executable_before_uninstall="$executable"
binary_bytes="$(wc -c < "$executable" | tr -d ' ')"
rm -rf "$install_root"
test ! -e "$executable_before_uninstall"
test -s "$database"
bun "$project_root/scripts/package-db-fixture.mjs" report \
    "$database" "$report" "$platform" "$artifact" true "$binary_bytes" true true

echo "$platform package fresh/upgrade/restart/uninstall smoke passed"
