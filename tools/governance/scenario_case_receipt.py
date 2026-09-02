#!/usr/bin/env python3
"""Build and validate privacy-safe Scenario case receipts.

This module is the candidate-side M1 contract implementation.  It deliberately
does not replace the trusted execution receipt v1 yet: Bootstrap-1 will load the
same verifier from the default branch, bind its implementation digest, and
attach case receipts to the existing exact-head aggregate receipt.
"""

from __future__ import annotations

import hashlib
import json
import re
from typing import Any


CASE_RECEIPT_SCHEMA_VERSION = 2
FIXTURE_MANIFEST_SCHEMA_VERSION = 1
ORACLE_NAMES = ("ui", "durable_state", "process", "side_effects", "delivery")
ORACLE_OUTCOMES = {"passed", "failed", "not_required_for_stage"}
STAGES = {"pull_request", "nightly", "release_artifact"}
E2E001_SCENARIO_IDS = ["CXD-002", "HLT-001", "HLT-002"]
E2E001_TARGET = "binary:--unattended-long-task-smoke"
E2E001_ORACLE_OBSERVATIONS = {
    "ui": ["desktop_completion_visible"],
    "durable_state": [
        "objective_identity_stable",
        "single_user_message",
        "zero_human_prompt",
        "objective_completed",
    ],
    "process": [
        "replacement_process_observed",
        "supervisor_hard_kill_issued",
        "worker_reaped",
        "zero_descendants",
        "no_live_owner",
        "no_claimable_remediation",
    ],
    "side_effects": [
        "single_durable_side_effect_receipt",
        "replay_links_reused_receipt",
        "artifact_content_verified",
    ],
}
E2E001_NOT_REQUIRED_REASONS = {"ui": "headless_worker_has_no_webview"}
KNOWN_FIXTURE_CAPABILITIES = {
    "isolated_app_data",
    "sqlite_fixture",
    "git_fixture",
    "scripted_provider",
    "fake_forge",
    "managed_browser",
    "previous_release",
}
FIXTURE_CAPABILITY_DEPENDENCIES = {
    "sqlite_fixture": {"isolated_app_data"},
    "git_fixture": {"isolated_app_data"},
    "scripted_provider": {"isolated_app_data"},
    "fake_forge": {"git_fixture"},
    "managed_browser": {"isolated_app_data"},
    "previous_release": {"isolated_app_data"},
}
FORBIDDEN_PRIVATE_KEYS = {
    "sessionid",
    "objectiveid",
    "rootturnid",
    "receiptid",
    "cwd",
    "home",
    "path",
    "credential",
    "credentials",
    "token",
    "secret",
    "password",
    "apitoken",
    "apikey",
    "rawmessage",
    "rawprompt",
    "toolarguments",
    "toolargs",
}
SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/-]{0,159}$")
SAFE_SEED = re.compile(r"^[a-z0-9][a-z0-9._-]{2,79}$")
ABSOLUTE_PATH = re.compile(
    r"(?i)(?:file:///|[A-Za-z]:[\\/]|\\\\|/(?:Users|home|tmp|private|var|workspace)/)"
)
SECRET_VALUE = re.compile(
    r"(?i)(?:bearer\s+|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]+|"
    r"sk-(?:proj-)?[A-Za-z0-9_-]{8,}|github_pat_[A-Za-z0-9_]+|"
    r"gh[pousr]_[A-Za-z0-9]+|(?:api[_-]?key|token|password|secret|credential)\s*[:=])"
)
PRIVATE_CODE_MARKERS = ("token", "secret", "password", "credential", "api_key", "apikey")


def _is_safe_code(value: Any) -> bool:
    return (
        isinstance(value, str)
        and SAFE_ID.fullmatch(value) is not None
        and not any(marker in value.lower() for marker in PRIVATE_CODE_MARKERS)
    )


def _canonical_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=True,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def _sha256(value: Any) -> str:
    return hashlib.sha256(_canonical_json(value)).hexdigest()


def _privacy_errors(value: Any, location: str = "$") -> list[str]:
    errors: list[str] = []
    if isinstance(value, dict):
        for key, item in value.items():
            key_text = str(key)
            normalized_key = re.sub(r"[^a-z0-9]", "", key_text.lower())
            if normalized_key in FORBIDDEN_PRIVATE_KEYS:
                errors.append(f"{location} contains forbidden private key: {key_text}")
            errors.extend(_privacy_errors(item, f"{location}.{key_text}"))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            errors.extend(_privacy_errors(item, f"{location}[{index}]"))
    elif isinstance(value, str):
        if ABSOLUTE_PATH.search(value):
            errors.append(f"{location} contains an absolute path")
        if SECRET_VALUE.search(value):
            errors.append(f"{location} contains a secret-like value")
    return errors


