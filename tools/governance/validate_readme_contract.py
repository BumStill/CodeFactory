#!/usr/bin/env python3
"""Validate the README contract and the machine-readable PR decision.

README.md is an evergreen product contract. Release-specific detail belongs in
GitHub Release notes, so this check deliberately rejects hard-coded release
versions and only requires a README diff when a PR declares a user-visible
contract change.

The validator has two layers:

* static checks run on every push and pull request (headings, maintenance
  marker, latest-release link, semver drift, and local Markdown links), and
* PR checks which require exactly one ``README-Update`` decision and reason;
  ``required`` must include README.md in the base-to-head diff.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
README_MARKER = "README-CONTRACT: evergreen"
REQUIRED_HEADINGS = (
    "## Features",
    "## Install",
    "## Quick start",
    "## Build from source",
    "## Data & privacy",
    "## Architecture",
    "## License",
)
EXACT_VERSION_RE = re.compile(r"(?<![A-Za-z0-9])v?\d+\.\d+\.\d+(?![A-Za-z0-9])")
MARKDOWN_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
DECISION_RE = re.compile(r"^[ \t]*README-Update:[ \t]*(required|reviewed)[ \t]*$", re.IGNORECASE | re.MULTILINE)
ANY_DECISION_RE = re.compile(r"^[ \t]*README-Update:[ \t]*(.+?)[ \t]*$", re.IGNORECASE | re.MULTILINE)
REASON_RE = re.compile(r"^[ \t]*README-Update-Reason:[ \t]*(.+?)[ \t]*$", re.IGNORECASE | re.MULTILINE)
PLACEHOLDER_RE = re.compile(r"^<[^>]+>$|\b(?:tbd|todo|fill[ -]?in|n/?a)\b", re.IGNORECASE)
FENCED_BLOCK_RE = re.compile(r"```.*?```", re.DOTALL)
INLINE_CODE_RE = re.compile(r"`[^`\n]*`")


def _git(repo_root: Path, *args: str) -> tuple[bool, str, str]:
    result = subprocess.run(
        ["git", "-C", str(repo_root), *args],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0, result.stdout.strip(), result.stderr.strip()


def _prose_only(text: str) -> str:
    """Blank out code so a documented toolchain pin is not read as a release claim.

    `## Build from source` is supposed to carry `rustup toolchain install
    1.83.0`; scanning it for release versions failed CI and told the author to
    move it to Release notes, which is wrong for a toolchain pin. The decision
    parser already strips fenced blocks for the same reason — this applies the
    same treatment to the version scan, plus inline spans (``Rust `1.83.0` ``).

    Replaces with same-length blanks so byte offsets — and therefore reported
    line numbers — stay correct.
    """

    def blank(match: re.Match[str]) -> str:
        return "".join("\n" if ch == "\n" else " " for ch in match.group(0))

    return INLINE_CODE_RE.sub(blank, FENCED_BLOCK_RE.sub(blank, text))


def _line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def _is_external_link(target: str) -> bool:
    lowered = target.lower()
    return lowered.startswith(
        ("https://", "http://", "mailto:", "#", "data:", "tel:")
    )


def _local_link_errors(repo_root: Path, readme_path: Path, text: str) -> list[str]:
    errors: list[str] = []
    for match in MARKDOWN_LINK_RE.finditer(text):
        raw_target = match.group(1).strip()
        target = raw_target.split(None, 1)[0] if raw_target else ""
        if not target or _is_external_link(target):
            continue
        target = target.split("#", 1)[0].split("?", 1)[0]
        if not target:
            continue
        if target.startswith("/"):
            resolved = repo_root / target.lstrip("/")
        else:
            resolved = (readme_path.parent / target).resolve()
        if not resolved.exists():
            line = _line_number(text, match.start())
            errors.append(
                f"README.md:{line}: missing local link target '{target}'"
            )
    return errors


def validate_static(repo_root: Path, readme_path: Path | None = None) -> list[str]:
    """Return static README contract violations for ``repo_root``."""

    readme_path = readme_path or repo_root / "README.md"
    if not readme_path.exists():
        return [f"README contract file is missing: {readme_path}"]
    try:
        text = readme_path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        return [f"README.md is not valid UTF-8: {exc}"]

    errors: list[str] = []
    if README_MARKER not in text:
        errors.append(
            "README.md: missing '<!-- README-CONTRACT: evergreen -->' maintenance marker"
        )
    for heading in REQUIRED_HEADINGS:
        if heading not in text:
            errors.append(f"README.md: missing required heading '{heading}'")
    if "releases/latest" not in text:
        errors.append(
            "README.md: keep at least one releases/latest link for version-neutral downloads"
        )
    versions = sorted(set(EXACT_VERSION_RE.findall(_prose_only(text))))
    if versions:
        errors.append(
            "README.md: exact version(s) "
            + ", ".join(versions)
            + " found; release-specific versions belong in Release notes"
        )
    errors.extend(_local_link_errors(repo_root, readme_path, text))
    return errors


def _parse_pr_decision(body: str) -> tuple[str | None, list[str]]:
    # A code sample must not accidentally become the PR's machine decision.
    body = FENCED_BLOCK_RE.sub("", body)
    errors: list[str] = []
    all_decisions = list(ANY_DECISION_RE.finditer(body))
    valid_decisions = list(DECISION_RE.finditer(body))
    if len(all_decisions) != 1:
        errors.append(
            "PR body must contain exactly one 'README-Update: required|reviewed' line"
        )
    elif len(valid_decisions) != 1:
        value = all_decisions[0].group(1).strip()
        errors.append(
            f"README-Update value '{value}' is invalid; use required or reviewed"
        )

    reasons = list(REASON_RE.finditer(body))
    if len(reasons) != 1:
        errors.append(
            "PR body must contain exactly one non-empty 'README-Update-Reason:' line"
        )
    elif not reasons[0].group(1).strip() or PLACEHOLDER_RE.search(reasons[0].group(1).strip()):
        errors.append(
            "README-Update-Reason must explain the product impact and cannot be a placeholder"
        )

    decision = valid_decisions[0].group(1).lower() if len(valid_decisions) == 1 else None
    return decision, errors


def _changed_files(repo_root: Path, base_sha: str) -> tuple[list[str], str | None]:
    if not base_sha:
        return [], "README contract needs a base SHA for pull-request validation"
    ok, stdout, stderr = _git(repo_root, "diff", "--name-only", f"{base_sha}...HEAD")
    if not ok:
        detail = stderr or "unknown git diff error"
        return [], f"cannot compare PR with base '{base_sha}': {detail}"
    return [line for line in stdout.splitlines() if line.strip()], None


def validate_pr_contract(repo_root: Path, body: str, base_sha: str) -> list[str]:
    """Return static and PR decision violations for a pull request."""

    errors = validate_static(repo_root)
    decision, decision_errors = _parse_pr_decision(body)
    errors.extend(decision_errors)
    if decision != "required":
        return errors

    changed, diff_error = _changed_files(repo_root, base_sha)
    if diff_error:
        errors.append(diff_error)
    elif "README.md" not in changed:
        errors.append(
            "README-Update: required but this PR must change README.md"
        )
    return errors


def _live_pr_body(number: Any) -> str | None:
    """Fetch the PR's CURRENT body via gh, or None when unavailable.

    The event payload is a snapshot taken when the run was triggered. Re-running
    a failed check replays that snapshot, so a body fixed after the fact can
    never pass — the only escape is a fresh commit. That turned a one-line
    body omission into a hard stop on the release pipeline (2026-08-03, PR #301
    for v1.77.0: body corrected, three consecutive re-runs still read the stale
    payload). The contract should judge the PR as it stands now, which is also
    what a reviewer sees.
    """

    try:
        number = int(number)
    except (TypeError, ValueError):
        return None
    result = subprocess.run(
        ["gh", "pr", "view", str(number), "--json", "body", "--jq", ".body"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(
            "::warning::README contract could not read the live PR body "
            f"({result.stderr.strip() or 'gh failed'}); falling back to the event payload"
        )
        return None
    return result.stdout


def _event_payload(path: str | None) -> dict[str, Any] | None:
    if not path:
        return None
    try:
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"::warning::README contract could not read event payload: {exc}")
        return None
    return payload if isinstance(payload, dict) else None


def _run(args: argparse.Namespace) -> int:
    repo_root = Path(args.repo).resolve() if args.repo else REPO_ROOT
    errors: list[str]

    if args.body_file:
        body = Path(args.body_file).read_text(encoding="utf-8")
        base_sha = args.base_ref or os.environ.get("README_CONTRACT_BASE_SHA", "").strip()
        errors = validate_pr_contract(repo_root, body, base_sha)
    elif args.ci:
        event = _event_payload(
            os.environ.get("README_CONTRACT_EVENT_PATH")
            or os.environ.get("GITHUB_EVENT_PATH")
        )
        pull_request = event.get("pull_request") if event else None
        if isinstance(pull_request, dict):
            live = _live_pr_body(pull_request.get("number"))
            body = live if live is not None else (pull_request.get("body") or "")
            base = pull_request.get("base")
            event_base = base.get("sha") if isinstance(base, dict) else ""
            base_sha = (
                os.environ.get("README_CONTRACT_BASE_SHA", "").strip()
                or str(event_base or "").strip()
            )
            errors = validate_pr_contract(repo_root, body, base_sha)
        else:
            errors = validate_static(repo_root)
    else:
        errors = validate_static(repo_root)

    if errors:
        print(f"readme-contract: {len(errors)} error(s)")
        for error in errors:
            print(f"::error::{error}")
        return 1
    print("readme-contract: OK")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ci", action="store_true", help="read the GitHub event payload")
    parser.add_argument("--body-file", help="validate a PR body from a local file")
    parser.add_argument("--base-ref", help="base SHA/ref for --body-file")
    parser.add_argument("--repo", help="repository root (defaults to this checkout)")
    args = parser.parse_args()
    if args.ci and args.body_file:
        parser.error("--ci and --body-file are mutually exclusive")
    return _run(args)


if __name__ == "__main__":
    sys.exit(main())
