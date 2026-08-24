#!/usr/bin/env bash
#
# dev-app-bundle-runner.sh — give the `tauri dev` GUI process a STABLE macOS
# bundle identity, on the first launch and on every hot restart alike.
#
# Wired in as a cargo runner (CARGO_TARGET_<triple>_RUNNER) by
# scripts/install-dev-app-wrapper.sh, so cargo invokes us as
#     dev-app-bundle-runner.sh <binary> [args...]
# every time `tauri dev` (re)starts the app — including after each rebuild.
#
# ── Why this exists ──────────────────────────────────────────────────
# macOS hands a process a CFBundleIdentifier in exactly two ways:
#
#   1. its executable sits inside a `.app` bundle, or
#   2. it claims a pending LaunchServices launch record.
#
# `tauri dev` gets neither reliably. Tauri embeds a __TEXT,__info_plist in
# the dev binary that carries CFBundleName and versions but NO
# CFBundleIdentifier, so `target/debug/codefactory` is anonymous on its own.
# What made it work at all was (2): `open -a CodeFactoryDev` opens a launch
# record for com.codefactory.dev, and the first GUI child that registers
# absorbs it. That record is claimed ONCE. cargo re-runs the binary as a new
# process after every rebuild, and process #2 registers with a NULL bundle
# id.
#
# The visible damage is badly misleading: computer-use screenshots keep
# working (the window is real and on screen) while the frontmost-app gate
# rejects every click with "the click would land on the desktop shell",
# so it reads as a coordinate bug. That silently breaks the AGENTS.md rule
# that UX changes must be verified live, because the normal edit-and-recheck
# loop destroys automation access on the very first rebuild.
#
# So we take route (1), which involves no race and no LaunchServices state:
# hardlink the freshly built binary into `<target>/CodeFactoryDev.app` and
# exec it from there. Running a bundled executable DIRECTLY (no `open`, no
# `lsregister`) is enough — macOS resolves the bundle from the executable's
# path, so identity holds for every restart.
#
# Escape hatch: CODEFACTORY_DEV_BUNDLE_IDENTITY=0 runs the binary bare.
set -uo pipefail

[ $# -ge 1 ] || { echo "usage: $0 <binary> [args...]" >&2; exit 2; }

BIN="$1"; shift

# The app binary is the only thing worth bundling. cargo applies a runner to
# `cargo test` and `cargo bench` too, so every other executable — notably the
# test binaries under deps/ — has to run exactly as cargo intended.
APP_BIN_NAME="${CODEFACTORY_DEV_BIN_NAME:-codefactory}"
BUNDLE_NAME="${CODEFACTORY_DEV_BUNDLE_NAME:-CodeFactoryDev}"
BUNDLE_ID="${CODEFACTORY_DEV_BUNDLE_ID:-com.codefactory.dev}"

BIN_DIR="$(cd "$(dirname "$BIN")" && pwd)"

# Pass-through cases are inline rather than in a helper: `exec "$BIN" "$@"`
# inside a function would forward the FUNCTION's arguments, not the app's.
if [ "${CODEFACTORY_DEV_BUNDLE_IDENTITY:-1}" = "0" ]; then
    exec "$BIN" "$@"
fi
if [ "$(uname -s)" != "Darwin" ]; then
    exec "$BIN" "$@"
fi
if [ "$(basename "$BIN")" != "$APP_BIN_NAME" ] || [ "$(basename "$BIN_DIR")" = "deps" ]; then
    exec "$BIN" "$@"
fi

BUNDLE="$BIN_DIR/$BUNDLE_NAME.app"
CONTENTS="$BUNDLE/Contents"
BUNDLED_BIN="$CONTENTS/MacOS/$BUNDLE_NAME"

if ! mkdir -p "$CONTENTS/MacOS"; then
    echo "dev-app-bundle-runner: cannot create $CONTENTS/MacOS — running bare" >&2
    exec "$BIN" "$@"
fi

# Rewritten every launch so an upgraded runner takes effect immediately
# rather than after someone remembers to delete the target directory.
cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$BUNDLE_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$BUNDLE_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$BUNDLE_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleVersion</key>
    <string>0.0.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.0.0-dev</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# Tauri resolves resources from the executable's own directory in dev, which
# it recognises by "…/target/<profile>" plus a .cargo-lock. From inside
# Contents/MacOS that test fails and tauri-utils falls through to
# ../Resources, so point that straight back at the cargo output directory.
# Without this the dev app silently loses its built-in skills.
if [ ! -L "$CONTENTS/Resources" ]; then
    rm -rf "$CONTENTS/Resources"
    ln -s "../.." "$CONTENTS/Resources"
fi

# cargo REPLACES the binary on rebuild, so an existing hardlink still points
# at the previous inode. Relink every launch or we would serve stale code and
# collect live-verification evidence against a build that no longer exists.
rm -f "$BUNDLED_BIN"
if ! ln "$BIN" "$BUNDLED_BIN" 2>/dev/null; then
    # Different filesystem (or a target dir that forbids links): copy instead.
    if ! cp -f "$BIN" "$BUNDLED_BIN"; then
        echo "dev-app-bundle-runner: cannot stage $BUNDLED_BIN — running bare" >&2
        exec "$BIN" "$@"
    fi
fi

echo "dev-app-bundle-runner: exec $BUNDLED_BIN ($BUNDLE_ID)" >&2
exec "$BUNDLED_BIN" "$@"
