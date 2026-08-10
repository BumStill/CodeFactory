#!/usr/bin/env python3
"""Require a test alongside user-visible frontend changes.

Audit of the week to 2026-08-05: every backend fix carried tests, and **every
frontend fix carried none** — four `feat`/`fix` commits changed layout, status
indicators and a new tab surface with zero test diff. That is not sporadic
forgetfulness; nothing on the frontend path ever asked. CI checks colour pairing
(`lightModeAudit`) but never asked whether behaviour was covered, and AGENTS.md
already records stick-to-bottom being re-fixed six times for exactly this reason.

The contract: a `feat`/`fix` PR that edits frontend source must also edit a test,
or state in its body why a test cannot express the change.

The escape hatch is deliberate. A gate with no way forward would be a dead end,
which this product forbids — but it is not free: the reason must be specific,
and placeholders are rejected, so declaring it costs more than writing the test
in the ordinary case.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

FRONTEND_SRC = re.compile(r"^src/.*\.(tsx?|jsx?)$")
IS_TEST = re.compile(r"(\.test\.|\.spec\.|__tests__/|^tests/|acceptance/)")
# Only user-visible change types. A refactor rides existing coverage; chore and
# docs change nothing a user can observe.
ENFORCED_TYPES = re.compile(r"^(feat|fix)(\([^)]*\))?!?:", re.IGNORECASE)
DECLARATION = re.compile(r"^[ \t]*UI-Test:[ \t]*(.+?)[ \t]*$", re.IGNORECASE | re.MULTILINE)
PLACEHOLDER = re.compile(r"^<[^>]+>$|\b(?:tbd|todo|n/?a|none|fill[ -]?in)\b", re.IGNORECASE)
FENCED = re.compile(r"```.*?```", re.DOTALL)


def changed_files(repo_root: Path, base_sha: str) -> tuple[list[str], str | None]:
    if not base_sha:
        return [], "frontend test contract needs a base SHA"
    result = subprocess.run(
        ["git", "-C", str(repo_root), "diff", "--name-only", f"{base_sha}...HEAD"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return [], f"cannot diff against '{base_sha}': {result.stderr.strip()}"
    return [line for line in result.stdout.splitlines() if line.strip()], None


def validate(title: str, body: str, files: list[str]) -> list[str]:
    """Return violations for one pull request."""
    if not ENFORCED_TYPES.match(title.strip()):
        return []
    touched_ui = [f for f in files if FRONTEND_SRC.match(f) and not IS_TEST.search(f)]
    if not touched_ui:
        return []
    if any(IS_TEST.search(f) for f in files):
        return []

    # No test diff — a declaration is the only other way through.
    reason = DECLARATION.search(FENCED.sub("", body))
    if not reason:
        return [
            "frontend source changed with no test: "
            + ", ".join(sorted(touched_ui)[:5])
            + ". Add a test, or state why one cannot express this change with a "
            "'UI-Test: <reason>' line in the PR body."
        ]
    text = reason.group(1).strip()
    if not text or PLACEHOLDER.search(text):
        return [
            f"UI-Test reason must be specific about what a test cannot capture; got '{text}'"
        ]
    return []


def _event_body_and_title(path: str | None) -> tuple[str, str]:
    if not path or not Path(path).exists():
        return "", ""
    try:
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return "", ""
    pr = payload.get("pull_request")
    if not isinstance(pr, dict):
        return "", ""
    return pr.get("body") or "", pr.get("title") or ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ci", action="store_true")
    parser.add_argument("--title")
    parser.add_argument("--body-file")
    parser.add_argument("--base-ref")
    parser.add_argument("--repo")
    args = parser.parse_args()

    repo_root = Path(args.repo).resolve() if args.repo else REPO_ROOT
    if args.body_file:
        body = Path(args.body_file).read_text(encoding="utf-8")
        title = args.title or ""
    else:
        body, title = _event_body_and_title(
            os.environ.get("FRONTEND_TEST_EVENT_PATH") or os.environ.get("GITHUB_EVENT_PATH")
        )
    base = args.base_ref or os.environ.get("FRONTEND_TEST_BASE_SHA", "").strip()

    files, diff_error = changed_files(repo_root, base)
    if diff_error:
        # Cannot see the diff → cannot claim a violation. Never invent one.
        print(f"::warning::frontend test contract skipped: {diff_error}")
        return 0

    errors = validate(title, body, files)
    if errors:
        print(f"frontend-test-contract: {len(errors)} error(s)")
        for error in errors:
            print(f"::error::{error}")
        return 1
    print("frontend-test-contract: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
