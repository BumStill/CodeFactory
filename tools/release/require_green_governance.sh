#!/usr/bin/env bash
# Fail closed unless governance-baseline concluded `success` on an exact main commit.
#
# The release gate used to discard every gh/API error (`2>/dev/null || echo ""`)
# and report it as "run not found". On 2026-09-29 release run 36554865773 was
# blocked that way on two attempts while governance-baseline run 36547443796 for
# the same SHA was completed/success: the API was intermittently answering EOF.
# A transport error says nothing about the run, so the lookup is retried with
# backoff and gh's stderr is always printed. What never happens is inferring
# green: only a completed run whose conclusion is `success` passes.
#
# Usage: require_green_governance.sh <40-char-commit-sha>
#
# Read-only. Exit 0: the latest governance-baseline run on main for that exact
# SHA concluded success. Exit 1: it concluded anything else, or it could not be
# confirmed within the retry budget. Exit 2: usage error.
set -euo pipefail

HEAD_SHA="${1:-}"
WORKFLOW="governance-baseline"
ATTEMPTS=4
BACKOFF_SECONDS=10 # waits 10s, 20s, 30s: at most one minute before failing closed

# The SHA is interpolated into the jq filter below, so accept nothing but a SHA.
if ! [[ "$HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error::require_green_governance needs a full 40-character commit SHA, got '$HEAD_SHA'" >&2
  exit 2
fi

ERR_FILE="$(mktemp)"
trap 'rm -f "$ERR_FILE"' EXIT

REASON=""
for (( attempt = 1; attempt <= ATTEMPTS; attempt++ )); do
  if [ "$attempt" -gt 1 ]; then
    sleep $(( BACKOFF_SECONDS * (attempt - 1) ))
  fi

  # --commit asks the API for runs of this exact head SHA instead of scanning a
  # window of recent main runs; select() still re-checks the SHA rather than
  # trusting the server-side filter.
  if RUN="$(gh run list --workflow "$WORKFLOW" --branch main --commit "$HEAD_SHA" \
      --limit 20 --json headSha,status,conclusion,databaseId \
      --jq "[.[] | select(.headSha == \"$HEAD_SHA\")][0] // empty
            | \"\(.status)|\(.conclusion)|\(.databaseId)\"" 2>"$ERR_FILE")"; then
    GH_STATUS=0
  else
    GH_STATUS=$?
  fi
  GH_ERR="$(cat "$ERR_FILE")"
  if [ -n "$GH_ERR" ]; then
    echo "gh stderr (lookup $attempt/$ATTEMPTS):" >&2
    printf '%s\n' "$GH_ERR" >&2
  fi

  if [ "$GH_STATUS" -ne 0 ]; then
    FIRST_ERR_LINE="${GH_ERR%%$'\n'*}"
    REASON="gh exited $GH_STATUS: ${FIRST_ERR_LINE:-no stderr}"
  elif [ -z "$RUN" ]; then
    REASON="no $WORKFLOW run on main for $HEAD_SHA yet"
  else
    IFS='|' read -r STATUS CONCLUSION RUN_ID <<< "$RUN"
    if [ "$STATUS" != "completed" ]; then
      REASON="$WORKFLOW run $RUN_ID is still $STATUS"
    elif [ "$CONCLUSION" = "success" ]; then
      echo "$WORKFLOW for $HEAD_SHA: success (run $RUN_ID)"
      exit 0
    else
      # A finished run that is not green is an answer, not an outage.
      echo "::error::$WORKFLOW run $RUN_ID for $HEAD_SHA concluded '$CONCLUSION' — refusing to release on red." >&2
      exit 1
    fi
  fi
  echo "lookup $attempt/$ATTEMPTS not confirmed: $REASON" >&2
done

echo "::error::$WORKFLOW for $HEAD_SHA not confirmed green after $ATTEMPTS lookups (last: $REASON) — refusing to infer green." >&2
exit 1
