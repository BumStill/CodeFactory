"""Trusted PR case bindings for the existing affected-scenario runner.

This is deliberately not a release or desktop verifier. Code and manifests are
read from the policy checkout; the candidate may supply only bounded evidence.
"""

from __future__ import annotations

import hashlib
import json
import platform
import re
import subprocess
import tomllib
from pathlib import Path
from typing import Any

from tools.governance.scenario_case_receipt import (
    E2E001_SCENARIO_IDS, E2E001_TARGET, ORACLE_NAMES,
    build_e2e001_case_receipt, fixture_manifest_digest, validate_case_receipt_for_gate,
)


DRIVER_FILES = (
    ".cargo/config.toml",
    "src-tauri/src/main.rs",
    "src-tauri/src/unattended_smoke_cli.rs",
    "src-tauri/src/agent/unattended_smoke.rs",
    "src-tauri/src/agent/scenario_case_observation.rs",
    "src-tauri/src/util/process_tree.rs",
    "src-tauri/build.rs",
)
VERIFIER_FILES = (
    "tools/governance/scenario_execution.py",
    "tools/governance/scenario_case_execution.py",
    "tools/governance/scenario_case_receipt.py",
)
FIXTURE_FILE = "tests/fixtures/scenarios/e2e-001/fixture-manifest.json"
LIB_PREFIX = "// SPDX-License-Identifier: Apache-2.0\npub mod unattended_smoke_cli;\n"
RUNNER_IDENTITIES = {
    "windows-latest": {"name": "windows-latest", "os": "windows", "arch": "x86_64"},
    "macos-14": {"name": "macos-14", "os": "macos", "arch": "aarch64"},
}
BOOL_FIELDS = {
    "ok", "same_objective", "process_restart_observed", "phase_one_was_hard_killed",
    "supervisor_hard_kill_issued", "worker_reaped", "replacement_process_distinct",
    "artifact_verified", "cleanup_ok", "cleanup_attempted", "orphan_sweep_performed",
}
INT_FIELDS = {
    "observation_schema_version", "user_message_count", "human_prompt_count",
    "side_effect_receipt_count", "replay_call_link_count", "descendant_process_count",
    "live_owner_count", "claimable_remediation_count", "leaked_resource_count",
}
ENUM_FIELDS = {
    "case_id": {"E2E-001"}, "scenario_id": {"HLT-001"},
    "objective_status": {"completed", "failed", "running", "parked"},
}
RAW_FIELDS = BOOL_FIELDS | INT_FIELDS | set(ENUM_FIELDS) | {"scenario_ids", "build_git_sha"}


def _read_regular(root: Path, relative: str) -> bytes:
    path = root / relative
    # Reject symlinked parents as well as the final file. The root itself may
    # be an explicitly selected checkout, but its descendants cannot redirect.
    if any(part.is_symlink() for part in [path, *path.parents] if part != root and root in part.parents):
        raise ValueError(f"protected case input is a symlink: {relative}")
    return path.read_bytes()


