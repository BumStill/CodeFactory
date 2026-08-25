#!/usr/bin/env python3
"""Run the tool-independent CodeFactory scenario hard gate."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

SCRIPT_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(SCRIPT_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(SCRIPT_REPO_ROOT))

from tools.governance.validate_scenario_test_governance import (
    REQUIRED_SCENARIO_CHECKS,
    _changed_files,
    _event_body_and_title,
    _is_product_file,
    load_registry,
    scenario_impact_files,
    validate_change_contract,
    validate_gate_readiness,
    validate_impacted_execution,
    validate_registry,
)

TRUST_ROOT_FILES = (
    ".github/rulesets/main.json",
    ".github/workflows/ci.yml",
    ".github/workflows/governance-baseline.yml",
    ".github/workflows/lock-independent-desktop-acceptance.yml",
    ".github/workflows/release.yml",
    ".github/workflows/scenario-gate-policy.yml",
    ".github/workflows/scenario-gate.yml",
    "tools/governance/run_scenario_harness_gate.py",
    "tools/governance/validate_scenario_test_governance.py",
)

def _read(path: Path) -> str:
    try:
        if path.is_symlink():
            return ""
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def _required_contexts(repo_root: Path) -> tuple[set[str], list[str]]:
    path = repo_root / ".github/rulesets/main.json"
    try:
        if path.is_symlink():
            raise OSError("ruleset policy must not be a symlink")
        policy = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return set(), [f"cannot read main ruleset policy: {exc}"]
    ruleset = policy.get("ruleset") or {}
    errors: list[str] = []
    if ruleset.get("enforcement") != "active":
        errors.append("main ruleset must be active")
    if ruleset.get("bypass_actors") != []:
        errors.append("main ruleset must have no bypass actors")
    contexts: set[str] = set()
    status_rule = None
    for rule in ruleset.get("rules") or []:
        if rule.get("type") == "required_status_checks":
            status_rule = rule
            break
    parameters = (status_rule or {}).get("parameters") or {}
    if parameters.get("strict_required_status_checks_policy") is not True:
        errors.append("main ruleset required status checks must be strict")
    for item in parameters.get("required_status_checks") or []:
        if isinstance(item, dict) and isinstance(item.get("context"), str):
            contexts.add(item["context"])
    return contexts, errors


def validate_gate_surfaces(repo_root: Path, registry: dict) -> list[str]:
    """Validate the candidate's executable trust surfaces without running it."""

    errors: list[str] = []
    contexts, ruleset_errors = _required_contexts(repo_root)
    errors.extend(ruleset_errors)
    declared = set(
        (registry.get("gate_policy") or {}).get("pull_request_required_checks") or []
    )
    missing = sorted(declared - contexts)
    if missing:
        errors.append("main ruleset is missing scenario required checks: " + ", ".join(missing))
    if not REQUIRED_SCENARIO_CHECKS.issubset(contexts):
        errors.append(
            "main ruleset is missing trusted scenario contexts: "
            + ", ".join(sorted(REQUIRED_SCENARIO_CHECKS - contexts))
        )

    pr_gate = _read(repo_root / ".github/workflows/scenario-gate.yml")
    for marker in (
        "name: scenario-gate-pr",
        "types: [opened, synchronize, reopened, edited]",
        "--stage pull_request",
        "--policy-repo",
    ):
        if marker not in pr_gate:
            errors.append(f"scenario-gate.yml is missing required marker: {marker}")
    if "continue-on-error" in pr_gate:
        errors.append("scenario-gate.yml must not use continue-on-error")

    trusted = _read(repo_root / ".github/workflows/scenario-gate-policy.yml")
    for marker in (
        "pull_request_target:",
        "name: scenario-gate-policy",
        "contents: read",
        "persist-credentials: false",
        "--policy-repo",
    ):
        if marker not in trusted:
            errors.append(f"scenario-gate-policy.yml is missing required marker: {marker}")
    if "continue-on-error" in trusted:
        errors.append("scenario-gate-policy.yml must not use continue-on-error")

    governance = _read(repo_root / ".github/workflows/governance-baseline.yml")
    if "types: [opened, synchronize, reopened, edited]" not in governance:
        errors.append("governance-baseline must rerun on pull_request.edited")

    release = _read(repo_root / ".github/workflows/release.yml")
    if "scenario-gate-release" not in release or "--stage release_artifact" not in release:
        errors.append("release workflow is missing scenario-gate-release before publication")
    if (
        "Verify installed macOS release can attach to existing Chrome" not in release
        or "--browser-chrome-attach-smoke" not in release
    ):
        errors.append("release workflow is missing the RTE-003 exact-artifact gate")

    for relative in (".githooks/pre-commit", ".githooks/pre-push"):
        hook = _read(repo_root / relative)
        if "run_scenario_harness_gate.py" not in hook or "--stage local" not in hook:
            errors.append(f"{relative} does not call the canonical local scenario gate")
    return errors


