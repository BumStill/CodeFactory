#!/usr/bin/env bash
#
# install-dev-app-wrapper.sh — register the dev binary with macOS
# LaunchServices so it shows up to `computer-use.request_access` (and
# anywhere else that queries installed apps), enabling live UX
# verification of the running `target/debug/codefactory` from agent
# tooling.
#
# What this script does:
#   1. Build a minimal CodeFactoryDev.app bundle under /Applications/
#      whose CFBundleIdentifier is `com.codefactory.dev` (distinct from
#      the production `com.codefactory.app` so the two can coexist).
#   2. The bundle's executable is a thin shim that resolves a checkout
#      AT LAUNCH TIME and runs that checkout's `pnpm tauri dev`.
#   3. Register the bundle with LaunchServices (`lsregister -f`).
#
# Idempotent. Re-running upgrades the existing wrapper.
#
# Why a wrapper instead of installing the release .app?
#   - release .app reads from production data dir; dev .app reads from
#     dev data dir, keeping the two cleanly separated.
#   - Running it always picks up the LATEST source — no rebuild needed
#     for frontend tweaks.
#   - The release pipeline still produces the real signed app for
#     end users; this is strictly for agent + developer live-verify.
#
# ── Which checkout does it launch? ───────────────────────────────────
# The shim resolves its target every time it starts, taking the first
# candidate that actually looks like a CodeFactory checkout (i.e. has
# `src-tauri/tauri.conf.json`):
#
#   1. $CODEFACTORY_DEV_TARGET          (only when exec'd directly)
#   2. the pointer file, default `~/.codefactory/dev-app-target`
#   3. the checkout this bundle was installed from (baked fallback)
#
# Retarget without reinstalling:
#   scripts/install-dev-app-wrapper.sh --target /path/to/checkout
#   scripts/install-dev-app-wrapper.sh --target        # = $PWD
#   scripts/install-dev-app-wrapper.sh --clear-target  # back to (3)
#   scripts/install-dev-app-wrapper.sh --show          # what resolves now
#
# WHY A POINTER FILE (and not the alternatives):
#   - Env var alone does not work for the case that matters. The agent
#     path is `open -a CodeFactoryDev` / LaunchServices, which launches
#     the bundle from launchd's environment — the caller's exported
#     variables are simply not there. An env-var-only design would fail
#     precisely on the primary user path it exists to serve, so
#     $CODEFACTORY_DEV_TARGET is kept only as a direct-exec override.
#   - Per-checkout bundles (CodeFactoryDev-<slice>.app) do work, and the
#     CODEFACTORY_DEV_* overrides below still allow them for genuinely
#     concurrent live-verification. But as the default they cost a
#     LaunchServices registration, a Tauri identifier and a data dir per
#     worktree, they litter /Applications with bundles that outlive the
#     worktree, and every agent would have to discover the right bundle
#     name before it can call request_access.
#   - The pointer file is read at launch regardless of HOW the app was
#     started (Finder, Spotlight, open(1), LaunchServices), keeps ONE
#     stable bundle name for request_access, and is a single line a
#     worktree can rewrite and then drop.
#   The cost we accept: it is one global "current target" per bundle, so
#   two concurrent live-verifications on the same bundle collide — that
#   is what the CODEFACTORY_DEV_* overrides are for. It is also mutable
#   state outside the repo, so a stale pointer could silently launch the
#   wrong checkout; the shim defends against the common case by falling
#   back to the baked install path (and logging it) as soon as the
#   pointer names a path that is gone or is not a checkout, which is
#   exactly what happens when a worktree is closed out.
#
# ── Window placement ─────────────────────────────────────────────────
# The shim also asks Tauri to open the main window at a fixed origin on
# the primary display (default 60,60). Screen capture of a window on a
# secondary display has been observed to fail (SCContentFilter returns
# nil), which blocks agent screenshots; a deterministic primary-display
# origin avoids that and makes screenshots reproducible. Override with
# CODEFACTORY_DEV_WINDOW_ORIGIN="x,y", or "off" to let Tauri place it.

