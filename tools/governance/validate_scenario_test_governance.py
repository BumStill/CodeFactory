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
ENFORCED_TYPES = re.compile(r"^(feat|fix)(\([^)]*\))?!?:", re.IGNORECASE)
DECLARATION = re.compile(
    r"^[ \t]*Scenario-Test:[ \t]*(.+?)[ \t]*$", re.IGNORECASE | re.MULTILINE
)
FENCED = re.compile(r"```.*?```", re.DOTALL)
PRODUCT_PREFIXES = (
    "src/",
    "src-tauri/src/",
    "src-tauri/crates/",
    "tools/agent/",
    "scripts/",
)
NON_PRODUCT_MARKERS = (".test.", ".spec.", "/tests/", "src/acceptance/")


def load_registry(path: Path = REGISTRY_PATH) -> dict[str, Any]:
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
    return [path for path in root.rglob(f"*{suffix}") if path.is_file()]


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
        return (repo_root / marker).is_file()
    if kind == "rust":
        roots = [repo_root / "src-tauri" / "src", repo_root / "src-tauri" / "crates"]
        files = [file for root in roots if root.exists() for file in _files_with_suffix(root, ".rs")]
    elif kind == "workflow":
        files = _files_with_suffix(repo_root / ".github" / "workflows", ".yml")
    elif kind == "binary":
        files = _files_with_suffix(repo_root / "src-tauri", ".rs")
    else:
        return False
    return any(marker in path.read_text(encoding="utf-8", errors="ignore") for path in files)


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
    if registry.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    if registry.get("source_policy") != "aggregate-shapes-only":
        errors.append("source_policy must be aggregate-shapes-only")

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
        if not isinstance(automation, list) or not automation:
            errors.append(f"{scenario_id} must name at least one automation target")
        else:
            for target in automation:
                if not isinstance(target, str) or not _automation_exists(target, repo_root):
                    errors.append(f"{scenario_id} points at missing automation: {target}")
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
        if case.get("must_remain_unattended") is True:
            serialized = json.dumps(oracles, ensure_ascii=False)
            for marker in ("user message 总数为 1", "human prompt 总数为 0"):
                if marker not in serialized:
                    errors.append(f"{case_id} unattended oracle missing: {marker}")

    privacy = registry.get("privacy")
    if not isinstance(privacy, dict) or not privacy.get("forbidden"):
        errors.append("privacy.forbidden must be declared")
    errors.extend(_forbidden_keys({key: value for key, value in registry.items() if key != "privacy"}))
    return errors


def _is_product_file(path: str) -> bool:
    return path.startswith(PRODUCT_PREFIXES) and not any(marker in path for marker in NON_PRODUCT_MARKERS)


def _impacted_p0_scenarios(files: list[str], registry: dict[str, Any]) -> set[str]:
    impacted: set[str] = set()
    for scenario in registry.get("scenarios", []):
        if scenario.get("priority") != "P0":
            continue
        patterns = scenario.get("change_patterns", [])
        if any(fnmatch.fnmatch(path, pattern) for path in files for pattern in patterns):
            impacted.add(scenario["id"])
    return impacted


def validate_change_contract(
    title: str, body: str, files: list[str], registry: dict[str, Any]
) -> list[str]:
    if not ENFORCED_TYPES.match(title.strip()) or not any(_is_product_file(path) for path in files):
        return []

    errors: list[str] = []
    match = DECLARATION.search(FENCED.sub("", body))
    impacted = _impacted_p0_scenarios(files, registry)
    if not match:
        suffix = f" Impacted P0 scenarios: {', '.join(sorted(impacted))}." if impacted else ""
        return ["product feat/fix is missing 'Scenario-Test: <IDs>' declaration." + suffix]

    declaration = match.group(1).strip()
    if declaration.lower().startswith("not-applicable"):
        if impacted:
            return [
                "Scenario-Test cannot be not-applicable for impacted P0 scenarios: "
                + ", ".join(sorted(impacted))
            ]
        if "-" not in declaration or len(declaration.split("-", 1)[1].strip()) < 12:
            return ["Scenario-Test not-applicable requires a specific reason"]
        return []

    declared = {item.strip().upper() for item in re.split(r"[,\s]+", declaration) if item.strip()}
    known = {scenario["id"] for scenario in registry.get("scenarios", [])}
    unknown = sorted(declared - known)
    if unknown:
        errors.append("Scenario-Test declares unknown scenario IDs: " + ", ".join(unknown))
    missing = sorted(impacted - declared)
    if missing:
        errors.append("Scenario-Test must include impacted P0 scenarios: " + ", ".join(missing))
    return errors


def _event_body_and_title(path: str | None) -> tuple[str, str]:
    if not path or not Path(path).exists():
        return "", ""
    try:
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return "", ""
    pull_request = payload.get("pull_request")
    if not isinstance(pull_request, dict):
        return "", ""
    return pull_request.get("body") or "", pull_request.get("title") or ""


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
    args = parser.parse_args()

    repo_root = Path(args.repo).resolve() if args.repo else REPO_ROOT
    try:
        registry = load_registry(repo_root / "docs" / "testing" / "scenario-registry.json")
    except ValueError as exc:
        print(f"scenario-test-governance: {exc}")
        return 1
    errors = validate_registry(registry, repo_root)

    if args.ci:
        if args.body_file:
            body = Path(args.body_file).read_text(encoding="utf-8")
            title = args.title or ""
        else:
            body, title = _event_body_and_title(
                os.environ.get("SCENARIO_TEST_EVENT_PATH") or os.environ.get("GITHUB_EVENT_PATH")
            )
        files, diff_error = _changed_files(
            repo_root,
            args.base_ref or os.environ.get("SCENARIO_TEST_BASE_SHA", "").strip(),
        )
        if diff_error:
            print(f"::warning::scenario change contract skipped: {diff_error}")
        else:
            errors.extend(validate_change_contract(title, body, files, registry))

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
