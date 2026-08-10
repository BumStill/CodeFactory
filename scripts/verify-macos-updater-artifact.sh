#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS updater artifact verification requires macOS" >&2
  exit 2
fi

if [[ $# -ne 5 ]]; then
  echo "usage: $0 <latest.json> <app.tar.gz> <app.tar.gz.sig> <expected-version> <previous-version>" >&2
  exit 2
fi

LATEST_JSON="$1"
UPDATER_ARCHIVE="$2"
UPDATER_SIGNATURE="$3"
EXPECTED_VERSION="${4#v}"
PREVIOUS_VERSION="${5#v}"

for path in "$LATEST_JSON" "$UPDATER_ARCHIVE" "$UPDATER_SIGNATURE"; do
  [[ -f "$path" ]] || {
    echo "macOS updater artifact verification failed: missing $path" >&2
    exit 1
  }
done

MANIFEST_VERSION="$(jq -er '.version' "$LATEST_JSON")"
MANIFEST_URL="$(jq -er '.platforms["darwin-aarch64"].url' "$LATEST_JSON")"
MANIFEST_SIGNATURE="$(jq -er '.platforms["darwin-aarch64"].signature' "$LATEST_JSON")"
FILE_SIGNATURE="$(cat "$UPDATER_SIGNATURE")"

[[ "$MANIFEST_VERSION" == "$EXPECTED_VERSION" ]] || {
  echo "macOS updater artifact verification failed: manifest version '$MANIFEST_VERSION' != '$EXPECTED_VERSION'" >&2
  exit 1
}
[[ "$(basename "$MANIFEST_URL")" == "$(basename "$UPDATER_ARCHIVE")" ]] || {
  echo "macOS updater artifact verification failed: manifest URL does not select the downloaded archive" >&2
  exit 1
}
[[ -n "$MANIFEST_SIGNATURE" && "$MANIFEST_SIGNATURE" == "$FILE_SIGNATURE" ]] || {
  echo "macOS updater artifact verification failed: manifest signature does not match the published .sig" >&2
  exit 1
}

python3 - "$PREVIOUS_VERSION" "$EXPECTED_VERSION" <<'PY'
import sys

def semver(value: str) -> tuple[int, int, int]:
    parts = value.split(".")
    if len(parts) != 3 or any(not part.isdigit() for part in parts):
        raise SystemExit(f"invalid release version: {value}")
    return tuple(map(int, parts))

previous = semver(sys.argv[1])
current = semver(sys.argv[2])
if current <= previous:
    raise SystemExit(
        f"published updater is not newer than the previous release: {current} <= {previous}"
    )
PY

EXTRACT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codefactory-updater.XXXXXX")"
cleanup() {
  local status=$?
  trap - EXIT
  rm -rf "$EXTRACT_DIR" || status=1
  exit "$status"
}
trap cleanup EXIT

tar -xzf "$UPDATER_ARCHIVE" -C "$EXTRACT_DIR"
APP_PATH="$(find "$EXTRACT_DIR" -maxdepth 2 -type d -name 'CodeFactory.app' -print -quit)"
[[ -n "$APP_PATH" ]] || {
  echo "macOS updater artifact verification failed: CodeFactory.app missing from updater archive" >&2
  exit 1
}

ACTUAL_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP_PATH/Contents/Info.plist")"
[[ "$ACTUAL_VERSION" == "$EXPECTED_VERSION" ]] || {
  echo "macOS updater artifact verification failed: archive version '$ACTUAL_VERSION' != '$EXPECTED_VERSION'" >&2
  exit 1
}

codesign --verify --deep --strict --verbose=4 "$APP_PATH"
SIGNATURE_INFO="$(codesign --display --verbose=4 "$APP_PATH" 2>&1)"
grep -q 'Authority=Developer ID Application:' <<<"$SIGNATURE_INFO" || {
  echo "macOS updater artifact verification failed: updater app lacks Developer ID Application authority" >&2
  exit 1
}
grep -Eq 'flags=.*runtime' <<<"$SIGNATURE_INFO" || {
  echo "macOS updater artifact verification failed: updater app lacks hardened runtime" >&2
  exit 1
}
grep -q '^Timestamp=' <<<"$SIGNATURE_INFO" || {
  echo "macOS updater artifact verification failed: updater app lacks a secure signing timestamp" >&2
  exit 1
}
grep -Eq '^TeamIdentifier=.+$' <<<"$SIGNATURE_INFO" || {
  echo "macOS updater artifact verification failed: updater app lacks a signing team identifier" >&2
  exit 1
}
xcrun stapler validate "$APP_PATH"
spctl --assess --type execute --verbose=4 "$APP_PATH"

echo "macOS updater artifact verification passed: previous_version=$PREVIOUS_VERSION version=$ACTUAL_VERSION"