set -euo pipefail

ORIGINAL_PWD="$PWD"
cd "$(dirname "$0")/.."
PROJECT_ROOT="$(pwd)"

# Optional overrides let concurrent tasks install isolated wrappers without
# replacing the shared CodeFactoryDev registration or data directory.
APP_PATH="${CODEFACTORY_DEV_APP_PATH:-/Applications/CodeFactoryDev.app}"
BUNDLE_ID="${CODEFACTORY_DEV_BUNDLE_ID:-com.codefactory.dev}"
DISPLAY_NAME="${CODEFACTORY_DEV_DISPLAY_NAME:-CodeFactoryDev}"
TAURI_CONFIG="${CODEFACTORY_DEV_TAURI_CONFIG:-src-tauri/tauri.dev.conf.json}"
DEV_HOME="${CODEFACTORY_DEV_HOME:-}"
LOG_PREFIX="${CODEFACTORY_DEV_LOG_PREFIX:-/tmp/CodeFactoryDev}"
WINDOW_ORIGIN="${CODEFACTORY_DEV_WINDOW_ORIGIN:-60,60}"

# The pointer file lives next to the bundle's identity, not next to the
# checkout, so a worktree can be deleted without orphaning it. Isolated
# bundles get their own pointer so they never fight over one target.
POINTER_HOME="${DEV_HOME:-$HOME}"
if [ -n "${CODEFACTORY_DEV_TARGET_FILE:-}" ]; then
    POINTER_FILE="$CODEFACTORY_DEV_TARGET_FILE"
elif [ "$BUNDLE_ID" = "com.codefactory.dev" ]; then
    POINTER_FILE="$POINTER_HOME/.codefactory/dev-app-target"
else
    POINTER_FILE="$POINTER_HOME/.codefactory/dev-app-target.$BUNDLE_ID"
fi

usage() {
    cat <<USAGE
Usage: scripts/install-dev-app-wrapper.sh [options]

  (no options)          install/upgrade the wrapper bundle and point it
                        at this checkout
  --target [PATH]       point the EXISTING wrapper at PATH (default: the
                        current directory) without touching the bundle
  --clear-target        drop the pointer; fall back to the checkout the
                        bundle was installed from
  --show                print the pointer file and what resolves today
  -h, --help            this text

Environment overrides (for isolated, concurrent wrappers):
  CODEFACTORY_DEV_APP_PATH, CODEFACTORY_DEV_BUNDLE_ID,
  CODEFACTORY_DEV_DISPLAY_NAME, CODEFACTORY_DEV_TAURI_CONFIG,
  CODEFACTORY_DEV_HOME, CODEFACTORY_DEV_LOG_PREFIX,
  CODEFACTORY_DEV_TARGET_FILE, CODEFACTORY_DEV_WINDOW_ORIGIN
USAGE
}

looks_like_checkout() {
    [ -n "${1:-}" ] && [ -f "$1/src-tauri/tauri.conf.json" ]
}

abs_path() {
    (cd "$ORIGINAL_PWD" && cd "$1" >/dev/null 2>&1 && pwd)
}

write_pointer() {
    mkdir -p "$(dirname "$POINTER_FILE")"
    cat > "$POINTER_FILE" <<POINTER
# CodeFactoryDev launch target — read every time the wrapper starts.
# One absolute path to a CodeFactory checkout. Blank lines and #-comments
# are ignored. Delete this file (or run --clear-target) to fall back to
# the checkout the bundle was installed from.
$1
POINTER
}

read_pointer() {
    [ -f "$POINTER_FILE" ] || return 0
    local line
    while IFS= read -r line || [ -n "$line" ]; do
        line="${line%%#*}"
        line="$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
        [ -n "$line" ] || continue
        case "$line" in "~/"*) line="$POINTER_HOME/${line#\~/}" ;; esac
        printf '%s\n' "$line"
        return 0
    done < "$POINTER_FILE"
}