def bundle_digest(root: Path, files: tuple[str, ...]) -> str:
    hashes = {relative: hashlib.sha256(_read_regular(root, relative)).hexdigest()
              for relative in sorted(files)}
    return hashlib.sha256(json.dumps(hashes, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def build_case_plans(targets: set[str], runners: dict[str, list[str]], aliases: dict,
                     *, policy_root: Path, base_sha: str, head_sha: str) -> list[dict]:
    selected = [target for target in targets if aliases.get(target, target) == E2E001_TARGET]
    if not selected:
        return []
    owners = {runner for runner, entries in runners.items() if any(t in entries for t in selected)}
    if len(owners) != 1:
        raise ValueError("E2E-001 canonical target must have exactly one runner")
    runner = RUNNER_IDENTITIES[next(iter(owners))]
    manifest = json.loads(_read_regular(policy_root, FIXTURE_FILE))
    return [{
        "expectation": {
            "case_id": "E2E-001", "scenario_ids": E2E001_SCENARIO_IDS,
            "stage": "pull_request", "base_sha": base_sha, "head_sha": head_sha,
            "canonical_target": E2E001_TARGET,
            "oracle_policy": {name: "not_required_for_stage" if name == "ui" else "required"
                              for name in ORACLE_NAMES},
            "runner": runner,
            "build_identity": {
                "source_sha": head_sha, "executable_build_sha": head_sha,
                "executable_sha256": None, "artifact_sha256": None, "version": None, "tag_sha": None,
            },
            "fixture_manifest_sha256": fixture_manifest_digest(manifest),
            "driver_sha256": bundle_digest(policy_root, DRIVER_FILES),
            "verifier_sha256": bundle_digest(policy_root, VERIFIER_FILES),
        },
        "fixture_manifest": manifest,
    }]


def project_raw_observations(raw: Any) -> dict:
    """Discard free text, paths, unknown keys and invalid typed values on export."""
    raw = raw if isinstance(raw, dict) else {}
    result = {}
    for field in sorted(RAW_FIELDS):
        value = raw.get(field)
        valid = False
        if field in BOOL_FIELDS:
            valid = type(value) is bool
        elif field in INT_FIELDS:
            valid = type(value) is int and 0 <= value <= 2**64 - 1
        elif field in ENUM_FIELDS:
            valid = isinstance(value, str) and value in ENUM_FIELDS[field]
        elif field == "scenario_ids":
            valid = value == E2E001_SCENARIO_IDS
        elif field == "build_git_sha":
            valid = isinstance(value, str) and bool(re.fullmatch(r"[0-9a-f]{40}|unknown", value))
        result[field] = value if valid else None
    return result


def raw_observation_errors(raw: Any, expected: dict) -> list[str]:
    if not isinstance(raw, dict) or set(raw) != RAW_FIELDS:
        return ["case raw observation fields are invalid"]
    projected = project_raw_observations(raw)
    if any(value is None for value in projected.values()) or projected != raw:
        return ["case raw observation types or values are invalid"]
    errors = []
    if raw["observation_schema_version"] != 1:
        errors.append("case raw observation schema is invalid")
    if raw["build_git_sha"] != expected["head_sha"]:
        errors.append("case raw executable build SHA does not match the candidate head")
    if raw["phase_one_was_hard_killed"] is not True:
        errors.append("case raw hard-kill exit observation did not pass")
    return errors


def build_case_entry(raw: Any, case: dict) -> dict:
    expected = case["expectation"]
    projected = project_raw_observations(raw)
    checked = dict(projected)
    if raw_observation_errors(projected, expected):
        checked["ok"] = False
    receipt = build_e2e001_case_receipt(
        checked, expected, case["fixture_manifest"],
        runner=expected["runner"], build_identity=expected["build_identity"],
    )
    return {"case_id": expected["case_id"], "raw_observations": projected, "receipt": receipt}


def validate_case_entry(entry: Any, case: dict) -> list[str]:
    expected = case["expectation"]
    if not isinstance(entry, dict) or set(entry) != {"case_id", "raw_observations", "receipt", "runner"}:
        return ["case execution entry fields are invalid"]
    errors = raw_observation_errors(entry["raw_observations"], expected)
    if entry["runner"] != expected["runner"]["name"]:
        errors.append("case execution runner does not match the plan")
    recomputed = build_case_entry(entry["raw_observations"], case)
    try:
        matches = all(
            json.dumps(entry[field], sort_keys=True, allow_nan=False)
            == json.dumps(recomputed[field], sort_keys=True, allow_nan=False)
            for field in recomputed
        )
    except (TypeError, ValueError):
        matches = False
    if not matches:
        errors.append("case receipt does not match trusted raw evidence recomputation")
    errors.extend(validate_case_receipt_for_gate(recomputed["receipt"], expected))
    return errors


def validate_canonical_entry_points(repo: Path) -> list[str]:
    errors = []
    try:
        for relative in (".cargo/config", "src-tauri/.cargo/config", "src-tauri/.cargo/config.toml"):
            path = repo / relative
            if path.exists() or path.is_symlink():
                errors.append("case execution cannot add alternative Cargo configuration")
        if not _read_regular(repo, "src-tauri/src/lib.rs").decode().startswith(LIB_PREFIX):
            errors.append("case canonical CLI must be the first unconditional library module")
        cargo = tomllib.loads(_read_regular(repo, "src-tauri/Cargo.toml").decode())
        lib = cargo.get("lib", {})
        package = cargo.get("package", {})
        if (lib.get("name") != "codefactory_lib" or lib.get("path", "src/lib.rs") != "src/lib.rs"
                or cargo.get("bin") or package.get("build", "build.rs") != "build.rs"
                or package.get("name") != "codefactory" or package.get("autobins", True) is not True):
            errors.append("case canonical Cargo entry points cannot be redirected")
    except (OSError, ValueError, AttributeError):
        errors.append("case canonical entry points cannot be read")
    return errors


def validate_case_execution_inputs(plan: dict, repo: Path, runner: str, policy_root: Path) -> list[str]:
    cases = [case for case in plan.get("case_plans", [])
             if case["expectation"]["runner"]["name"] == runner]
    if not cases:
        return []
    errors = []
    actual_os = {"Windows": "windows", "Darwin": "macos"}.get(platform.system(), "unsupported")
    actual_arch = {"AMD64": "x86_64", "arm64": "aarch64"}.get(platform.machine(), platform.machine())
    identity = {"name": runner, "os": actual_os, "arch": actual_arch}
    source = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True)
    if source.returncode or source.stdout.strip() != plan["head_sha"]:
        errors.append("case candidate checkout does not match the planned head")
    dirty = subprocess.run(["git", "-C", str(repo), "diff", "--quiet", "HEAD", "--"], capture_output=True, text=True)
    if dirty.returncode:
        errors.append("case candidate tracked tree is dirty or cannot be verified")
    for case in cases:
        expected = case["expectation"]
        if identity != expected["runner"]:
            errors.append("case actual runner OS or architecture does not match the plan")
        try:
            for root in (policy_root, repo):
                if bundle_digest(root, DRIVER_FILES) != expected["driver_sha256"]:
                    errors.append("case canonical driver digest does not match the trusted plan")
            if bundle_digest(policy_root, VERIFIER_FILES) != expected["verifier_sha256"]:
                errors.append("case trusted verifier digest does not match the plan")
            errors.extend(validate_canonical_entry_points(repo))
        except (OSError, ValueError) as error:
            errors.append(f"case implementation binding cannot be read: {type(error).__name__}")
    return sorted(set(errors))
