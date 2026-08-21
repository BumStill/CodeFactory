#!/usr/bin/env bash
set -u

fixture_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

case "${1:-}" in
  --help)
    cat "$fixture_root/references/usage.txt"
    ;;
  --version)
    version="$(cat "$fixture_root/assets/version.txt")"
    printf 'SKILL_RECOVERY_OK version=%s\n' "$version"
    ;;
  *)
    printf 'probe.sh: unrecognized option: %s\n' "${1:-<missing>}" >&2
    exit 2
    ;;
esac