baked_fallback() {
    local shim="$APP_PATH/Contents/MacOS/$DISPLAY_NAME"
    [ -f "$shim" ] || return 0
    sed -n 's/^CF_FALLBACK_ROOT=//p' "$shim" | head -1 | sed -e "s/^'//" -e "s/'$//"
}

# A bundle installed before launch-time targeting existed ignores the
# pointer file entirely. Writing one anyway would look like a successful
# retarget while the app keeps launching the old checkout — i.e. evidence
# collected against the wrong code. Detect it instead.
shim_reads_pointer() {
    local shim="$APP_PATH/Contents/MacOS/$DISPLAY_NAME"
    [ -f "$shim" ] && grep -q '^CF_POINTER_FILE=' "$shim"
}

show_state() {
    local pointer_value fallback effective source
    pointer_value="$(read_pointer)"
    fallback="$(baked_fallback)"
    echo "bundle:        $APP_PATH ($BUNDLE_ID)"
    if [ -d "$APP_PATH" ]; then
        echo "installed:     yes"
    else
        echo "installed:     NO — run this script with no options first"
    fi
    if [ -d "$APP_PATH" ] && ! shim_reads_pointer; then
        echo "⚠️  this bundle predates launch-time targeting and IGNORES the"
        echo "    pointer file — reinstall it before trusting --target."
    fi
    echo "pointer file:  $POINTER_FILE"
    if [ -n "$pointer_value" ]; then
        if looks_like_checkout "$pointer_value"; then
            echo "pointer value: $pointer_value (valid)"
        else
            echo "pointer value: $pointer_value (STALE — not a checkout, will be skipped)"
        fi
    else
        echo "pointer value: (none)"
    fi
    echo "install root:  ${fallback:-(unknown)}"

    effective=""
    source=""
    if looks_like_checkout "${CODEFACTORY_DEV_TARGET:-}"; then
        effective="$CODEFACTORY_DEV_TARGET"; source="CODEFACTORY_DEV_TARGET (direct exec only)"
    elif looks_like_checkout "$pointer_value"; then
        effective="$pointer_value"; source="pointer file"
    elif looks_like_checkout "$fallback"; then
        effective="$fallback"; source="install root"
    fi
    if [ -n "$effective" ]; then
        echo "→ launches:    $effective  [via $source]"
    else
        echo "→ launches:    NOTHING — no candidate is a valid checkout"
    fi
}

# ── Argument parsing ─────────────────────────────────────────────────
MODE="install"
TARGET_ARG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --target)
            MODE="retarget"
            if [ $# -ge 2 ] && [ "${2#-}" = "$2" ]; then
                TARGET_ARG="$2"; shift 2
            else
                TARGET_ARG="$ORIGINAL_PWD"; shift
            fi
            ;;
        --target=*) MODE="retarget"; TARGET_ARG="${1#*=}"; shift ;;
        --clear-target) MODE="clear"; shift ;;
        --show) MODE="show"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Error: unknown option '$1'" >&2; echo >&2; usage >&2; exit 2 ;;
    esac
done

case "$MODE" in
    show)
        show_state
        exit 0
        ;;
    clear)
        if [ -f "$POINTER_FILE" ]; then
            rm -f "$POINTER_FILE"
            echo "Cleared $POINTER_FILE"
        else
            echo "No pointer file at $POINTER_FILE — nothing to clear."
        fi
        echo ""
        show_state
        exit 0
        ;;
    retarget)
        RESOLVED="$(abs_path "$TARGET_ARG" || true)"
        if [ -z "$RESOLVED" ]; then
            echo "Error: '$TARGET_ARG' does not exist." >&2
            exit 1
        fi
        if ! looks_like_checkout "$RESOLVED"; then
            echo "Error: '$RESOLVED' is not a CodeFactory checkout" >&2
            echo "       (no src-tauri/tauri.conf.json)." >&2
            exit 1
        fi
        if [ ! -d "$APP_PATH" ]; then
            echo "Error: $APP_PATH is not installed yet." >&2
            echo "       Run this script with no options first." >&2
            exit 1
        fi
        if ! shim_reads_pointer; then
            echo "Error: $APP_PATH predates launch-time targeting and would" >&2
            echo "       ignore the pointer file — it keeps launching the" >&2
            echo "       checkout it was installed from, so any evidence you" >&2
            echo "       collect would come from the wrong code." >&2
            echo "       Reinstall once from the long-lived main checkout:" >&2
            echo "         (cd /path/to/main/checkout && $0)" >&2
            exit 1
        fi
        write_pointer "$RESOLVED"
        echo "Retargeted $DISPLAY_NAME → $RESOLVED"
        echo ""
        show_state
        exit 0
        ;;
