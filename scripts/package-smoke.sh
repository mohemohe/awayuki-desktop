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
install_root="$scratch/install"
state_root="$scratch/state"
launch_log="$scratch/launch.log"
mkdir -p "$install_root" "$state_root/home" "$state_root/data" \
    "$state_root/config" "$state_root/cache"

cleanup() {
    stop_app
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
        "${launch_prefix[@]}" "$executable" >> "$launch_log" 2>&1 &
    app_pid="$!"
    sleep 2
    if ! kill -0 "$app_pid" 2>/dev/null; then
        wait "$app_pid" || true
        echo "Packaged application exited before $label became observable:" >&2
        sed -n '1,200p' "$launch_log" >&2
        return 1
    fi
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

launch_prefix=()
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
        ;;
    windows)
        mkdir -p "$install_root/package"
        7z x -y "-o$install_root/package" "$artifact" >/dev/null
        executable="$install_root/package/awayuki.exe"
        test -s "$executable"
        assert_clean_tree "$install_root/package"
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
        launch_prefix=(xvfb-run -a)
        ;;
    *)
        echo "Unsupported package smoke platform: $platform" >&2
        exit 1
        ;;
esac

# 1. A clean profile must create and migrate exactly one functional SQLite DB.
start_app "fresh database launch"
database="$(wait_for_database verify-fresh)"
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
    "$database" "$report" "$platform" "$artifact" true "$binary_bytes"

echo "$platform package fresh/upgrade/restart/uninstall smoke passed"