def _normalized_fixture_manifest(manifest: dict[str, Any]) -> dict[str, Any]:
    capabilities = sorted(
        (
            {"id": item.get("id"), "schema_version": item.get("schema_version")}
            for item in manifest.get("capabilities") or []
            if isinstance(item, dict)
        ),
        key=lambda item: str(item["id"]),
    )
    return {
        "schema_version": manifest.get("schema_version"),
        "synthetic": manifest.get("synthetic"),
        "source_kind": manifest.get("source_kind"),
        "seed": manifest.get("seed"),
        "capabilities": capabilities,
    }


def validate_fixture_manifest(manifest: Any) -> list[str]:
    if not isinstance(manifest, dict):
        return ["fixture manifest must be an object"]
    errors = _privacy_errors(manifest, "fixture")
    allowed_keys = {"schema_version", "synthetic", "source_kind", "seed", "capabilities"}
    for key in sorted(set(manifest) - allowed_keys):
        errors.append(f"fixture manifest contains unsupported field: {key}")
    if manifest.get("schema_version") != FIXTURE_MANIFEST_SCHEMA_VERSION:
        errors.append("fixture manifest has an unsupported schema version")
    if manifest.get("synthetic") is not True:
        errors.append("fixture manifest must be synthetic")
    if manifest.get("source_kind") != "synthetic_fixture":
        errors.append("fixture manifest source_kind must be synthetic_fixture")
    seed = manifest.get("seed")
    if not isinstance(seed, str) or not SAFE_SEED.fullmatch(seed):
        errors.append("fixture manifest seed is invalid")

    capabilities = manifest.get("capabilities")
    if not isinstance(capabilities, list) or not capabilities:
        errors.append("fixture manifest must declare at least one capability")
        return errors
    seen: set[str] = set()
    for item in capabilities:
        if not isinstance(item, dict):
            errors.append("fixture capability must be an object")
            continue
        for key in sorted(set(item) - {"id", "schema_version"}):
            errors.append(f"fixture capability contains unsupported field: {key}")
        capability = item.get("id")
        if not isinstance(capability, str):
            errors.append("fixture capability id must be a string")
            capability_label = "invalid"
        elif capability not in KNOWN_FIXTURE_CAPABILITIES:
            errors.append(f"unknown fixture capability: {capability}")
            capability_label = capability
        elif capability in seen:
            errors.append(f"duplicate fixture capability: {capability}")
            capability_label = capability
        else:
            seen.add(capability)
            capability_label = capability
        if item.get("schema_version") != 1:
            errors.append(
                f"fixture capability {capability_label} has an unsupported schema version"
            )
    for capability in sorted(seen):
        missing = sorted(FIXTURE_CAPABILITY_DEPENDENCIES.get(capability, set()) - seen)
        if missing:
            errors.append(
                f"fixture capability {capability} is missing dependencies: {', '.join(missing)}"
            )
    return errors


def fixture_manifest_digest(manifest: dict[str, Any]) -> str:
    errors = validate_fixture_manifest(manifest)
    if errors:
        raise ValueError("; ".join(errors))
    return _sha256(_normalized_fixture_manifest(manifest))


def _validate_oracle(name: str, oracle: Any) -> list[str]:
    if not isinstance(oracle, dict):
        return [f"oracle {name} must be an object"]
    errors: list[str] = []
    allowed = {"outcome", "observations", "reason", "failure_code"}
    for key in sorted(set(oracle) - allowed):
        errors.append(f"oracle {name} contains unsupported field: {key}")
    outcome = oracle.get("outcome")
    if outcome not in ORACLE_OUTCOMES:
        errors.append(f"oracle {name} has an invalid outcome")
    observations = oracle.get("observations")
    if not isinstance(observations, list) or not all(
        _is_safe_code(item) for item in observations
    ):
        errors.append(f"oracle {name} observations must be safe identifiers")
    if outcome == "passed" and not observations:
        errors.append(f"oracle {name} passed without observations")
    expected_fields = {
        "passed": {"outcome", "observations"},
        "failed": {"outcome", "observations", "failure_code"},
        "not_required_for_stage": {"outcome", "observations", "reason"},
    }.get(outcome)
    if expected_fields is not None and set(oracle) != expected_fields:
        errors.append(f"oracle {name} fields contradict its outcome")
    if outcome == "not_required_for_stage":
        if observations:
            errors.append(f"oracle {name} not-required outcome cannot carry observations")
        reason = oracle.get("reason")
        if not _is_safe_code(reason):
            errors.append(f"oracle {name} not-required outcome needs a safe reason")
    if outcome == "failed":
        if not _is_safe_code(oracle.get("failure_code")):
            errors.append(f"oracle {name} failed outcome needs a safe failure code")
    errors.extend(_privacy_errors(oracle, f"oracles.{name}"))
    return errors


