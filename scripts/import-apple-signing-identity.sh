#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Apple signing identity import requires macOS" >&2
  exit 2
fi

required=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD)
missing=()
for name in "${required[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    missing+=("$name")
  fi
done
if (( ${#missing[@]} > 0 )); then
  printf 'Apple signing identity import failed: missing %s\n' "${missing[*]}" >&2
  exit 1
fi

: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"

# This password only protects the ephemeral runner keychain. Generate it in
# the job instead of turning disposable encryption material into a long-lived
# repository secret.
KEYCHAIN_PASSWORD="${KEYCHAIN_PASSWORD:-$(openssl rand -hex 32)}"
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  echo "::add-mask::$KEYCHAIN_PASSWORD"
fi

KEYCHAIN_PATH="$RUNNER_TEMP/codefactory-signing.keychain-db"
CERTIFICATE_PATH="$RUNNER_TEMP/codefactory-developer-id.p12"

# Never print certificate material or passwords. The hosted runner is
# ephemeral, but the keychain is still scoped to this job and removed by an
# always() cleanup step.
/usr/bin/base64 -D <<<"$APPLE_CERTIFICATE" > "$CERTIFICATE_PATH"
chmod 600 "$CERTIFICATE_PATH"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security import "$CERTIFICATE_PATH" \
  -k "$KEYCHAIN_PATH" \
  -P "$APPLE_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign \
  -T /usr/bin/security >/dev/null
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH" >/dev/null

# The job does not require login-keychain identities. Restricting the search
# list prevents an unrelated runner identity from being selected by accident.
security list-keychains -d user -s "$KEYCHAIN_PATH"

IDENTITIES="$({
  security find-identity -v -p codesigning "$KEYCHAIN_PATH" |
    sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p'
} || true)"
IDENTITY_COUNT="$(printf '%s\n' "$IDENTITIES" | sed '/^$/d' | wc -l | tr -d ' ')"
if [[ "$IDENTITY_COUNT" -ne 1 ]]; then
  echo "Apple signing identity import failed: expected exactly one valid Developer ID Application identity, found $IDENTITY_COUNT" >&2
  exit 1
fi

IDENTITY="$IDENTITIES"
printf 'APPLE_SIGNING_IDENTITY=%s\n' "$IDENTITY" >> "$GITHUB_ENV"
printf 'CODEFACTORY_SIGNING_KEYCHAIN=%s\n' "$KEYCHAIN_PATH" >> "$GITHUB_ENV"
echo "Apple Developer ID Application identity imported into ephemeral keychain"
