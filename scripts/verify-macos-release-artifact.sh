#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS release artifact smoke requires macOS" >&2
  exit 2
fi

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <CodeFactory.dmg> <expected-version>" >&2
  exit 2
fi

DMG_PATH="$1"
EXPECTED_VERSION="${2#v}"
if [[ ! -f "$DMG_PATH" ]]; then
  echo "macOS release artifact smoke failed: DMG not found: $DMG_PATH" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MOUNT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codefactory-dmg.XXXXXX")"
INSTALL_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codefactory-install.XXXXXX")"
MOUNTED=0

cleanup() {
  local status=$?
  trap - EXIT
  if [[ "$MOUNTED" -eq 1 ]]; then
    if ! hdiutil detach "$MOUNT_DIR" -quiet; then
      echo "macOS release artifact smoke failed: could not detach $MOUNT_DIR" >&2
      status=1
    fi
  fi
  if ! rm -rf "$MOUNT_DIR" "$INSTALL_DIR"; then
    echo "macOS release artifact smoke failed: could not remove temporary directories" >&2
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT

hdiutil attach "$DMG_PATH" -nobrowse -readonly -mountpoint "$MOUNT_DIR" -quiet
MOUNTED=1

SOURCE_APP="$MOUNT_DIR/CodeFactory.app"
INSTALLED_APP="$INSTALL_DIR/CodeFactory.app"
if [[ ! -d "$SOURCE_APP" ]]; then
  echo "macOS release artifact smoke failed: CodeFactory.app missing from DMG root" >&2
  exit 1
fi

ditto "$SOURCE_APP" "$INSTALLED_APP"

if ! hdiutil detach "$MOUNT_DIR" -quiet; then
  echo "macOS release artifact smoke failed: could not detach $MOUNT_DIR after copying the app" >&2
  exit 1
fi
MOUNTED=0

INFO_PLIST="$INSTALLED_APP/Contents/Info.plist"
BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST")"
ACTUAL_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO_PLIST")"
EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$INFO_PLIST")"
EXECUTABLE_PATH="$INSTALLED_APP/Contents/MacOS/$EXECUTABLE_NAME"

if [[ "$BUNDLE_ID" != "com.codefactory.app" ]]; then
  echo "macOS release artifact smoke failed: bundle id is '$BUNDLE_ID', expected 'com.codefactory.app'" >&2
  exit 1
fi
if [[ "$ACTUAL_VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "macOS release artifact smoke failed: version is '$ACTUAL_VERSION', expected '$EXPECTED_VERSION'" >&2
  exit 1
fi
if [[ ! -x "$EXECUTABLE_PATH" ]]; then
  echo "macOS release artifact smoke failed: executable missing: $EXECUTABLE_PATH" >&2
  exit 1
fi
ARCHS="$(lipo -archs "$EXECUTABLE_PATH")"
if [[ " $ARCHS " != *" arm64 "* ]]; then
  echo "macOS release artifact smoke failed: executable architectures are '$ARCHS', expected arm64" >&2
  exit 1
fi

WINDOW_ARGS=(
  "$SCRIPT_DIR/verify-macos-app-window.swift"
  "$INSTALLED_APP"
  "${CODEFACTORY_RELEASE_WINDOW_TIMEOUT_SEC:-30}"
)
if [[ -n "${CODEFACTORY_RELEASE_EVIDENCE_DIR:-}" ]]; then
  WINDOW_ARGS+=("$CODEFACTORY_RELEASE_EVIDENCE_DIR")
fi
swift "${WINDOW_ARGS[@]}"

ISOLATED_DB="$INSTALL_DIR/smoke-home/Library/Application Support/com.codefactory.app/codefactory.db"
if [[ ! -f "$ISOLATED_DB" ]]; then
  echo "macOS release artifact smoke failed: app did not initialize its database under isolated HOME" >&2
  exit 1
fi

echo "macOS release artifact smoke passed: bundle_id=$BUNDLE_ID version=$ACTUAL_VERSION"