def validate_case_receipt_structure(receipt: Any) -> list[str]:
    if not isinstance(receipt, dict):
        return ["case receipt must be an object"]
    errors = _privacy_errors(receipt, "receipt")
    required = {
        "schema_version",
        "case_id",
        "scenario_ids",
        "stage",
        "base_sha",
        "head_sha",
        "canonical_target",
        "run_id",
        "evidence_sha256",
        "implementation",
        "runner",
        "build_identity",
        "fixture",
        "oracles",
        "cleanup",
        "outcome",
        "diagnostic_code",
    }
    allowed = required
    for key in sorted(required - set(receipt)):
        errors.append(f"case receipt is missing required field: {key}")
    for key in sorted(set(receipt) - allowed):
        errors.append(f"case receipt contains unsupported field: {key}")
    if receipt.get("schema_version") != CASE_RECEIPT_SCHEMA_VERSION:
        errors.append("case receipt has an unsupported schema version")
    if not _is_safe_code(receipt.get("case_id")):
        errors.append("case receipt case_id is invalid")
    scenario_ids = receipt.get("scenario_ids")
    scenario_items_valid = (
        isinstance(scenario_ids, list)
        and bool(scenario_ids)
        and all(_is_safe_code(item) for item in scenario_ids)
    )
    if not scenario_items_valid or scenario_ids != sorted(set(scenario_ids)):
        errors.append("case receipt scenario_ids must be a sorted unique non-empty list")
    if receipt.get("stage") not in STAGES:
        errors.append("case receipt stage is invalid")
    for field in ("base_sha", "head_sha"):
        if not isinstance(receipt.get(field), str) or not SHA40.fullmatch(receipt[field]):
            errors.append(f"case receipt {field.replace('_', ' ')} is invalid")
    if not _is_safe_code(receipt.get("canonical_target")):
        errors.append("case receipt canonical target is invalid")
    run_id = receipt.get("run_id")
    if not isinstance(run_id, str) or not run_id.startswith("sha256:") or not SHA256.fullmatch(run_id[7:]):
        errors.append("case receipt run_id must be a full sha256 digest")
    if not isinstance(receipt.get("evidence_sha256"), str) or not SHA256.fullmatch(
        receipt["evidence_sha256"]
    ):
        errors.append("case receipt evidence digest is invalid")

    implementation = receipt.get("implementation")
    if not isinstance(implementation, dict):
        errors.append("case receipt implementation must be an object")
    else:
        if set(implementation) != {"driver_sha256", "verifier_sha256"}:
            errors.append("case receipt implementation fields are invalid")
        for field in ("driver_sha256", "verifier_sha256"):
            if not isinstance(implementation.get(field), str) or not SHA256.fullmatch(
                implementation[field]
            ):
                errors.append(f"case receipt {field.replace('_', ' ')} is invalid")

    runner = receipt.get("runner")
    if not isinstance(runner, dict) or set(runner) != {"name", "os", "arch"}:
        errors.append("case receipt runner fields are invalid")
    elif not all(_is_safe_code(value) for value in runner.values()):
        errors.append("case receipt runner values are invalid")

    build = receipt.get("build_identity")
    if not isinstance(build, dict) or set(build) != {
        "source_sha",
        "executable_build_sha",
        "executable_sha256",
        "artifact_sha256",
        "version",
        "tag_sha",
    }:
        errors.append("case receipt build identity fields are invalid")
    elif not isinstance(build.get("source_sha"), str) or not SHA40.fullmatch(build["source_sha"]):
        errors.append("case receipt build source SHA is invalid")
    else:
        executable_build_sha = build.get("executable_build_sha")
        if executable_build_sha != "unknown" and (
            not isinstance(executable_build_sha, str)
            or not SHA40.fullmatch(executable_build_sha)
        ):
            errors.append("case receipt executable build SHA is invalid")
        executable_sha256 = build.get("executable_sha256")
        if executable_sha256 is not None and (
            not isinstance(executable_sha256, str) or not SHA256.fullmatch(executable_sha256)
        ):
            errors.append("case receipt executable digest is invalid")
        artifact_sha256 = build.get("artifact_sha256")
        if artifact_sha256 is not None and (
            not isinstance(artifact_sha256, str) or not SHA256.fullmatch(artifact_sha256)
        ):
            errors.append("case receipt artifact digest is invalid")
        version = build.get("version")
        if version is not None and (
            not _is_safe_code(version)
        ):
            errors.append("case receipt version is invalid")
        tag_sha = build.get("tag_sha")
        if tag_sha is not None and (
            not isinstance(tag_sha, str) or not SHA40.fullmatch(tag_sha)
        ):
            errors.append("case receipt tag SHA is invalid")

    fixture = receipt.get("fixture")
    if not isinstance(fixture, dict) or set(fixture) != {"manifest", "manifest_sha256"}:
        errors.append("case receipt fixture fields are invalid")
    else:
        manifest = fixture.get("manifest")
        fixture_errors = validate_fixture_manifest(manifest)
        errors.extend(f"case receipt {error}" for error in fixture_errors)
        if not isinstance(fixture.get("manifest_sha256"), str) or not SHA256.fullmatch(
            fixture["manifest_sha256"]
        ):
            errors.append("case receipt fixture manifest digest is invalid")
        elif not fixture_errors and fixture_manifest_digest(manifest) != fixture["manifest_sha256"]:
            errors.append("case receipt fixture manifest digest contradicts its projection")

    oracles = receipt.get("oracles")
    if not isinstance(oracles, dict) or set(oracles) != set(ORACLE_NAMES):
        errors.append("case receipt must contain exactly the five oracle classes")
    else:
        for name in ORACLE_NAMES:
            errors.extend(_validate_oracle(name, oracles[name]))

    cleanup = receipt.get("cleanup")
    if not isinstance(cleanup, dict) or set(cleanup) != {
        "outcome",
        "leaked_resources",
        "cleanup_attempted",
        "orphan_sweep_performed",
        "failure_code",
    }:
        errors.append("case receipt cleanup fields are invalid")
    else:
        if cleanup.get("outcome") not in {"passed", "failed"}:
            errors.append("case receipt cleanup outcome is invalid")
        leaks = cleanup.get("leaked_resources")
        if leaks is not None and (
            not isinstance(leaks, int) or isinstance(leaks, bool) or leaks < 0
        ):
            errors.append("case receipt cleanup leaked_resources is invalid")
        if not isinstance(cleanup.get("cleanup_attempted"), bool):
            errors.append("case receipt cleanup_attempted must be boolean")
        if not isinstance(cleanup.get("orphan_sweep_performed"), bool):
            errors.append("case receipt orphan_sweep_performed must be boolean")
        if not _is_safe_code(cleanup.get("failure_code")):
            errors.append("case receipt cleanup failure code is invalid")
        if cleanup.get("outcome") == "passed" and (
            leaks != 0
            or cleanup.get("cleanup_attempted") is not True
            or cleanup.get("failure_code") != "none"
        ):
            errors.append("passed cleanup must be attempted, leak-free, and have no failure")
        if cleanup.get("outcome") == "failed" and cleanup.get("failure_code") == "none":
            errors.append("failed cleanup must name a failure code")

    if receipt.get("outcome") not in {"passed", "failed"}:
        errors.append("case receipt outcome is invalid")
    if receipt.get("outcome") == "passed":
        if isinstance(cleanup, dict) and cleanup.get("outcome") != "passed":
            errors.append("passed receipt cannot contain failed cleanup")
        if isinstance(oracles, dict) and any(
            isinstance(value, dict) and value.get("outcome") == "failed"
            for value in oracles.values()
        ):
            errors.append("passed receipt cannot contain a failed oracle")
    diagnostic_code = receipt.get("diagnostic_code")
    if not _is_safe_code(diagnostic_code):
        errors.append("case receipt diagnostic code is invalid")
    if receipt.get("outcome") == "passed" and diagnostic_code != "case_observations_accepted":
        errors.append("passed receipt must use the accepted diagnostic code")
    if receipt.get("outcome") == "failed" and diagnostic_code == "case_observations_accepted":
        errors.append("failed receipt cannot use the accepted diagnostic code")
    return errors


