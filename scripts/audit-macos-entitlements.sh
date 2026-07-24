#!/usr/bin/env bash
set -euo pipefail

APP_PATH="${1:?usage: audit-macos-entitlements.sh /path/to/Awayuki.app [output.plist]}"
OUTPUT_PATH="${2:-build/macos-entitlements.plist}"

mkdir -p "$(dirname "$OUTPUT_PATH")"
codesign -d --entitlements :- "$APP_PATH" >"$OUTPUT_PATH" 2>/dev/null

for forbidden in \
  com.apple.security.cs.allow-unsigned-executable-memory \
  com.apple.security.cs.disable-library-validation \
  com.apple.security.cs.allow-dyld-environment-variables \
  com.apple.security.get-task-allow
do
  if /usr/libexec/PlistBuddy -c "Print :$forbidden" "$OUTPUT_PATH" >/dev/null 2>&1; then
    echo "forbidden entitlement is enabled: $forbidden" >&2
    exit 1
  fi
done

for required in \
  com.apple.security.cs.allow-jit \
  com.apple.security.network.client
do
  value="$(/usr/libexec/PlistBuddy -c "Print :$required" "$OUTPUT_PATH")"
  if [[ "$value" != "true" ]]; then
    echo "required entitlement is missing: $required" >&2
    exit 1
  fi
done