esac

# ── Install ──────────────────────────────────────────────────────────
echo "Installing $DISPLAY_NAME wrapper → $APP_PATH"
echo "  project root: $PROJECT_ROOT"

case "$PROJECT_ROOT" in
    */.claude/worktrees/*)
        echo ""
        echo "⚠️  Installing from a git worktree. The baked fallback will point"
        echo "    at a directory that disappears when the worktree is closed out."
        echo "    Prefer installing once from the main checkout and then running"
        echo "    '$0 --target' from the worktree."
        echo ""
        ;;
esac

# ── Build the bundle skeleton ────────────────────────────────────────
rm -rf "$APP_PATH"
mkdir -p "$APP_PATH/Contents/MacOS"
mkdir -p "$APP_PATH/Contents/Resources"

# Info.plist — minimum keys to make LaunchServices treat this as a
# proper app and resolve "CodeFactoryDev" in request_access.
cat > "$APP_PATH/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$DISPLAY_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$DISPLAY_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$DISPLAY_NAME</string>
    <key>CFBundleVersion</key>
    <string>0.0.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.0.0-dev</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Shim executable. The wrapper IS launched once at user "open"; the
# shim then exec's into pnpm tauri dev so the webview window appears
# with this bundle's identifier in the Dock and LaunchServices index.
#
# Toolchain locations (pnpm, cargo) are resolved at install time so the
# wrapper survives PATH and shell differences when launched from Finder,
# Spotlight, or open(1). The CHECKOUT is deliberately not baked in — see
# the header comment; only the fallback is.
PNPM_PATH="$(which pnpm 2>/dev/null || true)"
if [ -z "$PNPM_PATH" ]; then
    echo "Error: pnpm not found on PATH. Install pnpm first." >&2
    exit 1
fi
NODE_PATH_BIN="$(which node 2>/dev/null || true)"
HOST_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
HOST_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
CARGO_BIN="$HOST_CARGO_HOME/bin"

SHIM="$APP_PATH/Contents/MacOS/$DISPLAY_NAME"
{
    printf '#!/usr/bin/env bash\n'
    printf '# %s wrapper — launches the dev tauri build of CodeFactory.\n' "$DISPLAY_NAME"
    printf '# Generated by scripts/install-dev-app-wrapper.sh — re-run to update.\n'
    printf '# The checkout is resolved at LAUNCH time, not baked in:\n'
    printf '#   CODEFACTORY_DEV_TARGET -> pointer file -> install root.\n'
    printf 'set -uo pipefail\n\n'

    # Install-time constants. printf %q keeps paths with spaces safe.
    printf 'CF_DISPLAY_NAME=%q\n' "$DISPLAY_NAME"
    printf 'CF_FALLBACK_ROOT=%q\n' "$PROJECT_ROOT"
    printf 'CF_POINTER_FILE=%q\n' "$POINTER_FILE"
    printf 'CF_TAURI_CONFIG=%q\n' "$TAURI_CONFIG"
    printf 'CF_LOG_PREFIX=%q\n' "$LOG_PREFIX"
    printf 'CF_PNPM_PATH=%q\n' "$PNPM_PATH"
    printf 'CF_PNPM_DIR=%q\n' "$(dirname "$PNPM_PATH")"
    printf 'CF_NODE_BIN=%q\n' "$NODE_PATH_BIN"
    printf 'CF_CARGO_HOME=%q\n' "$HOST_CARGO_HOME"
    printf 'CF_RUSTUP_HOME=%q\n' "$HOST_RUSTUP_HOME"
    printf 'CF_CARGO_BIN=%q\n' "$CARGO_BIN"
    printf 'CF_DEV_HOME=%q\n' "$DEV_HOME"
    printf 'CF_POINTER_HOME=%q\n' "$POINTER_HOME"
    printf 'CF_WINDOW_ORIGIN=%q\n' "$WINDOW_ORIGIN"
    printf '\n'

    cat <<'SHIM_BODY'
export PATH="$CF_CARGO_BIN:$CF_PNPM_DIR:/usr/local/bin:/opt/homebrew/bin:$PATH"
export CARGO_HOME="$CF_CARGO_HOME"
export RUSTUP_HOME="$CF_RUSTUP_HOME"
if [ -n "$CF_DEV_HOME" ]; then
  export HOME="$CF_DEV_HOME"
fi

# Log stdout/stderr — including the target resolution below — so dev-app
# failures and "it launched the wrong checkout" never disappear silently.
exec >> "$CF_LOG_PREFIX-$(date +%Y%m%d).log" 2>&1
echo "=== $(date '+%Y-%m-%d %H:%M:%S') $CF_DISPLAY_NAME launching ==="

looks_like_checkout() {
  [ -n "${1:-}" ] && [ -f "$1/src-tauri/tauri.conf.json" ]
}

# First non-empty, non-comment line of the pointer file.
read_pointer() {
  [ -f "$CF_POINTER_FILE" ] || return 0
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%%#*}"
    line="$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    [ -n "$line" ] || continue
    case "$line" in "~/"*) line="$CF_POINTER_HOME/${line#\~/}" ;; esac
    printf '%s\n' "$line"
    return 0
  done < "$CF_POINTER_FILE"
}

# Resolve the checkout: env override, then pointer file, then the
# checkout this bundle was installed from. A candidate that is gone or
# is not a checkout (e.g. a closed-out worktree) is skipped, loudly.
CF_SOURCES=("CODEFACTORY_DEV_TARGET" "pointer file $CF_POINTER_FILE" "install root")
CF_CANDIDATES=("${CODEFACTORY_DEV_TARGET:-}" "$(read_pointer)" "$CF_FALLBACK_ROOT")
CF_TARGET=""
CF_TARGET_SOURCE=""
cf_i=0
while [ "$cf_i" -lt "${#CF_CANDIDATES[@]}" ]; do
  cf_candidate="${CF_CANDIDATES[$cf_i]}"
  if [ -n "$cf_candidate" ]; then
    if looks_like_checkout "$cf_candidate"; then
      CF_TARGET="$cf_candidate"
      CF_TARGET_SOURCE="${CF_SOURCES[$cf_i]}"
      break
    fi
    echo "warn: ignoring ${CF_SOURCES[$cf_i]} -> '$cf_candidate' (no src-tauri/tauri.conf.json)"
  fi
  cf_i=$((cf_i + 1))
done

if [ -z "$CF_TARGET" ]; then
  echo "fatal: no valid CodeFactory checkout to launch."
  echo "       tried: CODEFACTORY_DEV_TARGET, $CF_POINTER_FILE, $CF_FALLBACK_ROOT"
  echo "       fix with: scripts/install-dev-app-wrapper.sh --target /path/to/checkout"
  exit 1
fi

cd "$CF_TARGET"
echo "target checkout: $CF_TARGET  [via $CF_TARGET_SOURCE]"
echo "commit:          $(git -C "$CF_TARGET" rev-parse --short HEAD 2>/dev/null || echo unknown) \
$(git -C "$CF_TARGET" rev-parse --abbrev-ref HEAD 2>/dev/null || true)"

# Pin the main window to a known origin on the primary display. Capturing
# a window on a secondary display has been seen to fail outright
# (SCContentFilter returns nil), which blocks agent screenshots. Tauri's
# window config is an array, and config merging replaces arrays wholesale,
# so the patch is built from the target checkout's own window definition
# at launch time rather than duplicated into a checked-in config file.
CF_WINDOW_PATCH_JS='
const fs = require("fs");
const read = (p) => JSON.parse(fs.readFileSync(p, "utf8"));
const base = read(process.env.CF_BASE_CONF);
let windows = [];
try {
  const dev = read(process.env.CF_DEV_CONF);
  windows = (dev.app && dev.app.windows) || [];
} catch (err) { windows = []; }
if (!windows.length) windows = (base.app && base.app.windows) || [];
if (!windows.length) process.exit(3);
const x = Number(process.env.CF_WX);
const y = Number(process.env.CF_WY);
if (!Number.isFinite(x) || !Number.isFinite(y)) process.exit(4);
const patched = windows.map((w) => Object.assign({}, w, { x, y }));
process.stdout.write(JSON.stringify({ app: { windows: patched } }));
'

window_patch() {
  [ "$CF_WINDOW_ORIGIN" != "off" ] || return 1
  case "$CF_WINDOW_ORIGIN" in *,*) : ;; *) return 1 ;; esac
  local node_bin dev_conf
  node_bin="$CF_NODE_BIN"
  [ -x "$node_bin" ] || node_bin="$(command -v node 2>/dev/null || true)"
  [ -n "$node_bin" ] || return 1
  case "$CF_TAURI_CONFIG" in
    /*) dev_conf="$CF_TAURI_CONFIG" ;;
    *)  dev_conf="$CF_TARGET/$CF_TAURI_CONFIG" ;;
  esac
  CF_BASE_CONF="$CF_TARGET/src-tauri/tauri.conf.json" \
  CF_DEV_CONF="$dev_conf" \
  CF_WX="${CF_WINDOW_ORIGIN%%,*}" \
  CF_WY="${CF_WINDOW_ORIGIN##*,}" \
    "$node_bin" -e "$CF_WINDOW_PATCH_JS"
}

CF_ARGS=(tauri dev --config "$CF_TAURI_CONFIG")
CF_PATCH="$(window_patch || true)"
if [ -n "$CF_PATCH" ]; then
  CF_ARGS+=(--config "$CF_PATCH")
  echo "window origin:   $CF_WINDOW_ORIGIN (primary display)"
else
  echo "window origin:   default (Tauri decides)"
fi

# Keep the interactive desktop session awake for the lifetime of the dev App.
# This prevents an unattended live-verification run from being interrupted by
# idle display/system sleep. A user-initiated lock is still respected.
exec /usr/bin/caffeinate -dimsu "$CF_PNPM_PATH" "${CF_ARGS[@]}"
SHIM_BODY
} > "$SHIM"

chmod +x "$SHIM"

# Point the freshly installed bundle at the checkout it came from, so a
# plain install behaves exactly like it always has.
write_pointer "$PROJECT_ROOT"

# ── Ad-hoc sign + clear quarantine ────────────────────────────────────
# Gatekeeper refuses to open unsigned bundles with -10669 even on the
# user's own machine. An ad-hoc signature (-s -) is enough to clear
# that gate for a local app the user clearly intends to run, and
# xattr removes the quarantine flag that gets set on freshly written
# bundles in some macOS configs.
codesign --force --deep --sign - "$APP_PATH" >/dev/null 2>&1 || true
xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

# ── Register with LaunchServices ─────────────────────────────────────
# -f forces a re-register (idempotent on repeat install).
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "$APP_PATH"

echo ""
echo "✅ Installed."
echo ""
show_state
echo ""
echo "Verify (should print the bundle path):"
echo "  mdfind 'kMDItemCFBundleIdentifier == \"$BUNDLE_ID\"'"
echo ""
echo "Agents can now request access via either name:"
echo "  - \"$DISPLAY_NAME\""
echo "  - \"$BUNDLE_ID\""
echo ""
echo "Launch (Finder, Spotlight, or):"
echo "  open -a $DISPLAY_NAME"
echo ""
echo "Point it at another checkout without reinstalling:"
echo "  scripts/install-dev-app-wrapper.sh --target /path/to/checkout"
echo ""
echo "Logs land at $LOG_PREFIX-<YYYYMMDD>.log"