def _e2e001_oracle_policy(stage: str) -> dict[str, str]:
    return {
        name: (
            "not_required_for_stage"
            if name == "ui" and stage != "release_artifact"
            else "required"
        )
        for name in ORACLE_NAMES
    }


def _e2e001_expected_observations(stage: str, name: str) -> list[str]:
    if name == "delivery":
        identity_observation = (
            "exact_release_identity_bound"
            if stage == "release_artifact"
            else "candidate_head_bound"
        )
        return [
            identity_observation,
            "canonical_target_bound",
            "implementation_digests_bound",
        ]
    return E2E001_ORACLE_OBSERVATIONS[name]


def _validate_expectation(expectation: Any) -> list[str]:
    if not isinstance(expectation, dict):
        return ["trusted expectation must be an object"]
    errors: list[str] = []
    required = {
        "case_id",
        "scenario_ids",
        "stage",
        "base_sha",
        "head_sha",
        "canonical_target",
        "oracle_policy",
        "runner",
        "build_identity",
        "fixture_manifest_sha256",
        "driver_sha256",
        "verifier_sha256",
    }
    if set(expectation) != required:
        errors.append("trusted expectation fields are invalid")
    if expectation.get("case_id") != "E2E-001":
        errors.append("trusted E2E-001 expectation has the wrong case ID")
    scenario_ids = expectation.get("scenario_ids")
    if scenario_ids != E2E001_SCENARIO_IDS:
        errors.append("trusted E2E-001 expectation has the wrong scenario IDs")
    stage = expectation.get("stage")
    if stage not in STAGES:
        errors.append("trusted expectation stage is invalid")
    elif expectation.get("oracle_policy") != _e2e001_oracle_policy(stage):
        errors.append("trusted E2E-001 expectation has an invalid oracle policy")
    if expectation.get("canonical_target") != E2E001_TARGET:
        errors.append("trusted E2E-001 expectation has the wrong canonical target")
    for field in ("base_sha", "head_sha"):
        value = expectation.get(field)
        if not isinstance(value, str) or not SHA40.fullmatch(value):
            errors.append(f"trusted expectation {field.replace('_', ' ')} is invalid")
    runner = expectation.get("runner")
    if (
        not isinstance(runner, dict)
        or set(runner) != {"name", "os", "arch"}
        or not all(_is_safe_code(value) for value in runner.values())
    ):
        errors.append("trusted expectation runner is invalid")
    for field in ("fixture_manifest_sha256", "driver_sha256", "verifier_sha256"):
        value = expectation.get(field)
        if not isinstance(value, str) or not SHA256.fullmatch(value):
            errors.append(f"trusted expectation {field.replace('_', ' ')} is invalid")
    build = expectation.get("build_identity")
    if not isinstance(build, dict):
        errors.append("trusted expectation build identity is invalid")
    return errors


