#!/usr/bin/env python3
"""Validate GitHub Actions workflow files, ruleset JSON and other .github YAML.

Req D1/D2/D3 of docs/specs/feature-specs/ci-workflow-lint-and-flaky-quarantine.md.

Two layers, both fail closed:
  1. YAML/JSON parsing so a workflow that cannot be parsed (e.g. #674: an
     unquoted ``if:`` value containing ``": "``) can never merge.
  2. Optional actionlint run (``--actionlint PATH``) for semantic checks.

The actionlint binary is downloaded and checksum-verified by the workflow, so
this script never needs the network.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - CI installs pyyaml before running us
    print("ERROR: pyyaml is required (pip install pyyaml)", file=sys.stderr)
    raise SystemExit(2)

YAML_SUFFIXES = (".yml", ".yaml")


def _load_yaml(path: Path) -> tuple[dict | None, str | None]:
    """Parse a YAML file, returning (mapping, error)."""
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as exc:
        return None, f"{path}: YAML parse error: {exc}"
    except OSError as exc:
        return None, f"{path}: cannot read file: {exc}"
    if data is None:
        return None, f"{path}: file is empty"
    if not isinstance(data, dict):
        return None, f"{path}: expected a mapping at the top level"
    return data, None


def check_workflow(path: Path, errors: list[str]) -> None:
    data, error = _load_yaml(path)
    if error is not None:
        errors.append(error)
        return
    jobs = data.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        errors.append(f"{path}: workflow has no non-empty 'jobs' mapping")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".", help="repository root (default: cwd)")
    parser.add_argument(
        "--actionlint",
        default=None,
        help="path to an actionlint binary; when given its failures fail the gate",
    )
    args = parser.parse_args(argv)

    repo = Path(args.repo).resolve()
    github_dir = repo / ".github"
    workflows_dir = github_dir / "workflows"
    errors: list[str] = []

    if not workflows_dir.is_dir():
        errors.append(f"{workflows_dir}: workflows directory does not exist")
        workflows = []
    else:
        workflows = sorted(
            p for p in workflows_dir.iterdir() if p.is_file() and p.suffix in YAML_SUFFIXES
        )
        if not workflows:
            errors.append(f"{workflows_dir}: no workflow files found")

    for path in workflows:
        check_workflow(path, errors)

    rulesets = sorted((github_dir / "rulesets").glob("*.json")) if (github_dir / "rulesets").is_dir() else []
    for path in rulesets:
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            errors.append(f"{path}: JSON parse error: {exc}")
            continue
        if not isinstance(payload, dict):
            errors.append(f"{path}: expected a JSON object at the top level")

    other_yaml = sorted(
        p
        for p in github_dir.rglob("*")
        if p.is_file() and p.suffix in YAML_SUFFIXES and workflows_dir not in p.parents
    )
    for path in other_yaml:
        _data, error = _load_yaml(path)
        if error is not None:
            errors.append(error)

    if errors:
        for message in errors:
            print(f"ERROR: {message}", file=sys.stderr)
        print(f"FAILED: {len(errors)} workflow/config problem(s)", file=sys.stderr)
        return 1

    print(
        f"OK: parsed {len(workflows)} workflow(s), {len(rulesets)} ruleset(s), "
        f"{len(other_yaml)} other .github YAML file(s)"
    )

    if args.actionlint:
        # External linters (shellcheck/pyflakes) are disabled on purpose: whether
        # they are installed differs between runners, and shell style of existing
        # scripts is not what this gate is about. Workflow syntax/semantics is.
        command = [
            args.actionlint,
            "-no-color",
            "-shellcheck=",
            "-pyflakes=",
            *[str(p) for p in workflows],
        ]
        result = subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if result.stdout:
            sys.stdout.write(result.stdout)
        if result.stderr:
            sys.stderr.write(result.stderr)
        if result.returncode != 0:
            print(
                f"FAILED: actionlint reported problems in .github/workflows (exit {result.returncode})",
                file=sys.stderr,
            )
            return 1
        print(f"OK: actionlint found no problems in {len(workflows)} workflow file(s)")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
