#!/usr/bin/env python3
"""Enforce the Windows CI UTF-8 contract (M49).

Two failure modes have repeatedly broken Windows CI:

* a governance script printing non-ASCII text under the runner's default
  ``cp1252`` stdout, raising ``UnicodeEncodeError`` and hiding the real error
  (2026-10-10, #593), and
* a script reading subprocess output in text mode without an explicit
  ``encoding``, raising ``UnicodeDecodeError`` (2026-10-17, #680).

The contract has two halves: every workflow under ``.github/workflows/``
enables Python's UTF-8 mode (top-level ``env`` or per-job ``env``), and every
text-mode subprocess call in ``tools/`` and ``scripts/`` pins
``encoding="utf-8"``.

Run with no arguments from the repository root; exits non-zero on any
violation. See ``docs/specs/feature-specs/windows-ci-python-utf8.md``.
"""
from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

REQUIRED_WORKFLOW_ENV = {"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8"}
SUBPROCESS_CALLS = {"run", "Popen", "check_output", "call", "check_call"}
TEXT_MODE_KEYWORDS = {"text", "universal_newlines"}
SCANNED_ROOTS = ("tools", "scripts")


def _normalise_value(raw: str) -> str:
    return raw.strip().strip("'\"").strip().lower()


def _indent_of(line: str) -> int:
    if not line.strip():
        return -1
    return len(line) - len(line.lstrip(" "))


def _mapping_entries(lines: list[str], start: int, indent: int) -> dict[str, str]:
    """Collect ``KEY: value`` entries one level below the block at ``start``."""

    entries: dict[str, str] = {}
    child_indent = indent + 2
    for line in lines[start + 1 :]:
        current = _indent_of(line)
        if current == -1 or line.lstrip().startswith("#"):
            continue
        if current <= indent:
            break
        if current == child_indent:
            key, _, value = line.strip().partition(":")
            entries[key.strip()] = _normalise_value(value)
    return entries


def _has_utf8_env(entries: dict[str, str]) -> bool:
    return all(entries.get(key) == value for key, value in REQUIRED_WORKFLOW_ENV.items())


def _missing_utf8_keys(entries: dict[str, str]) -> list[str]:
    return [
        key
        for key, value in REQUIRED_WORKFLOW_ENV.items()
        if entries.get(key) != value
    ]


def workflow_env_violations(label: str, text: str) -> list[str]:
    """Return R1 violations for one workflow document."""

    lines = text.splitlines()
    top_index = next(
        (index for index, line in enumerate(lines) if _indent_of(line) == 0 and line.strip() == "env:"),
        None,
    )
    if top_index is not None and _has_utf8_env(_mapping_entries(lines, top_index, 0)):
        return []

    jobs_index = next(
        (index for index, line in enumerate(lines) if _indent_of(line) == 0 and line.strip() == "jobs:"),
        None,
    )
    if jobs_index is None:
        return [
            f"{label}: no workflow-level `env:` with PYTHONUTF8/PYTHONIOENCODING and no `jobs:` block (R1)"
        ]

    job_starts = [
        index
        for index in range(jobs_index + 1, len(lines))
        if _indent_of(lines[index]) == 2 and lines[index].strip().endswith(":")
    ]
    if not job_starts:
        return [f"{label}: `jobs:` declares no jobs to inherit the UTF-8 environment (R1)"]

    violations: list[str] = []
    for position, start in enumerate(job_starts):
        end = job_starts[position + 1] if position + 1 < len(job_starts) else len(lines)
        env_index = next(
            (
                index
                for index in range(start + 1, end)
                if _indent_of(lines[index]) == 4 and lines[index].strip() == "env:"
            ),
            None,
        )
        entries = _mapping_entries(lines, env_index, 4) if env_index is not None else {}
        missing = _missing_utf8_keys(entries)
        if missing:
            job_name = lines[start].strip().rstrip(":")
            violations.append(
                f"{label}: job `{job_name}` does not set {', '.join(missing)} (R1)"
            )
    return violations


def _is_subprocess_callable(node: ast.expr) -> bool:
    if isinstance(node, ast.Attribute) and node.attr in SUBPROCESS_CALLS:
        base = node.value
        while isinstance(base, ast.Attribute):
            base = base.value
        return isinstance(base, ast.Name) and base.id == "subprocess"
    return False


def python_subprocess_violations(label: str, source: str) -> list[str]:
    """Return R2 violations for one Python module's source."""

    try:
        tree = ast.parse(source, filename=label)
    except SyntaxError as exc:
        return [f"{label}: cannot parse for the UTF-8 contract: {exc}"]

    violations: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or not _is_subprocess_callable(node.func):
            continue
        keyword_names = {keyword.arg for keyword in node.keywords if keyword.arg}
        if not keyword_names & TEXT_MODE_KEYWORDS:
            continue
        if "encoding" in keyword_names:
            continue
        violations.append(
            f'{label}:{node.lineno}: text-mode subprocess call without '
            'encoding="utf-8" (R2)'
        )
    return violations


def workflow_violations(workflows_dir: Path) -> list[str]:
    if not workflows_dir.is_dir():
        return [f"{workflows_dir}: workflow directory is missing (R1)"]
    violations: list[str] = []
    for path in sorted(workflows_dir.glob("*.yml")):
        violations.extend(
            workflow_env_violations(
                f".github/workflows/{path.name}", path.read_text(encoding="utf-8")
            )
        )
    return violations


def python_violations(repo_root: Path) -> list[str]:
    violations: list[str] = []
    for root_name in SCANNED_ROOTS:
        root = repo_root / root_name
        if not root.is_dir():
            continue
        for path in sorted(root.rglob("*.py")):
            relative = path.relative_to(repo_root)
            violations.extend(
                python_subprocess_violations(
                    str(relative), path.read_text(encoding="utf-8")
                )
            )
    return violations


def collect(repo_root: Path) -> list[str]:
    return workflow_violations(repo_root / ".github" / "workflows") + python_violations(
        repo_root
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo",
        help="repository root (defaults to this checkout)",
    )
    args = parser.parse_args(argv)
    repo_root = (
        Path(args.repo).resolve()
        if args.repo
        else Path(__file__).resolve().parents[2]
    )

    violations = collect(repo_root)
    if violations:
        print(f"python-utf8-contract: {len(violations)} violation(s)")
        for violation in violations:
            print(f"::error::{violation}")
        return 1

    workflows = sorted((repo_root / ".github" / "workflows").glob("*.yml"))
    modules = [
        path
        for root_name in SCANNED_ROOTS
        for path in sorted((repo_root / root_name).rglob("*.py"))
    ]
    print(
        "python-utf8-contract: OK "
        f"({len(workflows)} workflows with the UTF-8 environment, "
        f"{len(modules)} Python modules with explicit subprocess encodings)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