def case_receipt_run_id(receipt: dict[str, Any], oracle_policy: Any) -> str:
    binding = {
        "case_id": receipt.get("case_id"),
        "scenario_ids": receipt.get("scenario_ids"),
        "stage": receipt.get("stage"),
        "base_sha": receipt.get("base_sha"),
        "head_sha": receipt.get("head_sha"),
        "canonical_target": receipt.get("canonical_target"),
        "evidence_sha256": receipt.get("evidence_sha256"),
        "implementation": receipt.get("implementation"),
        "runner": receipt.get("runner"),
        "build_identity": receipt.get("build_identity"),
        "fixture_manifest_sha256": (receipt.get("fixture") or {}).get("manifest_sha256")
        if isinstance(receipt.get("fixture"), dict)
        else None,
        "oracle_policy": oracle_policy,
        "oracles": receipt.get("oracles"),
        "cleanup": receipt.get("cleanup"),
    }
    return f"sha256:{_sha256(binding)}"


def validate_case_receipt_for_gate(
    receipt: Any, expectation: Any
) -> list[str]:
    errors = validate_case_receipt_structure(receipt)
    errors.extend(_validate_expectation(expectation))
    if not isinstance(receipt, dict) or not isinstance(expectation, dict):
        return sorted(set(errors))
    comparisons = (
        ("case_id", "case ID"),
        ("stage", "stage"),
        ("base_sha", "base SHA"),
        ("head_sha", "head SHA"),
        ("canonical_target", "canonical target"),
    )
    for field, label in comparisons:
        if receipt.get(field) != expectation.get(field):
            errors.append(f"case receipt {label} does not match the trusted expectation")
    expected_scenarios = expectation.get("scenario_ids")
    if receipt.get("scenario_ids") != expected_scenarios:
        errors.append("case receipt scenario IDs do not match the trusted expectation")
    if receipt.get("runner") != expectation.get("runner"):
        errors.append("case receipt runner does not match the trusted expectation")
    implementation = receipt.get("implementation")
    if not isinstance(implementation, dict):
        implementation = {}
    if implementation.get("driver_sha256") != expectation.get("driver_sha256"):
        errors.append("case receipt driver digest does not match the trusted expectation")
    if implementation.get("verifier_sha256") != expectation.get("verifier_sha256"):
        errors.append("case receipt verifier digest does not match the trusted expectation")
    fixture = receipt.get("fixture")
    if not isinstance(fixture, dict):
        fixture = {}
    if fixture.get("manifest_sha256") != expectation.get("fixture_manifest_sha256"):
        errors.append("case receipt fixture manifest digest does not match the trusted expectation")

    oracle_policy = expectation.get("oracle_policy")
    if (
        not isinstance(oracle_policy, dict)
        or set(oracle_policy) != set(ORACLE_NAMES)
        or not set(oracle_policy.values()).issubset({"required", "not_required_for_stage"})
    ):
        errors.append("trusted expectation has an invalid oracle policy")
        oracle_policy = {}
    oracles = receipt.get("oracles")
    if not isinstance(oracles, dict):
        oracles = {}
    for name in ORACLE_NAMES:
        oracle = oracles.get(name)
        outcome = oracle.get("outcome") if isinstance(oracle, dict) else None
        if oracle_policy.get(name) == "required" and outcome != "passed":
            errors.append(f"required oracle {name} did not pass")
        elif oracle_policy.get(name) == "required" and oracle.get(
            "observations"
        ) != _e2e001_expected_observations(expectation.get("stage"), name):
            errors.append(
                f"required oracle {name} observations do not match the trusted E2E-001 contract"
            )
        elif (
            oracle_policy.get(name) == "not_required_for_stage"
            and outcome != "not_required_for_stage"
        ):
            errors.append(f"oracle {name} must be not_required_for_stage")
        elif oracle_policy.get(name) == "not_required_for_stage" and (
            oracle.get("observations") != []
            or oracle.get("reason") != E2E001_NOT_REQUIRED_REASONS.get(name)
        ):
            errors.append(
                f"oracle {name} not-required reason does not match the trusted E2E-001 contract"
            )

    cleanup = receipt.get("cleanup")
    if not isinstance(cleanup, dict):
        cleanup = {}
    if cleanup.get("outcome") != "passed":
        errors.append("case receipt cleanup did not pass")
    if cleanup.get("leaked_resources") != 0:
        errors.append("case receipt reports leaked resources")
    if cleanup.get("cleanup_attempted") is not True:
        errors.append("case receipt cleanup was not attempted")
    if cleanup.get("orphan_sweep_performed") is not True:
        errors.append("E2E-001 case receipt did not perform the orphan sweep")
    build = receipt.get("build_identity")
    if not isinstance(build, dict):
        build = {}
    if build != expectation.get("build_identity"):
        errors.append("case receipt build identity does not match the trusted expectation")
    if build.get("source_sha") != expectation.get("head_sha"):
        errors.append("case receipt build source SHA does not match the head SHA")
    if expectation.get("stage") == "release_artifact":
        if build.get("executable_build_sha") != expectation.get("head_sha"):
            errors.append("release executable build SHA does not match the head SHA")
        if not isinstance(build.get("executable_sha256"), str) or not SHA256.fullmatch(
            build["executable_sha256"]
        ):
            errors.append("release executable digest is missing or invalid")
        if build.get("tag_sha") != expectation.get("head_sha"):
            errors.append("release tag SHA does not match the head SHA")
        if not isinstance(build.get("artifact_sha256"), str) or not SHA256.fullmatch(
            build["artifact_sha256"]
        ):
            errors.append("release artifact digest is missing or invalid")
        if not isinstance(build.get("version"), str) or not build["version"]:
            errors.append("release version is missing")
    if isinstance(receipt, dict) and receipt.get("run_id") != case_receipt_run_id(
        receipt, oracle_policy
    ):
        errors.append("case receipt run_id does not match its full binding")
    if receipt.get("outcome") != ("passed" if not errors else "failed"):
        errors.append("case receipt outcome contradicts the trusted gate result")
    return sorted(set(errors))


