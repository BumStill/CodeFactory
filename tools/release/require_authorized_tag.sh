#!/usr/bin/env bash
# Fail closed unless the remote release tag still points at the authorized commit.
#
# A release job freezes one authorized SHA up front, but a tag is a mutable ref:
# it can be moved while a job is doing something slow. Checking once when the job
# starts is not enough — between that check and `tauri-action`'s upload sits a
# full Tauri compile, and between it and `gh release edit --draft=false` sits the
# whole latest.json assembly. This script exists so the check can sit immediately
# next to every mutation instead of only at the top of a job.
#
# Usage: require_authorized_tag.sh <tag> <authorized-sha> [context]
#
# Reads nothing, writes nothing, mutates nothing: it fetches the tag ref and
# compares. Exit 0 means the caller may proceed with its mutation; any non-zero
# exit must abort the caller before it publishes, tags, builds or uploads.
set -euo pipefail

TAG="${1:-${TAG:-}}"
AUTHORIZED_TAG_SHA="${2:-${AUTHORIZED_TAG_SHA:-}}"
CONTEXT="${3:-${CONTEXT:-release mutation}}"

if [ -z "$TAG" ] || [ -z "$AUTHORIZED_TAG_SHA" ]; then
  echo "::error::require_authorized_tag needs <tag> and <authorized-sha>" >&2
  exit 2
fi

git fetch --force origin "refs/tags/$TAG:refs/tags/$TAG"
REMOTE_TAG_SHA="$(git rev-parse "${TAG}^{commit}")"

if [ "$REMOTE_TAG_SHA" != "$AUTHORIZED_TAG_SHA" ]; then
  echo "::error::release tag moved to $REMOTE_TAG_SHA after authorization; expected $AUTHORIZED_TAG_SHA (blocked before: $CONTEXT)" >&2
  exit 1
fi

echo "release tag $TAG still authorized at $AUTHORIZED_TAG_SHA ($CONTEXT)"
