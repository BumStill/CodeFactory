#!/usr/bin/env python3
"""Enforce docs/governance/rules.yml on every change — agent-agnostically.

This is the hard layer of the cross-agent harness (see
docs/governance/agent-conformance.md). It:

  1. loads + shape-validates the rules manifest,
  2. verifies every rule's `doc` and `enforcer` artifact actually exists
     (catches governance drift — a rule that claims enforcement it doesn't
     have), and
  3. runs `enforcement: check` rules — currently `design-doc-for-major`,
     which flags a major change that lands without a design doc.

Blockers exit non-zero; warnings are emitted as GitHub annotations but do not
fail the run (so a rule can ramp `warn -> error`). Output mirrors the contract
of validate_repo_governance_baseline.py.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

try:
    import yaml  # PyYAML; installed by the governance-baseline workflow.
except ImportError:  # pragma: no cover - environment guard
    print("::error::PyYAML not available; cannot read docs/governance/rules.yml")
    sys.exit(2)

REPO_ROOT = Path(__file__).resolve().parents[2]
RULES_PATH = REPO_ROOT / "docs" / "governance" / "rules.yml"

ENFORCEMENTS = {"structural", "check", "review"}
LEVELS = {"warn", "error"}

# Directories a design doc may live in (relative to repo root).
DESIGN_DOC_DIRS = (
    "docs/specs/",
    "docs/design/",
    "docs/principles/",
    "docs/governance/",
    "docs/self-evolution/",
)

# Heuristic markers for a "major" change.
MAJOR_SCHEMA_PATHS = ("migrations/", "src-tauri/src/storage/db.rs")
MAJOR_LARGE_CODE_DIFF = 12  # changed code files at/above this count → treat as major


def annotate_warning(msg: str) -> None:
    print(f"::warning::{msg}")


def blocker(msg: str) -> dict:
    return {"severity": "blocker", "message": msg}


def git(*args: str) -> str:
    try:
        return subprocess.run(
            ["git", *args], cwd=REPO_ROOT, capture_output=True, text=True, check=True
        ).stdout.strip()
    except Exception:
        return ""


def changed_files() -> list[str] | None:
    """Files changed vs the base, or None if the base can't be determined."""
    base = os.environ.get("GOVERNANCE_BASE_SHA", "").strip()
    if not base:
        # Local fallback: compare against origin/main if it exists.
        base = git("rev-parse", "--verify", "--quiet", "origin/main")
    if not base:
        return None
    out = git("diff", "--name-only", f"{base}...HEAD")
    if not out:
        return []
    return [line for line in out.splitlines() if line.strip()]


def added_files() -> set[str]:
    base = os.environ.get("GOVERNANCE_BASE_SHA", "").strip() or git(
        "rev-parse", "--verify", "--quiet", "origin/main"
    )
    if not base:
        return set()
    out = git("diff", "--name-only", "--diff-filter=A", f"{base}...HEAD")
    return {l for l in out.splitlines() if l.strip()}


def commit_messages_since_base() -> str:
    base = os.environ.get("GOVERNANCE_BASE_SHA", "").strip() or git(
        "rev-parse", "--verify", "--quiet", "origin/main"
    )
    if not base:
        return ""
    return git("log", "--format=%B", f"{base}..HEAD")


def is_code_file(p: str) -> bool:
    return (
        (p.startswith("src/") or p.startswith("src-tauri/src/"))
        and not ("/test" in p.lower() or p.endswith(".test.tsx") or p.endswith(".test.ts"))
        and p.rsplit(".", 1)[-1] in {"rs", "ts", "tsx"}
    )


def looks_major(changed: list[str], added: set[str]) -> tuple[bool, str]:
    # Schema / migration / persisted-format change.
    for p in changed:
        if any(p.startswith(m) or p == m for m in MAJOR_SCHEMA_PATHS):
            return True, f"schema/migration change ({p})"
    # A newly added Rust module (new subsystem surface).
    for p in added:
        if p.startswith("src-tauri/src/") and p.endswith(".rs") and "/test" not in p.lower():
            return True, f"new backend module ({p})"
    # Large code diff (cross-cutting).
    code_changed = [p for p in changed if is_code_file(p)]
    if len(code_changed) >= MAJOR_LARGE_CODE_DIFF:
        return True, f"{len(code_changed)} code files changed"
    return False, ""


def has_design_doc(changed: list[str]) -> bool:
    if any(p.startswith(DESIGN_DOC_DIRS) for p in changed):
        return True
    if "design-doc:" in commit_messages_since_base().lower():
        return True
    return False


def check_design_doc_for_major(level: str) -> list[dict]:
    """Returns blockers (only when level == 'error')."""
    changed = changed_files()
    if changed is None:
        print("::notice::design-doc-for-major: base unknown, skipping diff check")
        return []
    if not changed:
        return []
    major, why = looks_major(changed, added_files())
    if not major:
        return []
    if has_design_doc(changed):
        print(f"::notice::major change ({why}) ships with a design doc — OK")
        return []
    msg = (
        f"Major change detected ({why}) without a design doc under "
        f"docs/specs|design|principles. Per docs/principles/design-docs-for-major-changes.md, "
        f"land a design doc with this change (or add a 'Design-Doc:' commit trailer)."
    )
    if level == "error":
        return [blocker(msg)]
    annotate_warning(msg)
    return []


def main() -> int:
    failures: list[dict] = []

    if not RULES_PATH.exists():
        print("::error::docs/governance/rules.yml is missing")
        return 1

    try:
        data = yaml.safe_load(RULES_PATH.read_text(encoding="utf-8")) or {}
    except yaml.YAMLError as e:  # pragma: no cover
        print(f"::error::rules.yml is not valid YAML: {e}")
        return 1

    rules = data.get("rules")
    if not isinstance(rules, list) or not rules:
        print("::error::rules.yml must contain a non-empty 'rules' list")
        return 1

    seen_ids: set[str] = set()
    for i, rule in enumerate(rules):
        rid = rule.get("id", f"#{i}")
        # Shape.
        for field in ("id", "statement", "doc", "enforcement", "enforcer"):
            if not rule.get(field):
                failures.append(blocker(f"rule '{rid}': missing required field '{field}'"))
        if rid in seen_ids:
            failures.append(blocker(f"duplicate rule id '{rid}'"))
        seen_ids.add(rid)
        enf = rule.get("enforcement")
        if enf and enf not in ENFORCEMENTS:
            failures.append(blocker(f"rule '{rid}': enforcement '{enf}' not in {sorted(ENFORCEMENTS)}"))
        if enf == "check" and rule.get("level") not in LEVELS:
            failures.append(blocker(f"rule '{rid}': enforcement=check needs level in {sorted(LEVELS)}"))
        # Existence of the canonical doc + the enforcer artifact.
        for field in ("doc", "enforcer"):
            ref = rule.get(field)
            if ref and not (REPO_ROOT / ref).exists():
                failures.append(
                    blocker(f"rule '{rid}': {field} '{ref}' does not exist (governance drift)")
                )
        # Run check-kind rules.
        if enf == "check" and rid == "design-doc-for-major":
            failures.extend(check_design_doc_for_major(rule.get("level", "warn")))

    if failures:
        print(f"governance-rules: {len(failures)} blocker(s)")
        for f in failures:
            print(f"::error::{f['message']}")
        return 1

    print(f"governance-rules: OK ({len(rules)} rules, manifest consistent)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