def _oracle(outcome: str, observations: list[str], failure_code: str) -> dict[str, Any]:
    value: dict[str, Any] = {"outcome": outcome, "observations": observations}
    if outcome == "failed":
        value["failure_code"] = failure_code
    return value


def _not_required(reason: str) -> dict[str, Any]:
    return {
        "outcome": "not_required_for_stage",
        "reason": reason,
        "observations": [],
    }


def build_e2e001_case_receipt(
    raw_receipt: dict[str, Any],
    expectation: dict[str, Any],
    manifest: dict[str, Any],
    *,
    runner: dict[str, str],
    build_identity: dict[str, Any],
) -> dict[str, Any]:
    """Project E2E-001 observations into a stage-bound case receipt.

    The raw binary never chooses which oracle is required.  The trusted
    expectation supplies stage and identity; this adapter only maps bounded
    observations to named outcomes and discards raw error text.
    """

    manifest_sha256 = fixture_manifest_digest(manifest)
    raw_identity_ok = all(
        (
            raw_receipt.get("observation_schema_version") == 1,
            raw_receipt.get("case_id") == "E2E-001",
            raw_receipt.get("scenario_ids") == E2E001_SCENARIO_IDS,
            raw_receipt.get("scenario_id") == "HLT-001",
        )
    )
    durable_ok = bool(raw_receipt.get("ok")) and raw_identity_ok and all(
        (
            raw_receipt.get("same_objective") is True,
            raw_receipt.get("user_message_count") == 1,
            raw_receipt.get("human_prompt_count") == 0,
            raw_receipt.get("objective_status") == "completed",
        )
    )
    process_ok = bool(raw_receipt.get("ok")) and all(
        (
            raw_receipt.get("process_restart_observed") is True,
            raw_receipt.get("supervisor_hard_kill_issued") is True,
            raw_receipt.get("worker_reaped") is True,
            raw_receipt.get("replacement_process_distinct") is True,
            raw_receipt.get("descendant_process_count") == 0,
            raw_receipt.get("live_owner_count") == 0,
            raw_receipt.get("claimable_remediation_count") == 0,
        )
    )
    side_effects_ok = bool(raw_receipt.get("ok")) and all(
        (
            raw_receipt.get("side_effect_receipt_count") == 1,
            raw_receipt.get("replay_call_link_count") == 2,
            raw_receipt.get("artifact_verified") is True,
        )
    )
    cleanup_attempted = raw_receipt.get("cleanup_attempted") is True
    orphan_sweep_performed = raw_receipt.get("orphan_sweep_performed") is True
    raw_leaks = raw_receipt.get("leaked_resource_count")
    leaked_resources = (
        raw_leaks
        if isinstance(raw_leaks, int) and not isinstance(raw_leaks, bool) and raw_leaks >= 0
        else None
    )
    cleanup_ok = all(
        (
            raw_receipt.get("cleanup_ok") is True,
            cleanup_attempted,
            orphan_sweep_performed,
            leaked_resources == 0,
        )
    )
    release_identity_ok = all(
        (
            build_identity == expectation.get("build_identity"),
            build_identity.get("source_sha") == expectation.get("head_sha"),
            build_identity.get("executable_build_sha") == expectation.get("head_sha"),
            isinstance(build_identity.get("executable_sha256"), str),
            bool(
                isinstance(build_identity.get("executable_sha256"), str)
                and SHA256.fullmatch(build_identity["executable_sha256"])
            ),
            build_identity.get("tag_sha") == expectation.get("head_sha"),
            isinstance(build_identity.get("artifact_sha256"), str),
            bool(
                isinstance(build_identity.get("artifact_sha256"), str)
                and SHA256.fullmatch(build_identity["artifact_sha256"])
            ),
            isinstance(build_identity.get("version"), str),
            bool(build_identity.get("version")),
            raw_receipt.get("build_git_sha") == build_identity.get("executable_build_sha"),
        )
    )
    delivery_ok = all(
        (
            raw_identity_ok,
            build_identity == expectation.get("build_identity"),
            expectation.get("canonical_target") == E2E001_TARGET,
            bool(SHA256.fullmatch(str(expectation.get("driver_sha256", "")))),
            bool(SHA256.fullmatch(str(expectation.get("verifier_sha256", "")))),
            manifest_sha256 == expectation.get("fixture_manifest_sha256"),
        )
    ) and (expectation.get("stage") != "release_artifact" or release_identity_ok)
    oracles = {
        "ui": _not_required("headless_worker_has_no_webview"),
        "durable_state": _oracle(
            "passed" if durable_ok else "failed",
            ["objective_identity_stable", "single_user_message", "zero_human_prompt", "objective_completed"]
            if durable_ok
            else [],
            "durable_state_assertions_failed",
        ),
        "process": _oracle(
            "passed" if process_ok else "failed",
            ["replacement_process_observed", "supervisor_hard_kill_issued", "worker_reaped", "zero_descendants", "no_live_owner", "no_claimable_remediation"]
            if process_ok
            else [],
            "process_assertions_failed",
        ),
        "side_effects": _oracle(
            "passed" if side_effects_ok else "failed",
            ["single_durable_side_effect_receipt", "replay_links_reused_receipt", "artifact_content_verified"]
            if side_effects_ok
            else [],
            "side_effect_assertions_failed",
        ),
        "delivery": _oracle(
            "passed" if delivery_ok else "failed",
            (
                ["exact_release_identity_bound", "canonical_target_bound", "implementation_digests_bound"]
                if expectation.get("stage") == "release_artifact" and delivery_ok
                else ["candidate_head_bound", "canonical_target_bound", "implementation_digests_bound"]
                if delivery_ok
                else []
            ),
            "delivery_identity_assertions_failed",
        ),
    }
    cleanup = {
        "outcome": "passed" if cleanup_ok else "failed",
        "leaked_resources": leaked_resources,
        "cleanup_attempted": cleanup_attempted,
        "orphan_sweep_performed": orphan_sweep_performed,
        "failure_code": "none" if cleanup_ok else "fixture_cleanup_failed",
    }
    oracle_policy = expectation.get("oracle_policy") or {}
    passed = cleanup_ok and all(
        oracles[name]["outcome"]
        == ("passed" if policy == "required" else "not_required_for_stage")
        for name, policy in oracle_policy.items()
    ) and set(oracle_policy) == set(ORACLE_NAMES)
    normalized_observations = {
        "ok": raw_receipt.get("ok") is True,
        "raw_identity_matches": raw_identity_ok,
        "same_objective": raw_receipt.get("same_objective") is True,
        "single_user_message": raw_receipt.get("user_message_count") == 1,
        "zero_human_prompt": raw_receipt.get("human_prompt_count") == 0,
        "single_side_effect_receipt": raw_receipt.get("side_effect_receipt_count") == 1,
        "two_replay_links": raw_receipt.get("replay_call_link_count") == 2,
        "objective_completed": raw_receipt.get("objective_status") == "completed",
        "no_live_owner": raw_receipt.get("live_owner_count") == 0,
        "no_claimable_remediation": raw_receipt.get("claimable_remediation_count") == 0,
        "process_restart_observed": raw_receipt.get("process_restart_observed") is True,
        "supervisor_hard_kill_issued": raw_receipt.get("supervisor_hard_kill_issued") is True,
        "worker_reaped": raw_receipt.get("worker_reaped") is True,
        "replacement_process_distinct": raw_receipt.get("replacement_process_distinct") is True,
        "zero_descendants": raw_receipt.get("descendant_process_count") == 0,
        "artifact_verified": raw_receipt.get("artifact_verified") is True,
        "cleanup_attempted": cleanup_attempted,
        "orphan_sweep_performed": orphan_sweep_performed,
        "leaked_resources": leaked_resources,
        "cleanup_ok": cleanup_ok,
        "reported_build_matches": raw_receipt.get("build_git_sha")
        == build_identity.get("executable_build_sha"),
    }
    evidence_sha256 = _sha256(normalized_observations)
    normalized_manifest = _normalized_fixture_manifest(manifest)
    receipt = {
        "schema_version": CASE_RECEIPT_SCHEMA_VERSION,
        "case_id": expectation.get("case_id"),
        "scenario_ids": sorted(set(expectation.get("scenario_ids") or [])),
        "stage": expectation.get("stage"),
        "base_sha": expectation.get("base_sha"),
        "head_sha": expectation.get("head_sha"),
        "canonical_target": expectation.get("canonical_target"),
        "run_id": "sha256:" + "0" * 64,
        "evidence_sha256": evidence_sha256,
        "implementation": {
            "driver_sha256": expectation.get("driver_sha256"),
            "verifier_sha256": expectation.get("verifier_sha256"),
        },
        "runner": runner,
        "build_identity": build_identity,
        "fixture": {"manifest": normalized_manifest, "manifest_sha256": manifest_sha256},
        "oracles": oracles,
        "cleanup": cleanup,
        "outcome": "passed" if passed else "failed",
        "diagnostic_code": "case_observations_accepted"
        if passed
        else "unattended_smoke_failed"
        if raw_receipt.get("ok") is not True
        else "unattended_smoke_rejected",
    }
    receipt["run_id"] = case_receipt_run_id(receipt, oracle_policy)
    return receipt