def validate_trust_root_immutability(repo_root: Path, policy_root: Path) -> list[str]:
    """Prevent an ordinary PR from redefining its judge or required execution.

    The protected CI/release workflows are part of the trust root because their
    required contexts are the proof that declared targets actually ran. Letting
    a candidate rewrite those workflows would let it leave target names in
    comments or skipped steps and self-attest green.
    """

    if repo_root == policy_root:
        return []
    errors: list[str] = []
    for relative in TRUST_ROOT_FILES:
        candidate = repo_root / relative
        trusted = policy_root / relative
        try:
            matches = (
                not candidate.is_symlink()
                and not trusted.is_symlink()
                and candidate.read_bytes() == trusted.read_bytes()
            )
        except OSError:
            matches = False
        if not matches:
            errors.append(
                f"trusted scenario gate root cannot self-modify in a normal PR: {relative}; "
                "use an external governance bootstrap"
            )
    return errors


def _resolve_local_base(repo_root: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "origin/main"],
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else ""


def is_initial_trust_bootstrap(repo_root: Path, base_ref: str) -> bool:
    """Allow the one PR that installs a gate absent from its trusted base.

    Once the policy workflow exists on the base branch this can never return
    true again. The base-owned pull_request_target gate then byte-compares the
    complete trust root before considering candidate results.
    """

    if not base_ref or not all((repo_root / relative).is_file() for relative in TRUST_ROOT_FILES):
        return False
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repo_root),
            "cat-file",
            "-e",
            f"{base_ref}:.github/workflows/scenario-gate-policy.yml",
        ],
        capture_output=True,
        text=True,
    )
    return result.returncode != 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", choices=("local", "pull_request", "release_artifact"), required=True)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--policy-repo", required=True)
    parser.add_argument("--base-ref")
    parser.add_argument("--event-path")
    parser.add_argument("--title")
    parser.add_argument("--body-file")
    args = parser.parse_args()

    repo_root = Path(args.repo).resolve()
    policy_root = Path(args.policy_repo).resolve()
    errors: list[str] = []
    expected_runner = policy_root / "tools/governance/run_scenario_harness_gate.py"
    if not expected_runner.is_file():
        errors.append(f"trusted policy runner is missing: {expected_runner}")

    try:
        registry = load_registry(repo_root / "docs/testing/scenario-registry.json")
    except ValueError as exc:
        registry = {}
        errors.append(str(exc))
    if registry:
        errors.extend(validate_registry(registry, repo_root))
        errors.extend(validate_gate_surfaces(repo_root, registry))
    errors.extend(validate_trust_root_immutability(repo_root, policy_root))

    if args.stage == "release_artifact":
        if registry:
            errors.extend(validate_gate_readiness(registry, "release_artifact"))
    else:
        base_ref = args.base_ref or os.environ.get("SCENARIO_TEST_BASE_SHA", "").strip()
        if args.stage == "local" and not base_ref:
            base_ref = _resolve_local_base(repo_root)
        files, diff_error = _changed_files(repo_root, base_ref)
        if diff_error:
            errors.append(f"scenario gate failed closed: {diff_error}")
        elif registry:
            impact_files = scenario_impact_files(repo_root, base_ref, files)
            product_files = [path for path in impact_files if _is_product_file(path)]
            if args.stage == "pull_request":
                if args.body_file:
                    body = Path(args.body_file).read_text(encoding="utf-8")
                    title = args.title or ""
                else:
                    body, title = _event_body_and_title(
                        args.event_path
                        or os.environ.get("SCENARIO_TEST_EVENT_PATH")
                        or os.environ.get("GITHUB_EVENT_PATH")
                    )
                errors.extend(
                    validate_change_contract(title, body, impact_files, registry)
                )
            initial_bootstrap = (
                repo_root == policy_root
                and is_initial_trust_bootstrap(repo_root, base_ref)
            )
            if product_files and not initial_bootstrap:
                # Per-change enforcement: what this change touches must be
                # covered by automation that really runs. The catalog-wide
                # readiness sweep stays on release_artifact, where unpaid L4
                # debt genuinely must block the artifact — running it here
                # would instead demand every catalog gap be closed before any
                # product change could land at all.
                errors.extend(
                    validate_impacted_execution(
                        registry, impact_files, "pull_request", repo_root
                    )
                )

    if errors:
        print(f"scenario-harness-gate: {len(errors)} error(s)")
        for error in errors:
            print(f"::error::{error}")
        return 1
    print(f"scenario-harness-gate: OK ({args.stage}, tool-independent policy)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
