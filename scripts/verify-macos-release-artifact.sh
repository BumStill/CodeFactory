#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS release artifact smoke requires macOS" >&2
  exit 2
fi

if [[ $# -lt 2 || $# -gt 5 ]]; then
  echo "usage: $0 <CodeFactory.dmg> <expected-version> [expected-build-sha] [CodeFactory.app.tar.gz] [latest.json]" >&2
  exit 2
fi

DMG_PATH="$1"
EXPECTED_VERSION="${2#v}"
EXPECTED_BUILD_SHA="${3:-}"
UPDATER_ARCHIVE="${4:-}"
RELEASE_MANIFEST="${5:-}"
if [[ ! -f "$DMG_PATH" ]]; then
  echo "macOS release artifact smoke failed: DMG not found: $DMG_PATH" >&2
  exit 1
fi
if [[ -n "$EXPECTED_BUILD_SHA" && ! "$EXPECTED_BUILD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "macOS release artifact smoke failed: expected build SHA must be a full lowercase Git SHA" >&2
  exit 1
fi
if [[ -n "$UPDATER_ARCHIVE" && ! -f "$UPDATER_ARCHIVE" ]]; then
  echo "macOS release artifact smoke failed: updater archive not found: $UPDATER_ARCHIVE" >&2
  exit 1
fi
if [[ -n "$RELEASE_MANIFEST" && ! -f "$RELEASE_MANIFEST" ]]; then
  echo "macOS release artifact smoke failed: latest.json not found: $RELEASE_MANIFEST" >&2
  exit 1
fi
if [[ -n "$RELEASE_MANIFEST" && ( -z "$EXPECTED_BUILD_SHA" || -z "$UPDATER_ARCHIVE" ) ]]; then
  echo "macOS release artifact smoke failed: latest.json verification requires expected build SHA and updater archive" >&2
  exit 1
fi

MANIFEST_BUILD_SHA=""
MANIFEST_VERSION=""
if [[ -n "$RELEASE_MANIFEST" ]]; then
  if ! MANIFEST_BUILD_SHA="$(/usr/bin/plutil -extract build_git_sha raw "$RELEASE_MANIFEST" 2>/dev/null)"; then
    echo "macOS release artifact smoke failed: latest.json build_git_sha is missing or invalid" >&2
    exit 1
  fi
  if ! MANIFEST_VERSION="$(/usr/bin/plutil -extract version raw "$RELEASE_MANIFEST" 2>/dev/null)"; then
    echo "macOS release artifact smoke failed: latest.json version is missing or invalid" >&2
    exit 1
  fi
  if [[ ! "$MANIFEST_BUILD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    echo "macOS release artifact smoke failed: latest.json build_git_sha is not a full lowercase Git SHA" >&2
    exit 1
  fi
  if [[ "$MANIFEST_BUILD_SHA" != "$EXPECTED_BUILD_SHA" ]]; then
    echo "macOS release artifact smoke failed: latest.json build_git_sha is '$MANIFEST_BUILD_SHA', expected '$EXPECTED_BUILD_SHA'" >&2
    exit 1
  fi
  if [[ "$MANIFEST_VERSION" != "$EXPECTED_VERSION" ]]; then
    echo "macOS release artifact smoke failed: latest.json version is '$MANIFEST_VERSION', expected '$EXPECTED_VERSION'" >&2
    exit 1
  fi
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MOUNT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codefactory-dmg.XXXXXX")"
INSTALL_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codefactory-install.XXXXXX")"
UPDATER_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codefactory-updater.XXXXXX")"
MOUNTED=0
RTE003_POLICY_INSTALLED=0
RTE003_POLICY_FILES=(
  "/Library/Managed Preferences/com.google.Chrome.plist"
  "/Library/Managed Preferences/com.google.chrome.for.testing.plist"
)

cleanup() {
  local status=$?
  trap - EXIT
  if [[ "$MOUNTED" -eq 1 ]]; then
    if ! hdiutil detach "$MOUNT_DIR" -quiet; then
      echo "macOS release artifact smoke failed: could not detach $MOUNT_DIR" >&2
      status=1
    fi
  fi
  if [[ "$RTE003_POLICY_INSTALLED" -eq 1 ]]; then
    for policy_file in "${RTE003_POLICY_FILES[@]}"; do
      if ! sudo -n /bin/rm -f "$policy_file"; then
        echo "macOS release artifact smoke failed: could not remove temporary Chrome policy $policy_file" >&2
        status=1
      fi
    done
  fi
  if ! rm -rf "$MOUNT_DIR" "$INSTALL_DIR" "$UPDATER_DIR"; then
    echo "macOS release artifact smoke failed: could not remove temporary directories" >&2
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT

BUNDLE_ID=""
ACTUAL_VERSION=""
EXECUTABLE_PATH=""
UPDATER_EXECUTABLE_PATH=""

verify_app_bundle() {
  local app_path="$1"
  local source_label="$2"
  local info_plist bundle_id actual_version executable_name executable_path archs

  if [[ ! -d "$app_path" ]]; then
    echo "macOS release artifact smoke failed: CodeFactory.app missing from $source_label" >&2
    return 1
  fi

  info_plist="$app_path/Contents/Info.plist"
  bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$info_plist")"
  actual_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$info_plist")"
  executable_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$info_plist")"
  executable_path="$app_path/Contents/MacOS/$executable_name"

  if [[ "$bundle_id" != "com.codefactory.app" ]]; then
    echo "macOS release artifact smoke failed: $source_label bundle id is '$bundle_id', expected 'com.codefactory.app'" >&2
    return 1
  fi
  if [[ "$actual_version" != "$EXPECTED_VERSION" ]]; then
    echo "macOS release artifact smoke failed: $source_label version is '$actual_version', expected '$EXPECTED_VERSION'" >&2
    return 1
  fi
  if [[ ! -x "$executable_path" ]]; then
    echo "macOS release artifact smoke failed: $source_label executable missing: $executable_path" >&2
    return 1
  fi
  archs="$(lipo -archs "$executable_path")"
  if [[ " $archs " != *" arm64 "* ]]; then
    echo "macOS release artifact smoke failed: $source_label executable architectures are '$archs', expected arm64" >&2
    return 1
  fi

  # A linker-generated Mach-O signature is not a valid app-bundle signature:
  # it leaves Info.plist unbound and has no sealed resource envelope. The
  # compatibility channel uses Tauri's complete ad-hoc bundle signing, which
  # needs no Apple credentials but must still pass strict on-disk validation.
  if ! /usr/bin/codesign --verify --deep --strict --verbose=4 "$app_path"; then
    echo "macOS release artifact smoke failed: $source_label app-bundle signature is invalid" >&2
    return 1
  fi
  /usr/bin/codesign --display --verbose=4 "$app_path" 2>&1

  if [[ "$source_label" == "DMG" ]]; then
    BUNDLE_ID="$bundle_id"
    ACTUAL_VERSION="$actual_version"
    EXECUTABLE_PATH="$executable_path"
  elif [[ "$source_label" == "updater" ]]; then
    UPDATER_EXECUTABLE_PATH="$executable_path"
  fi
}

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

if [[ -n "$UPDATER_ARCHIVE" ]]; then
  if ! tar -tzf "$UPDATER_ARCHIVE" | /usr/bin/awk -F/ '
    /^\// { exit 1 }
    {
      for (component = 1; component <= NF; component++) {
        if ($component == "..") exit 1
      }
    }
    END { if (NR == 0) exit 1 }
  '; then
    echo "macOS release artifact smoke failed: updater archive contains an unsafe or empty path set" >&2
    exit 1
  fi
  if ! tar -tvzf "$UPDATER_ARCHIVE" | /usr/bin/awk '
    {
      entry_type = substr($1, 1, 1)
      if (entry_type != "-" && entry_type != "d") exit 1
    }
    END { if (NR == 0) exit 1 }
  '; then
    echo "macOS release artifact smoke failed: updater archive may contain only regular files and directories" >&2
    exit 1
  fi
  tar -xzf "$UPDATER_ARCHIVE" -C "$UPDATER_DIR"
  UPDATER_APP_COUNT="$(find "$UPDATER_DIR" -maxdepth 3 -type d -name 'CodeFactory.app' | wc -l | tr -d ' ')"
  if [[ "$UPDATER_APP_COUNT" != "1" ]]; then
    echo "macOS release artifact smoke failed: expected one updater CodeFactory.app, found $UPDATER_APP_COUNT" >&2
    exit 1
  fi
  UPDATER_APP="$(find "$UPDATER_DIR" -maxdepth 3 -type d -name 'CodeFactory.app' -print -quit)"
  verify_app_bundle "$UPDATER_APP" "updater"
fi

# Both install paths must be fenced before this script executes release code.
verify_app_bundle "$INSTALLED_APP" "DMG"
if [[ -n "$UPDATER_ARCHIVE" ]]; then
  DMG_EXECUTABLE_SHA256="$(/usr/bin/shasum -a 256 "$EXECUTABLE_PATH" | /usr/bin/awk '{print $1}')"
  UPDATER_EXECUTABLE_SHA256="$(/usr/bin/shasum -a 256 "$UPDATER_EXECUTABLE_PATH" | /usr/bin/awk '{print $1}')"
  if [[ "$DMG_EXECUTABLE_SHA256" != "$UPDATER_EXECUTABLE_SHA256" ]]; then
    echo "macOS release artifact smoke failed: DMG and updater contain different executable content" >&2
    exit 1
  fi
  echo "macOS release artifact executable match: sha256=$DMG_EXECUTABLE_SHA256"
fi

# RTE-003 exact-artifact gate. The candidate binary starts its native
# extension bridge, materializes the extension embedded in that binary, and
# attaches to a real synthetic Chrome fixture. Closing the CodeFactory session
# must release its lease without terminating that already-running browser.
# Chrome 142+ requires a prior Local Network Access grant before a service
# worker can dial loopback, but a headless release fixture has no user to answer
# the prompt. Use Chrome's documented managed allowlist only for the isolated
# Chrome for Testing process, then remove it in cleanup.
if ! sudo -n true; then
  echo "macOS release artifact smoke failed: RTE-003 requires passwordless sudo for its temporary Chrome policy" >&2
  exit 1
fi
for policy_file in "${RTE003_POLICY_FILES[@]}"; do
  if sudo -n /usr/bin/test -e "$policy_file"; then
    echo "macOS release artifact smoke failed: refusing to overwrite existing Chrome policy $policy_file" >&2
    exit 1
  fi
done
RTE003_POLICY_INSTALLED=1
for policy_domain in com.google.Chrome com.google.chrome.for.testing; do
  sudo -n /usr/bin/defaults write "/Library/Managed Preferences/$policy_domain" \
    LocalNetworkAccessAllowedForUrls -array "chrome-extension://*"
  sudo -n /usr/bin/defaults write "/Library/Managed Preferences/$policy_domain" \
    LoopbackNetworkAllowedForUrls -array "chrome-extension://*"
  sudo -n /bin/chmod 0644 "/Library/Managed Preferences/$policy_domain.plist"
done
CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT="${CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT:-$INSTALL_DIR/browser-chrome-attach-smoke.json}"
mkdir -p "$(dirname "$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT")" "$INSTALL_DIR/browser-attach-home"
HOME="$INSTALL_DIR/browser-attach-home" \
CODEFACTORY_BROWSER_CHROME_FIXTURE="managed" \
  "$EXECUTABLE_PATH" --browser-chrome-attach-smoke "$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT"
if [[ "$(/usr/bin/plutil -extract status raw "$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT")" != "passed" ]]; then
  echo "macOS release artifact smoke failed: Chrome attachment status was not passed" >&2
  exit 1
fi
if [[ "$(/usr/bin/plutil -extract connection_kind raw "$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT")" != "attached_chrome" ]]; then
  echo "macOS release artifact smoke failed: Chrome fixture did not use the attached browser path" >&2
  exit 1
fi
for field in tab_observation_ok detached_without_managed_close lease_reclaimed_after_detach browser_process_alive_after_detach; do
  if [[ "$(/usr/bin/plutil -extract "$field" raw "$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT")" != "true" ]]; then
    echo "macOS release artifact smoke failed: Chrome attachment receipt field '$field' was not true" >&2
    exit 1
  fi
done
cat "$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT"

EVOLUTION_RECEIPT="$INSTALL_DIR/evolution-release-smoke.json"
"$EXECUTABLE_PATH" --evolution-smoke "$EVOLUTION_RECEIPT"
if [[ "$(/usr/bin/plutil -extract status raw "$EVOLUTION_RECEIPT")" != "pass" ]]; then
  echo "macOS release artifact smoke failed: Evolution smoke status was not pass" >&2
  exit 1
fi
if [[ "$(/usr/bin/plutil -extract failed_eval_blocked_activation raw "$EVOLUTION_RECEIPT")" != "true" ]]; then
  echo "macOS release artifact smoke failed: failed Eval did not block activation" >&2
  exit 1
fi
EVAL_REQUIRED_COUNT="$(/usr/bin/plutil -extract eval_required_count raw "$EVOLUTION_RECEIPT")"
EVAL_PASSED_COUNT="$(/usr/bin/plutil -extract eval_passed_count raw "$EVOLUTION_RECEIPT")"
if [[ "$EVAL_REQUIRED_COUNT" -ne "$EVAL_PASSED_COUNT" ]]; then
  echo "macOS release artifact smoke failed: required Evals did not all pass" >&2
  exit 1
fi
if [[ "$(/usr/bin/plutil -extract restart_reopen_observed raw "$EVOLUTION_RECEIPT")" != "true" ]]; then
  echo "macOS release artifact smoke failed: Evolution state was not verified after reopen" >&2
  exit 1
fi
if [[ "$(/usr/bin/plutil -extract rollback_status raw "$EVOLUTION_RECEIPT")" != "rolled_back" ]]; then
  echo "macOS release artifact smoke failed: activation did not roll back" >&2
  exit 1
fi
if [[ "$(/usr/bin/plutil -extract cleanup raw "$EVOLUTION_RECEIPT")" != "true" ]]; then
  echo "macOS release artifact smoke failed: isolated Evolution state was not cleaned" >&2
  exit 1
fi
if [[ -n "$EXPECTED_BUILD_SHA" ]]; then
  ACTUAL_BUILD_SHA="$(/usr/bin/plutil -extract build_git_sha raw "$EVOLUTION_RECEIPT")"
  if [[ "$ACTUAL_BUILD_SHA" != "$EXPECTED_BUILD_SHA" ]]; then
    echo "macOS release artifact smoke failed: build Git SHA is '$ACTUAL_BUILD_SHA', expected '$EXPECTED_BUILD_SHA'" >&2
    exit 1
  fi
fi
if [[ -n "$RELEASE_MANIFEST" ]]; then
  if [[ "$ACTUAL_BUILD_SHA" != "$MANIFEST_BUILD_SHA" ]]; then
    echo "macOS release artifact smoke failed: binary and latest.json build identities differ" >&2
    exit 1
  fi
  echo "release build identity matched: build_git_sha=$ACTUAL_BUILD_SHA executable_sha256=$DMG_EXECUTABLE_SHA256"
fi
cat "$EVOLUTION_RECEIPT"

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
