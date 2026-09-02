#!/usr/bin/env python3
"""Validate CodeFactory's scenario registry and PR scenario coverage contract."""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
REGISTRY_PATH = REPO_ROOT / "docs" / "testing" / "scenario-registry.json"

ALLOWED_PRIORITIES = {"P0", "P1", "P2"}
ALLOWED_GATES = {"pull_request", "nightly", "release_artifact", "manual_canary"}
ALLOWED_SOURCE_KINDS = {"anonymized_history_shape", "product_contract", "incident_shape"}
ALLOWED_AUTOMATION_STATUS = {"designed", "partially_implemented", "implemented"}
ALLOWED_PR_GATE_STATUS = {"designed", "partially_implemented", "implemented"}
REQUIRED_ORACLES = {"ui", "durable_state", "process", "side_effects", "delivery"}
FORBIDDEN_DATA_KEYS = {
    "session_id",
    "objective_id",
    "content",
    "local_user_path",
    "credential",
    "tool_arguments",
    "raw_tool_arguments",
}
DECLARATION = re.compile(
    r"^[ \t]*Scenario-Test:[ \t]*(.+?)[ \t]*$", re.IGNORECASE | re.MULTILINE
)
FENCED = re.compile(r"```.*?```", re.DOTALL)
PRODUCT_PREFIXES = (
    "src/",
    "src-tauri/src/",
    "src-tauri/crates/",
    "src-tauri/migrations/",
    "extension/",
    "tools/agent/",
    "scripts/",
)
NON_PRODUCT_MARKERS = (".test.", ".spec.", "/tests/", "src/acceptance/")
REQUIRED_SCENARIO_CHECKS = {
    "scenario-gate-pr",
}
TARGET_CHECKS = {
    "binary": "check-rust",
    "pnpm": "check-frontend",
    "path": "check-frontend",
    "rust": "check-rust",
    "workflow": "check-rust",
}
GLOBAL_PRODUCT_FILES = {
    "package.json",
    "pnpm-lock.yaml",
    "src-tauri/Cargo.lock",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
}
EXPLICIT_PRODUCT_FILES = {
    ".github/workflows/auto-release.yml",
    ".github/workflows/release.yml",
}
VERSION_MANIFEST_FILES = {
    "package.json",
    "src-tauri/Cargo.lock",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
}
SEMVER = r"([0-9]+\.[0-9]+\.[0-9]+)"
VERSION_LINE_PATTERNS = {
    "package.json": re.compile(rf'^\s*"version"\s*:\s*"{SEMVER}"[,]?\s*$'),
    "src-tauri/Cargo.lock": re.compile(rf'^\s*version\s*=\s*"{SEMVER}"\s*$'),
    "src-tauri/Cargo.toml": re.compile(rf'^\s*version\s*=\s*"{SEMVER}"\s*$'),
    "src-tauri/tauri.conf.json": re.compile(
        rf'^\s*"version"\s*:\s*"{SEMVER}"[,]?\s*$'
    ),
}


def _is_test_harness_file(path: str) -> bool:
    return (
        path.startswith("src/acceptance/")
        or (
            path.startswith("scripts/verify-")
            and path.endswith("-headless.mjs")
        )
        or any(marker in path for marker in NON_PRODUCT_MARKERS)
    )


def _is_version_manifest_only_patch(files: list[str], patch: str) -> bool:
    """Accept only the release bot's synchronized four-file version bump.

    Paths alone are insufficient: dependency, script, Cargo, and Tauri config
    changes are global product changes.  The exemption applies only when every
    changed line is the same semantic-version transition in all four manifests.
    """

    if set(files) != VERSION_MANIFEST_FILES or len(files) != len(VERSION_MANIFEST_FILES):
        return False

    removed: dict[str, list[str]] = {path: [] for path in VERSION_MANIFEST_FILES}
    added: dict[str, list[str]] = {path: [] for path in VERSION_MANIFEST_FILES}
    current: str | None = None
    for line in patch.splitlines():
        if line.startswith("diff --git a/"):
            parts = line.split(" ")
            current = parts[2][2:] if len(parts) >= 4 else None
            if current not in VERSION_MANIFEST_FILES:
                return False
            continue
        if current is None or line.startswith(("--- ", "+++ ")):
            continue
        if not line.startswith(("-", "+")):
            continue
        pattern = VERSION_LINE_PATTERNS[current]
        match = pattern.fullmatch(line[1:])
        if not match:
            return False
        target = removed if line.startswith("-") else added
        target[current].append(match.group(1))

    if any(len(removed[path]) != 1 or len(added[path]) != 1 for path in VERSION_MANIFEST_FILES):
        return False
    old_versions = {values[0] for values in removed.values()}
    new_versions = {values[0] for values in added.values()}
    return len(old_versions) == 1 and len(new_versions) == 1 and old_versions != new_versions


