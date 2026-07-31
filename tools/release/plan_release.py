#!/usr/bin/env python3
"""Plan one batched release from the commits in a git revision range."""

from __future__ import annotations

import argparse
import json
import subprocess
from dataclasses import asdict, dataclass
from pathlib import Path


SLOT_PRIORITY = {"none": 0, "patch": 1, "minor": 2, "major": 3}
ALLOWED_URGENCIES = {"immediate", "hold"}


@dataclass(frozen=True)
class ReleasePlan:
    slot: str
    skip: bool
    skip_reason: str
    immediate: int
    hold: int
    invalid_urgency: int


def _git(repo: Path, *args: str, stdin: str | None = None) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        input=stdin,
        text=True,
        capture_output=True,
    )
    return result.stdout


def _subject_slot(subject: str) -> str:
    if subject.startswith("chore: bump version to"):
        return "none"

    prefix, separator, _ = subject.partition(":")
    if not separator:
        return "none"

    if prefix.endswith("!"):
        return "major"
    if prefix == "feat" or (prefix.startswith("feat(") and prefix.endswith(")")):
        return "minor"
    if prefix == "fix" or (prefix.startswith("fix(") and prefix.endswith(")")):
        return "patch"
    return "none"


def _footer_lines(body: str) -> list[str]:
    lines = body.rstrip().splitlines()
    if not lines:
        return []

    footer_start = 0
    for index in range(len(lines) - 1, -1, -1):
        if not lines[index].strip():
            footer_start = index + 1
            break
    return lines[footer_start:]


def _has_breaking_footer(body: str) -> bool:
    return any(
        line.startswith("BREAKING CHANGE:") or line.startswith("BREAKING-CHANGE:")
        for line in _footer_lines(body)
    )


def _release_urgencies(body: str) -> list[str]:
    values: list[str] = []
    for line in _footer_lines(body):
        key, separator, value = line.partition(":")
        if separator and key.strip().casefold() == "release-urgency":
            values.append(value.strip().casefold())
    return values


def plan_release(
    repo: Path,
    revision_range: str,
    *,
    force: bool = False,
    allow_guarded_batch: bool = False,
) -> ReleasePlan:
    shas = [
        sha
        for sha in _git(repo, "rev-list", "--reverse", revision_range).splitlines()
        if sha
    ]

    slot = "none"
    immediate = 0
    hold = 0
    invalid_urgency = 0

    for sha in shas:
        subject = _git(repo, "show", "-s", "--format=%s", sha).rstrip("\n")
        candidate = _subject_slot(subject)

        body = _git(repo, "show", "-s", "--format=%B", sha)
        if _has_breaking_footer(body):
            candidate = "major"
        if SLOT_PRIORITY[candidate] > SLOT_PRIORITY[slot]:
            slot = candidate

        for urgency in _release_urgencies(body):
            if urgency == "immediate":
                immediate += 1
            elif urgency == "hold":
                hold += 1
            elif urgency not in ALLOWED_URGENCIES:
                invalid_urgency += 1

    guarded = hold > 0 or invalid_urgency > 0
    if guarded and not allow_guarded_batch:
        reasons = []
        if hold:
            reasons.append(f"{hold} hold trailer(s)")
        if invalid_urgency:
            reasons.append(f"{invalid_urgency} invalid urgency trailer(s)")
        return ReleasePlan(
            slot=slot,
            skip=True,
            skip_reason="guarded batch: " + ", ".join(reasons),
            immediate=immediate,
            hold=hold,
            invalid_urgency=invalid_urgency,
        )

    if slot == "none":
        if force:
            slot = "patch"
        else:
            return ReleasePlan(
                slot=slot,
                skip=True,
                skip_reason="no feat/fix since the last tag",
                immediate=immediate,
                hold=hold,
                invalid_urgency=invalid_urgency,
            )

    return ReleasePlan(
        slot=slot,
        skip=False,
        skip_reason="",
        immediate=immediate,
        hold=hold,
        invalid_urgency=invalid_urgency,
    )


def _parse_bool(value: str) -> bool:
    return value.strip().casefold() == "true"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--range", dest="revision_range", required=True)
    parser.add_argument("--force", default="false")
    parser.add_argument("--allow-guarded-batch", default="false")
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()

    plan = plan_release(
        args.repo,
        args.revision_range,
        force=_parse_bool(args.force),
        allow_guarded_batch=_parse_bool(args.allow_guarded_batch),
    )
    print(json.dumps(asdict(plan), ensure_ascii=False, sort_keys=True))

    if args.github_output:
        lines = [
            f"slot={plan.slot}",
            f"skip={'true' if plan.skip else 'false'}",
            f"skip_reason={plan.skip_reason}",
            f"immediate={plan.immediate}",
            f"hold={plan.hold}",
            f"invalid_urgency={plan.invalid_urgency}",
        ]
        with args.github_output.open("a", encoding="utf-8") as output:
            output.write("\n".join(lines) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
