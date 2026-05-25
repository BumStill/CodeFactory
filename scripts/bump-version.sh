#!/usr/bin/env bash
# Cut a release. See VERSIONING.md for the policy.
#
# Usage:
#   ./scripts/bump-version.sh patch        # 0.4.0 -> 0.4.1
#   ./scripts/bump-version.sh minor        # 0.4.1 -> 0.5.0
#   ./scripts/bump-version.sh major        # 0.4.1 -> 1.0.0
#   ./scripts/bump-version.sh 1.2.3        # explicit version
#   ./scripts/bump-version.sh              # defaults to patch
#
# Requires: git, node (for package.json), sed

set -euo pipefail
cd "$(dirname "$0")/.."

# ── Resolve current version ──────────────────────────────────────────────────
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "v0.0.0")
CURRENT="${LAST_TAG#v}"
IFS='.' read -r C_MAJOR C_MINOR C_PATCH <<< "$CURRENT"

SLOT="${1:-patch}"

case "$SLOT" in
  patch) NEW="$C_MAJOR.$C_MINOR.$((C_PATCH + 1))" ;;
  minor) NEW="$C_MAJOR.$((C_MINOR + 1)).0" ;;
  major) NEW="$((C_MAJOR + 1)).0.0" ;;
  [0-9]*) NEW="$SLOT" ;;  # explicit version
  *) echo "Usage: $0 [patch|minor|major|x.y.z]" >&2; exit 1 ;;
esac

TAG="v$NEW"
echo "Bumping $LAST_TAG → $TAG"

# ── Guard: must be on main with clean tree ───────────────────────────────────
BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ "$BRANCH" != "main" ]]; then
  echo "Error: must be on main branch (currently on '$BRANCH')" >&2
  exit 1
fi
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Error: working tree has uncommitted changes" >&2
  exit 1
fi

# ── Update version files ─────────────────────────────────────────────────────

# package.json
node -e "
const fs = require('fs');
const p = JSON.parse(fs.readFileSync('package.json', 'utf8'));
p.version = '$NEW';
fs.writeFileSync('package.json', JSON.stringify(p, null, 2) + '\n');
"

# src-tauri/Cargo.toml  (first [package] block only)
if [[ "$(uname)" == "Darwin" ]]; then
  sed -i '' "0,/^version = \"[^\"]*\"/{s/^version = \"[^\"]*\"/version = \"$NEW\"/}" src-tauri/Cargo.toml
else
  sed -i "0,/^version = \"[^\"]*\"/{s/^version = \"[^\"]*\"/version = \"$NEW\"/}" src-tauri/Cargo.toml
fi

# src-tauri/tauri.conf.json
node -e "
const fs = require('fs');
const c = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
c.version = '$NEW';
fs.writeFileSync('src-tauri/tauri.conf.json', JSON.stringify(c, null, 2) + '\n');
"

# ── Commit, tag, push ────────────────────────────────────────────────────────
git add package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json
git commit -m "chore: bump version to $NEW"
git tag "$TAG"
git push origin main
git push origin "$TAG"

echo ""
echo "Released $TAG — GitHub Actions will build the MSI and publish the release."