def scenario_impact_files(repo_root: Path, base_ref: str, files: list[str]) -> list[str]:
    """Return files that may affect scenarios, exempting an exact version bump."""

    if set(files) != VERSION_MANIFEST_FILES:
        return files
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repo_root),
            "diff",
            "--unified=0",
            f"{base_ref}...HEAD",
            "--",
            *sorted(VERSION_MANIFEST_FILES),
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return files
    return [] if _is_version_manifest_only_patch(files, result.stdout) else files


def load_registry(path: Path = REGISTRY_PATH) -> dict[str, Any]:
    if path.is_symlink():
        raise ValueError(f"scenario registry must not be a symlink: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"scenario registry is missing: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid scenario registry JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise ValueError("scenario registry root must be an object")
    return payload


def _duplicates(values: list[str]) -> set[str]:
    seen: set[str] = set()
    duplicates: set[str] = set()
    for value in values:
        if value in seen:
            duplicates.add(value)
        seen.add(value)
    return duplicates


def _files_with_suffix(root: Path, suffix: str) -> list[Path]:
    return [
        path
        for path in root.rglob(f"*{suffix}")
        if path.is_file() and not path.is_symlink()
    ]


def _automation_exists(target: str, repo_root: Path) -> bool:
    if ":" not in target:
        return False
    kind, marker = target.split(":", 1)
    if not marker.strip():
        return False
    if kind == "pnpm":
        try:
            package = json.loads((repo_root / "package.json").read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return False
        return marker in package.get("scripts", {})
    if kind == "path":
        path = repo_root / marker
        return path.is_file() and not path.is_symlink()
    if kind == "rust":
        # Must be a real test function, not just the name appearing somewhere.
        # Substring matching would keep a declaration "valid" after its test was
        # renamed or deleted, as long as the name survived in a comment, a
        # string literal, or a failure code — which is precisely how a scenario
        # goes on reporting coverage it no longer has.
        roots = [repo_root / "src-tauri" / "src", repo_root / "src-tauri" / "crates"]
        pattern = re.compile(
            r"#\[(?:tokio::)?test(?:\([^\]]*\))?\]"
            r"(?:\s*#\[[^\]]+\])*\s*(?:async\s+)?fn\s+"
            + re.escape(marker)
            + r"\s*\(",
            re.MULTILINE,
        )
        return any(
            pattern.search(file.read_text(encoding="utf-8", errors="ignore"))
            for root in roots
            if root.exists()
            for file in _files_with_suffix(root, ".rs")
        )
    if kind == "workflow":
        files = _files_with_suffix(repo_root / ".github" / "workflows", ".yml")
    elif kind == "binary":
        files = _files_with_suffix(repo_root / "src-tauri", ".rs")
    else:
        return False
    return any(marker in path.read_text(encoding="utf-8", errors="ignore") for path in files)


def _automation_gate_stages(
    target: str, gate_policy: dict[str, Any], repo_root: Path
) -> set[str]:
    if ":" not in target:
        return set()
    kind, marker = target.split(":", 1)
    binding = (gate_policy.get("target_bindings") or {}).get(kind)
    if not isinstance(binding, dict):
        return set()
    configured_paths: list[str] = []
    if isinstance(binding.get("workflow"), str):
        configured_paths.append(binding["workflow"])
    if isinstance(binding.get("workflows"), list):
        configured_paths.extend(binding["workflows"])
    workflow_texts: dict[str, str] = {}
    for relative in configured_paths:
        path = repo_root / relative
        if path.is_file():
            workflow_texts[relative] = path.read_text(encoding="utf-8", errors="ignore")
    if not workflow_texts:
        return set()
    if kind == "pnpm":
        if any(f"pnpm {marker}" in text for text in workflow_texts.values()):
            return set(binding.get("stages") or [])
        return set()
    if kind in {"binary", "workflow"}:
        stages: set[str] = set()
        workflow_stages = binding.get("workflow_stages") or {}
        delegated_scripts = binding.get("delegated_scripts") or {}
        for relative, text in workflow_texts.items():
            if marker in text:
                stages.update(workflow_stages.get(relative) or [])
                continue
            scripts = delegated_scripts.get(relative) or []
            for script in scripts:
                script_relative = Path(script) if isinstance(script, str) else None
                if (
                    script_relative is None
                    or script_relative.is_absolute()
                    or ".." in script_relative.parts
                    or not script.startswith("scripts/")
                ):
                    continue
                script_path = repo_root / script
                if (
                    script in text
                    and script_path.is_file()
                    and not script_path.is_symlink()
                    and marker
                    in script_path.read_text(encoding="utf-8", errors="ignore")
                ):
                    stages.update(workflow_stages.get(relative) or [])
        return stages
    command = binding.get("command")
    job = binding.get("job")
    if bool(
        isinstance(command, str)
        and isinstance(job, str)
        and any(command in text and f"  {job}:" in text for text in workflow_texts.values())
    ):
        return set(binding.get("stages") or [])
    return set()


def _automation_is_hard_gated(
    target: str, gate_policy: dict[str, Any], repo_root: Path
) -> bool:
    return bool(_automation_gate_stages(target, gate_policy, repo_root))


def _forbidden_keys(value: Any, location: str = "registry") -> list[str]:
    errors: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            child_location = f"{location}.{key}"
            if key.lower() in FORBIDDEN_DATA_KEYS:
                errors.append(f"forbidden production data key at {child_location}")
            errors.extend(_forbidden_keys(child, child_location))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            errors.extend(_forbidden_keys(child, f"{location}[{index}]"))
    return errors


def validate_registry(registry: dict[str, Any], repo_root: Path = REPO_ROOT) -> list[str]:
    errors: list[str] = []
    if registry.get("schema_version") != 2:
        errors.append("schema_version must be 2")
    if registry.get("source_policy") != "aggregate-shapes-only":
        errors.append("source_policy must be aggregate-shapes-only")
    gate_policy = registry.get("gate_policy")
    if not isinstance(gate_policy, dict):
        errors.append("gate_policy must be an object")
        gate_policy = {}
    if gate_policy.get("mode") != "all-registered-targets-hard-gated":
        errors.append("gate_policy.mode must be all-registered-targets-hard-gated")
    required_checks = set(gate_policy.get("pull_request_required_checks") or [])
    if not REQUIRED_SCENARIO_CHECKS.issubset(required_checks):
        errors.append(
            "gate_policy.pull_request_required_checks must include "
            + ", ".join(sorted(REQUIRED_SCENARIO_CHECKS))
        )
    if gate_policy.get("release_required_job") != "scenario-gate-release":
        errors.append("gate_policy.release_required_job must be scenario-gate-release")
    if set((gate_policy.get("target_bindings") or {}).keys()) != set(TARGET_CHECKS):
        errors.append(
            "gate_policy.target_bindings must cover exactly: "
            + ", ".join(sorted(TARGET_CHECKS))
        )
    for kind, binding in (gate_policy.get("target_bindings") or {}).items():
        if not isinstance(binding, dict):
            errors.append(f"gate_policy.target_bindings.{kind} must be an object")
            continue
        declared_stages = set(binding.get("stages") or [])
        workflow_stages = binding.get("workflow_stages") or {}
        if not isinstance(workflow_stages, dict):
            errors.append(f"gate_policy.target_bindings.{kind}.workflow_stages must be an object")
            workflow_stages = {}
        for stages in workflow_stages.values():
            declared_stages.update(stages or [])
        delegated_scripts = binding.get("delegated_scripts") or {}
        if not isinstance(delegated_scripts, dict):
            errors.append(
                f"gate_policy.target_bindings.{kind}.delegated_scripts must be an object"
            )
        else:
            configured_workflows = set(binding.get("workflows") or [])
            workflow = binding.get("workflow")
            if isinstance(workflow, str):
                configured_workflows.add(workflow)
            for workflow_path, scripts in delegated_scripts.items():
                if workflow_path not in configured_workflows:
                    errors.append(
                        f"gate_policy.target_bindings.{kind}.delegated_scripts names an unconfigured workflow: {workflow_path}"
                    )
                if not isinstance(scripts, list) or not scripts:
                    errors.append(
                        f"gate_policy.target_bindings.{kind}.delegated_scripts.{workflow_path} must be a non-empty list"
                    )
                    continue
                for script in scripts:
                    script_path = Path(script) if isinstance(script, str) else None
                    if (
                        script_path is None
                        or script_path.is_absolute()
                        or ".." in script_path.parts
                        or not script.startswith("scripts/")
                    ):
                        errors.append(
                            f"gate_policy.target_bindings.{kind}.delegated_scripts contains an unsafe path: {script}"
                        )
        unknown_stages = sorted(declared_stages - ALLOWED_GATES)
        if unknown_stages:
            errors.append(
                f"gate_policy.target_bindings.{kind} has unknown stages: "
                + ", ".join(unknown_stages)
            )

    categories = registry.get("categories")
    if not isinstance(categories, list) or not categories:
        errors.append("categories must be a non-empty list")
        categories = []
    category_ids = [item.get("id", "") for item in categories if isinstance(item, dict)]
    for duplicate in sorted(_duplicates(category_ids)):
        errors.append(f"duplicate category id: {duplicate}")
    allowed_categories = set(category_ids)

    evidence_levels = registry.get("evidence_levels")
    if not isinstance(evidence_levels, list) or not evidence_levels:
        errors.append("evidence_levels must be a non-empty list")
        evidence_levels = []
    evidence_ids = [item.get("id", "") for item in evidence_levels if isinstance(item, dict)]
    for duplicate in sorted(_duplicates(evidence_ids)):
        errors.append(f"duplicate evidence level: {duplicate}")
    allowed_evidence = set(evidence_ids)

    scenarios = registry.get("scenarios")
    if not isinstance(scenarios, list) or not scenarios:
        errors.append("scenarios must be a non-empty list")
        scenarios = []
    scenario_ids = [item.get("id", "") for item in scenarios if isinstance(item, dict)]
    for duplicate in sorted(_duplicates(scenario_ids)):
        errors.append(f"duplicate scenario id: {duplicate}")
    known_scenarios = set(scenario_ids)

    required_fields = {
        "id",
        "name",
        "category",
        "priority",
        "source_kind",
        "change_patterns",
        "automated_by",
        "required_evidence",
        "gates",
    }
    for index, scenario in enumerate(scenarios):
        if not isinstance(scenario, dict):
            errors.append(f"scenario at index {index} must be an object")
            continue
        scenario_id = scenario.get("id") or f"index-{index}"
        missing = sorted(required_fields - scenario.keys())
        if missing:
            errors.append(f"{scenario_id} missing fields: {', '.join(missing)}")
        if scenario.get("category") not in allowed_categories:
            errors.append(f"{scenario_id} has unknown category: {scenario.get('category')}")
        if scenario.get("priority") not in ALLOWED_PRIORITIES:
            errors.append(f"{scenario_id} has invalid priority: {scenario.get('priority')}")
        if scenario.get("source_kind") not in ALLOWED_SOURCE_KINDS:
            errors.append(f"{scenario_id} has invalid source_kind: {scenario.get('source_kind')}")
        patterns = scenario.get("change_patterns")
        if not isinstance(patterns, list) or not patterns or not all(
            isinstance(item, str) and item.strip() for item in patterns
        ):
            errors.append(f"{scenario_id} must define non-empty change_patterns")
        automation = scenario.get("automated_by")
        scenario_gates = set(scenario.get("gates") or [])
        if not isinstance(automation, list) or not automation:
            errors.append(f"{scenario_id} must name at least one automation target")
        else:
            for target in automation:
                if not isinstance(target, str) or not _automation_exists(target, repo_root):
                    errors.append(f"{scenario_id} points at missing automation: {target}")
                elif not _automation_is_hard_gated(target, gate_policy, repo_root):
                    errors.append(
                        f"{scenario_id} automation is not bound to a hard gate: {target}"
                    )
                elif not (_automation_gate_stages(target, gate_policy, repo_root) & scenario_gates):
                    errors.append(
                        f"{scenario_id} automation has no declared hard-gate stage: {target}"
                    )
        required_evidence = scenario.get("required_evidence")
        if not isinstance(required_evidence, list) or not required_evidence:
            errors.append(f"{scenario_id} must define required_evidence")
        else:
            unknown = sorted(set(required_evidence) - allowed_evidence)
            if unknown:
                errors.append(f"{scenario_id} has unknown evidence levels: {', '.join(unknown)}")
        gates = scenario.get("gates")
        if not isinstance(gates, list) or not gates:
            errors.append(f"{scenario_id} must define gates")
        else:
            unknown_gates = sorted(set(gates) - ALLOWED_GATES)
            if unknown_gates:
                errors.append(f"{scenario_id} has unknown gates: {', '.join(unknown_gates)}")
            if "release_artifact" in gates and "L4" not in set(required_evidence or []):
                errors.append(f"{scenario_id} release_artifact gate requires L4 evidence")
            if "pull_request" not in gates:
                errors.append(f"{scenario_id} must include pull_request hard gate")

    cases = registry.get("complex_e2e_cases")
    if not isinstance(cases, list) or not cases:
        errors.append("complex_e2e_cases must be a non-empty list")
        cases = []
    case_ids = [item.get("id", "") for item in cases if isinstance(item, dict)]
    for duplicate in sorted(_duplicates(case_ids)):
        errors.append(f"duplicate complex e2e id: {duplicate}")

    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            errors.append(f"complex e2e at index {index} must be an object")
            continue
        case_id = case.get("id") or f"index-{index}"
        if case.get("priority") not in ALLOWED_PRIORITIES:
            errors.append(f"{case_id} has invalid priority: {case.get('priority')}")
        covers = case.get("covers")
        if not isinstance(covers, list) or not covers:
            errors.append(f"{case_id} must cover at least one registered scenario")
        else:
            unknown = sorted(set(covers) - known_scenarios)
            if unknown:
                errors.append(f"{case_id} covers unknown scenarios: {', '.join(unknown)}")
        case_patterns = case.get("change_patterns")
        if not isinstance(case_patterns, list) or not case_patterns or not all(
            isinstance(item, str) and item.strip() for item in case_patterns
        ):
            errors.append(f"{case_id} must define non-empty change_patterns")
        fixture = case.get("fixture")
        if not isinstance(fixture, dict) or fixture.get("synthetic") is not True:
            errors.append(f"{case_id} fixture must be synthetic")
        steps = case.get("steps")
        if not isinstance(steps, list) or len(steps) < 4:
            errors.append(f"{case_id} must define at least four end-to-end steps")
        faults = case.get("fault_injection")
        if not isinstance(faults, list) or not faults:
            errors.append(f"{case_id} must define fault_injection")
        oracles = case.get("oracles")
        if not isinstance(oracles, dict):
            errors.append(f"{case_id} must define cross-layer oracles")
        else:
            for oracle in sorted(REQUIRED_ORACLES):
                values = oracles.get(oracle)
                if not isinstance(values, list) or not values:
                    errors.append(f"{case_id} missing required oracle: {oracle}")
        execution = case.get("execution")
        if not isinstance(execution, dict) or not execution or not set(execution).issubset(ALLOWED_GATES):
            errors.append(f"{case_id} has invalid execution gates")
        if case.get("automation_status") not in ALLOWED_AUTOMATION_STATUS:
            errors.append(f"{case_id} has invalid automation_status")
        # `automation_status` must match what the case actually automates.
        # Forcing every case to declare automation would just invite fake
        # declarations; the honest bar is that a case may not *claim* more than
        # it runs, and an incomplete case must say what is still missing so the
        # hole is visible in review instead of reading as coverage.
        automated = case.get("automated_by")
        automated = automated if isinstance(automated, list) else []
        for target in automated:
            if not _automation_exists(target, repo_root):
                errors.append(f"{case_id} points at missing automation: {target}")
            elif not _automation_is_hard_gated(target, gate_policy, repo_root):
                errors.append(
                    f"{case_id} automation is not bound to a hard gate: {target}"
                )
            elif not (
                _automation_gate_stages(target, gate_policy, repo_root)
                & set(execution or {})
            ):
                errors.append(
                    f"{case_id} automation has no declared hard-gate stage: {target}"
                )
        gaps = case.get("remaining_gaps")
        gaps = gaps if isinstance(gaps, list) else []
        status = case.get("automation_status")
        if status == "implemented":
            if not automated:
                errors.append(f"{case_id} must name at least one automation target")
            if gaps:
                errors.append(
                    f"{case_id} is implemented but still records remaining_gaps"
                )
        elif status == "partially_implemented":
            if not automated:
                errors.append(f"{case_id} must name at least one automation target")
            if not gaps:
                errors.append(
                    f"{case_id} is partially_implemented and must record remaining_gaps"
                )
        elif status == "designed":
            if automated:
                errors.append(
                    f"{case_id} is only designed but already names automation"
                )
            if not gaps:
                errors.append(f"{case_id} is designed and must record remaining_gaps")

        pr_gate = case.get("pull_request_gate")
        has_pr_execution = "pull_request" in (execution or {})
        if has_pr_execution and not isinstance(pr_gate, dict):
            errors.append(f"{case_id} pull_request execution requires pull_request_gate")
        elif not has_pr_execution and pr_gate is not None:
            errors.append(f"{case_id} has pull_request_gate without pull_request execution")
        elif isinstance(pr_gate, dict):
            pr_status = pr_gate.get("status")
            pr_targets = pr_gate.get("required_targets")
            pr_gaps = pr_gate.get("remaining_gaps")
            if pr_status not in ALLOWED_PR_GATE_STATUS:
                errors.append(f"{case_id} has invalid pull_request_gate status: {pr_status}")
            if not isinstance(pr_targets, list) or not all(
                isinstance(item, str) and item.strip() for item in pr_targets
            ):
                errors.append(f"{case_id} pull_request_gate required_targets must be a list")
                pr_targets = []
            if not isinstance(pr_gaps, list) or not all(
                isinstance(item, str) and item.strip() for item in pr_gaps
            ):
                errors.append(f"{case_id} pull_request_gate remaining_gaps must be a list")
                pr_gaps = []
            if pr_status == "implemented":
                if not pr_targets:
                    errors.append(
                        f"{case_id} implemented pull_request_gate must name required_targets"
                    )
                if pr_gaps:
                    errors.append(
                        f"{case_id} implemented pull_request_gate still has PR gaps"
                    )
            elif pr_status in {"designed", "partially_implemented"} and not pr_gaps:
                errors.append(
                    f"{case_id} incomplete pull_request_gate must record PR gaps"
                )
            unknown_targets = sorted(set(pr_targets) - set(automated))
            if unknown_targets:
                errors.append(
                    f"{case_id} pull_request_gate requires undeclared automation: "
                    + ", ".join(unknown_targets)
                )
            for target in pr_targets:
                if not _automation_exists(target, repo_root):
                    errors.append(
                        f"{case_id} pull_request_gate points at missing automation: {target}"
                    )
                elif "pull_request" not in _automation_gate_stages(
                    target, gate_policy, repo_root
                ):
                    errors.append(
                        f"{case_id} pull_request_gate target does not run at pull_request: {target}"
                    )
        if case.get("must_remain_unattended") is True:
            serialized = json.dumps(oracles, ensure_ascii=False)
            for marker in ("user message 总数为 1", "human prompt 总数为 0"):
                if marker not in serialized:
                    errors.append(f"{case_id} unattended oracle missing: {marker}")

    execution_policy = registry.get("execution_policy")
    if not isinstance(execution_policy, dict):
        errors.append("execution_policy must be an object")
        execution_policy = {}
    if execution_policy.get("workflow") != ".github/workflows/scenario-execution.yml":
        errors.append("execution_policy.workflow must be scenario-execution.yml")
    if execution_policy.get("receipt_schema_version") != 1:
        errors.append("execution_policy.receipt_schema_version must be 1")
    supported_runners = {"windows-latest", "macos-14"}
    if execution_policy.get("default_runner") not in supported_runners:
        errors.append("execution_policy.default_runner must be a supported runner")
    all_targets = {
        target
        for scenario in scenarios
        for target in scenario.get("automated_by") or []
        if isinstance(target, str)
    } | {
        target
        for case in cases
        for target in case.get("automated_by") or []
        if isinstance(target, str)
    }
    excluded_targets = set(execution_policy.get("pull_request_excluded_targets") or [])
    unknown_excluded = sorted(excluded_targets - all_targets)
    if unknown_excluded:
        errors.append(
            "execution_policy excludes unknown pull_request targets: "
            + ", ".join(unknown_excluded)
        )
    runner_overrides = execution_policy.get("target_runners") or {}
    if not isinstance(runner_overrides, dict):
        errors.append("execution_policy.target_runners must be an object")
        runner_overrides = {}
    for target, runner in runner_overrides.items():
        if target not in all_targets:
            errors.append(f"execution_policy target runner names unknown target: {target}")
        if runner not in supported_runners:
            errors.append(f"execution_policy target runner is unsupported: {target}")
    aliases = execution_policy.get("workflow_aliases") or {}
    if not isinstance(aliases, dict):
        errors.append("execution_policy.workflow_aliases must be an object")
        aliases = {}
    workflow_commands = execution_policy.get("workflow_commands") or {}
    if not isinstance(workflow_commands, dict):
        errors.append("execution_policy.workflow_commands must be an object")
        workflow_commands = {}
    workflow_targets = {
        target
        for target in all_targets
        if target.startswith("workflow:") and target not in excluded_targets
    }
    for target in sorted(workflow_targets - set(aliases) - set(workflow_commands)):
        errors.append(f"execution_policy is missing workflow alias: {target}")
    for target, concrete in aliases.items():
        if target not in workflow_targets:
            errors.append(f"execution_policy names unknown workflow target: {target}")
        if not isinstance(concrete, str) or not _automation_exists(concrete, repo_root):
            errors.append(f"execution_policy workflow alias is not executable: {target}")
    for target, command in workflow_commands.items():
        valid = (
            target in workflow_targets
            and isinstance(command, list)
            and len(command) == 2
            and command[0] == "node"
            and isinstance(command[1], str)
            and command[1].startswith("scripts/")
            and ".." not in Path(command[1]).parts
            and (repo_root / command[1]).is_file()
        )
        if not valid:
            errors.append(f"execution_policy workflow command is invalid: {target}")
    binary_oracles = execution_policy.get("binary_receipt_oracles") or {}
    if not isinstance(binary_oracles, dict):
        errors.append("execution_policy.binary_receipt_oracles must be an object")
        binary_oracles = {}
    binary_flags = {
        target.split(":", 1)[1]
        for target in all_targets
        if target.startswith("binary:") and target not in excluded_targets
    }
    for flag in sorted(binary_flags - set(binary_oracles)):
        errors.append(f"execution_policy is missing binary receipt oracle: {flag}")
    for flag, oracle in binary_oracles.items():
        if flag not in binary_flags:
            errors.append(f"execution_policy names unknown binary receipt oracle: {flag}")
        if (
            not isinstance(oracle, dict)
            or not isinstance(oracle.get("field"), str)
            or "equals" not in oracle
        ):
            errors.append(f"execution_policy binary receipt oracle is invalid: {flag}")

    privacy = registry.get("privacy")
    if not isinstance(privacy, dict) or not privacy.get("forbidden"):
        errors.append("privacy.forbidden must be declared")
    errors.extend(_forbidden_keys({key: value for key, value in registry.items() if key != "privacy"}))
    return errors


def validate_gate_readiness(registry: dict[str, Any], stage: str) -> list[str]:
    """Reject catalog entries that cannot act as a real hard gate at *stage*.

    The base registry validator intentionally permits honest test debt to be
    recorded. This stricter compiler is what a required PR/release gate calls:
    debt remains visible, but it cannot be counted as a passing gate.
    """

    if stage not in {"pull_request", "release_artifact"}:
        return [f"unknown scenario gate stage: {stage}"]

    errors: list[str] = []
    for scenario in registry.get("scenarios", []):
        scenario_id = scenario.get("id", "unknown")
        gates = set(scenario.get("gates") or [])
        if stage == "pull_request" and "pull_request" not in gates:
            if gates == {"manual_canary"}:
                errors.append(
                    f"{scenario_id} manual_canary is not a hard gate; add pull_request"
                )
            else:
                errors.append(f"{scenario_id} is missing pull_request hard gate")
        if stage == "release_artifact" and "release_artifact" in gates:
            evidence = set(scenario.get("required_evidence") or [])
            if "L4" not in evidence:
                errors.append(
                    f"{scenario_id} release_artifact gate is missing L4 evidence"
                )

    for case in registry.get("complex_e2e_cases", []):
        execution = case.get("execution") or {}
        if stage not in execution:
            continue
        case_id = case.get("id", "unknown")
        status = case.get("automation_status")
        gaps = case.get("remaining_gaps") or []
        if status != "implemented" or gaps:
            errors.append(
                f"{case_id} is not gate-ready for {stage}: "
                f"status={status}, remaining_gaps={len(gaps)}"
            )
    return errors


def validate_impacted_execution(
    registry: dict[str, Any],
    files: list[str],
    stage: str,
    repo_root: Path,
    *,
    expand_global_files: bool = True,
    fail_on_unmapped: bool = False,
) -> list[str]:
    """Require the scenarios a change touches to be covered by automation that
    really runs at *stage*.

    This is the per-change gate. It deliberately does not audit the whole
    catalog: unrelated test debt stays visible in ``remaining_gaps`` without
    blocking work that cannot re-break it. What it will not tolerate is a
    change landing next to a scenario whose declared automation never
    executes — declaring a target and never running it is what let fixed
    scenarios quietly regress. ``_automation_gate_stages`` resolves each
    target against the workflow files, so a declaration alone is not enough.

    The release caller applies the same rule to the diff since the previous
    published tag with global-file expansion disabled. This keeps exact-artifact
    proof strict for the affected batch without turning unrelated catalog debt
    into a repository-wide release freeze.
    """

    product_files = [path for path in files if _is_product_file(path)]
    if not product_files:
        return []
    impacted = _impacted_scenarios(
        product_files,
        registry,
        expand_global_files=expand_global_files,
    )
    if not impacted:
        if fail_on_unmapped:
            return [
                "scenario registry coverage gap for product files: "
                + ", ".join(sorted(product_files))
            ]
        # validate_change_contract already fails closed on unmapped PR product
        # files; do not report the same gap twice.
        return []

    gate_policy = registry.get("gate_policy") or {}
    errors: list[str] = []

    for scenario in registry.get("scenarios", []):
        if scenario.get("id") not in impacted:
            continue
        if stage not in set(scenario.get("gates") or []):
            continue
        running = [
            target
            for target in scenario.get("automated_by") or []
            if stage in _automation_gate_stages(target, gate_policy, repo_root)
        ]
        if not running:
            errors.append(
                f"{scenario.get('id')} is impacted by this change but no declared "
                f"target actually runs at {stage}"
            )

    for case in registry.get("complex_e2e_cases", []):
        if stage not in (case.get("execution") or {}):
            continue
        patterns = case.get("change_patterns") or []
        directly_impacted = any(
            fnmatch.fnmatch(path, pattern)
            for path in product_files
            for pattern in patterns
        ) or (
            expand_global_files
            and any(path in GLOBAL_PRODUCT_FILES for path in product_files)
        )
        if not directly_impacted:
            continue
        if stage == "pull_request":
            pr_gate = case.get("pull_request_gate") or {}
            status = pr_gate.get("status")
            gaps = pr_gate.get("remaining_gaps") or []
            if status != "implemented" or gaps:
                errors.append(
                    f"{case.get('id')} is impacted by this change but its pull_request "
                    f"E2E slice is not implemented (status={status}, "
                    f"remaining_gaps={len(gaps)})"
                )
                continue
            required_targets = pr_gate.get("required_targets") or []
        else:
            # A release slice is complete only when at least one declared
            # target is physically bound to the release workflow. PR/nightly
            # targets remain useful evidence but are not exact-artifact proof.
            required_targets = [
                target
                for target in case.get("automated_by") or []
                if stage in _automation_gate_stages(target, gate_policy, repo_root)
            ]
        missing_targets = [
            target
            for target in required_targets
            if not _automation_exists(target, repo_root)
            or stage not in _automation_gate_stages(target, gate_policy, repo_root)
        ]
        if not required_targets:
            errors.append(
                f"{case.get('id')} is impacted but declares no required targets at {stage}"
            )
        elif missing_targets:
            errors.append(
                f"{case.get('id')} impacted E2E targets do not actually run at {stage}: "
                + ", ".join(missing_targets)
            )

    return errors


def _is_product_file(path: str) -> bool:
    if path in GLOBAL_PRODUCT_FILES or path in EXPLICIT_PRODUCT_FILES:
        return True
    return path.startswith(PRODUCT_PREFIXES) and not _is_test_harness_file(path)


def _impacted_scenarios(
    files: list[str],
    registry: dict[str, Any],
    *,
    expand_global_files: bool = True,
) -> set[str]:
    impacted: set[str] = set()
    for scenario in registry.get("scenarios", []):
        patterns = scenario.get("change_patterns", [])
        if any(fnmatch.fnmatch(path, pattern) for path in files for pattern in patterns):
            impacted.add(scenario["id"])
    if expand_global_files and any(path in GLOBAL_PRODUCT_FILES for path in files):
        impacted.update(
            scenario["id"] for scenario in registry.get("scenarios", [])
        )
    return impacted


def validate_change_contract(
    title: str, body: str, files: list[str], registry: dict[str, Any]
) -> list[str]:
    product_files = [path for path in files if _is_product_file(path)]
    if not product_files:
        return []

    errors: list[str] = []
    match = DECLARATION.search(FENCED.sub("", body))
    impacted = _impacted_scenarios(product_files, registry)
    if not impacted:
        errors.append(
            "scenario registry coverage gap for product files: "
            + ", ".join(sorted(product_files))
        )
    if not match:
        suffix = f" Impacted scenarios: {', '.join(sorted(impacted))}." if impacted else ""
        errors.append("product change is missing 'Scenario-Test: <IDs>' declaration." + suffix)
        return errors

    declaration = match.group(1).strip()
    if declaration.lower().startswith("not-applicable"):
        errors.append(
            "Scenario-Test cannot be not-applicable for product changes"
            + (": " + ", ".join(sorted(impacted)) if impacted else "")
        )
        return errors

    known = {scenario["id"] for scenario in registry.get("scenarios", [])}
    if declaration.upper() == "ALL":
        declared = known
    else:
        declared = {item.strip().upper() for item in re.split(r"[,\s]+", declaration) if item.strip()}
    unknown = sorted(declared - known)
    if unknown:
        errors.append("Scenario-Test declares unknown scenario IDs: " + ", ".join(unknown))
    missing = sorted(impacted - declared)
    if missing:
        errors.append("Scenario-Test must include impacted scenarios: " + ", ".join(missing))
    return errors


def _event_change_contract_context(
    path: str | None,
) -> tuple[bool | None, str, str]:
    if not path or not Path(path).exists():
        return None, "", ""
    try:
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None, "", ""
    pull_request = payload.get("pull_request")
    if isinstance(pull_request, dict):
        return (
            True,
            pull_request.get("body") or "",
            pull_request.get("title") or "",
        )
    # A protected-branch push has no PR body. Its exact candidate already ran
    # the change contract as a required PR check; the post-merge workflow must
    # still validate the registry itself without inventing a missing body.
    if isinstance(payload.get("ref"), str) and (
        "before" in payload or "after" in payload
    ):
        return False, "", ""
    return None, "", ""


def _event_body_and_title(path: str | None) -> tuple[str, str]:
    """Compatibility helper for the PR-only scenario harness runner."""
    _, body, title = _event_change_contract_context(path)
    return body, title


def _changed_files(repo_root: Path, base_sha: str) -> tuple[list[str], str | None]:
    if not base_sha:
        return [], "scenario test contract needs a base SHA"
    result = subprocess.run(
        ["git", "-C", str(repo_root), "diff", "--name-only", f"{base_sha}...HEAD"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return [], f"cannot diff against '{base_sha}': {result.stderr.strip()}"
    return [line for line in result.stdout.splitlines() if line.strip()], None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ci", action="store_true")
    parser.add_argument("--repo")
    parser.add_argument("--base-ref")
    parser.add_argument("--title")
    parser.add_argument("--body-file")
    parser.add_argument(
        "--stage", choices=("pull_request", "release_artifact")
    )
    parser.add_argument("--enforce-ready", action="store_true")
    args = parser.parse_args()

    repo_root = Path(args.repo).resolve() if args.repo else REPO_ROOT
    try:
        registry = load_registry(repo_root / "docs" / "testing" / "scenario-registry.json")
    except ValueError as exc:
        print(f"scenario-test-governance: {exc}")
        return 1
    errors = validate_registry(registry, repo_root)

    if args.ci:
        requires_change_contract: bool | None
        if args.body_file:
            body = Path(args.body_file).read_text(encoding="utf-8")
            title = args.title or ""
            requires_change_contract = True
        else:
            requires_change_contract, body, title = _event_change_contract_context(
                os.environ.get("SCENARIO_TEST_EVENT_PATH") or os.environ.get("GITHUB_EVENT_PATH")
            )
        if requires_change_contract is None:
            errors.append("scenario change contract could not determine CI event type")
        elif requires_change_contract:
            base_ref = args.base_ref or os.environ.get("SCENARIO_TEST_BASE_SHA", "").strip()
            files, diff_error = _changed_files(repo_root, base_ref)
            if diff_error:
                errors.append(f"scenario change contract failed closed: {diff_error}")
            else:
                # The release bot's four-file version bump touches package.json,
                # a GLOBAL_PRODUCT_FILE, so without this every version PR is
                # "impacted by" all 27 scenarios and has to declare
                # `Scenario-Test: ALL` — a claim that a version-number change
                # exercised every scenario in the repository. It does not. The
                # harness-gate path already exempts an exact version bump; this
                # entry point (the one governance-baseline runs) did not, so the
                # two disagreed and the bot's own PRs could only pass by
                # over-declaring. Teaching people to write declarations they
                # know are false is the same rot the registry gates exist to
                # stop.
                errors.extend(
                    validate_change_contract(
                        title,
                        body,
                        scenario_impact_files(repo_root, base_ref, files),
                        registry,
                    )
                )

    if args.enforce_ready:
        if not args.stage:
            errors.append("--enforce-ready requires --stage")
        else:
            errors.extend(validate_gate_readiness(registry, args.stage))

    if errors:
        print(f"scenario-test-governance: {len(errors)} error(s)")
        for error in errors:
            print(f"::error::{error}")
        return 1
    print(
        "scenario-test-governance: OK "
        f"({len(registry['scenarios'])} scenarios, "
        f"{len(registry['complex_e2e_cases'])} complex E2E cases)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
