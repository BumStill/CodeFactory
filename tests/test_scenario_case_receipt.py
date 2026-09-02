from __future__ import annotations

import copy
import json
import unittest
from pathlib import Path

from tools.governance.scenario_case_receipt import (
    build_e2e001_case_receipt,
    case_receipt_run_id,
    fixture_manifest_digest,
    validate_case_receipt_for_gate,
    validate_case_receipt_structure,
    validate_fixture_manifest,
)


BASE_SHA = "a" * 40
HEAD_SHA = "b" * 40
DRIVER_SHA = "c" * 64
VERIFIER_SHA = "d" * 64
FIXTURE_ROOT = Path(__file__).parent / "fixtures" / "scenarios" / "e2e-001"


def fixture_manifest() -> dict:
    return json.loads((FIXTURE_ROOT / "fixture-manifest.json").read_text(encoding="utf-8"))


def expectation(stage: str = "pull_request") -> dict:
    oracle_policy = {
        name: (
            "not_required_for_stage"
            if name == "ui" and stage != "release_artifact"
            else "required"
        )
        for name in ("ui", "durable_state", "process", "side_effects", "delivery")
    }
    return {
        "case_id": "E2E-001",
        "scenario_ids": ["CXD-002", "HLT-001", "HLT-002"],
        "stage": stage,
        "base_sha": BASE_SHA,
        "head_sha": HEAD_SHA,
        "canonical_target": "binary:--unattended-long-task-smoke",
        "oracle_policy": oracle_policy,
        "runner": runner(),
        "build_identity": build_identity(stage == "release_artifact"),
        "fixture_manifest_sha256": fixture_manifest_digest(fixture_manifest()),
        "driver_sha256": DRIVER_SHA,
        "verifier_sha256": VERIFIER_SHA,
    }


def raw_smoke() -> dict:
    return json.loads((FIXTURE_ROOT / "raw-pass.json").read_text(encoding="utf-8"))


def runner() -> dict:
    return {"name": "windows-latest", "os": "windows", "arch": "x86_64"}


def build_identity(release: bool = False) -> dict:
    if release:
        return {
            "source_sha": HEAD_SHA,
            "executable_build_sha": HEAD_SHA,
            "executable_sha256": "f" * 64,
            "artifact_sha256": "e" * 64,
            "version": "1.82.0",
            "tag_sha": HEAD_SHA,
        }
    return {
        "source_sha": HEAD_SHA,
        "executable_build_sha": "unknown",
        "executable_sha256": None,
        "artifact_sha256": None,
        "version": None,
        "tag_sha": None,
    }


def receipt(stage: str = "pull_request") -> dict:
    raw = raw_smoke()
    if stage == "release_artifact":
        raw["build_git_sha"] = HEAD_SHA
    return build_e2e001_case_receipt(
        raw,
        expectation(stage),
        fixture_manifest(),
        runner=runner(),
        build_identity=build_identity(stage == "release_artifact"),
    )


class FixtureManifestContractTests(unittest.TestCase):
    def test_fixture_digest_is_stable_across_capability_order(self) -> None:
        manifest = fixture_manifest()
        reversed_manifest = copy.deepcopy(manifest)
        reversed_manifest["capabilities"].reverse()

        self.assertEqual(validate_fixture_manifest(manifest), [])
        self.assertEqual(
            fixture_manifest_digest(manifest),
            fixture_manifest_digest(reversed_manifest),
        )

    def test_fixture_must_be_synthetic_and_use_known_capabilities(self) -> None:
        real_fixture = fixture_manifest()
        real_fixture["synthetic"] = False
        unknown = fixture_manifest()
        unknown["capabilities"].append({"id": "production_session", "schema_version": 1})

        self.assertTrue(any("synthetic" in error for error in validate_fixture_manifest(real_fixture)))
        self.assertTrue(any("unknown fixture capability" in error for error in validate_fixture_manifest(unknown)))

    def test_fixture_digest_changes_with_seed_or_capability(self) -> None:
        manifest = fixture_manifest()
        changed_seed = copy.deepcopy(manifest)
        changed_seed["seed"] = "e2e-001-v2"
        changed_capability = copy.deepcopy(manifest)
        changed_capability["capabilities"].append(
            {"id": "fake_forge", "schema_version": 1}
        )

        self.assertNotEqual(
            fixture_manifest_digest(manifest), fixture_manifest_digest(changed_seed)
        )
        self.assertNotEqual(
            fixture_manifest_digest(manifest),
            fixture_manifest_digest(changed_capability),
        )

    def test_fixture_capability_dependencies_fail_closed(self) -> None:
        missing_dependency = fixture_manifest()
        missing_dependency["capabilities"] = [
            {"id": "fake_forge", "schema_version": 1}
        ]

        errors = validate_fixture_manifest(missing_dependency)

        self.assertTrue(any("missing dependencies: git_fixture" in error for error in errors), errors)

    def test_fixture_rejects_private_ids_paths_and_credentials(self) -> None:
        private = fixture_manifest()
        private["session_id"] = "real-session"
        private["cwd"] = "/Users/alice/private-project"
        private["token"] = "secret-value"
        private["apiToken"] = "ghp_supersecret"

        errors = validate_fixture_manifest(private)

        self.assertTrue(any("session_id" in error for error in errors), errors)
        self.assertTrue(any("absolute path" in error for error in errors), errors)
        self.assertTrue(any("token" in error for error in errors), errors)
        self.assertTrue(any("apiToken" in error for error in errors), errors)


class CaseReceiptContractTests(unittest.TestCase):
    def test_e2e001_pull_request_slice_is_a_valid_bound_receipt(self) -> None:
        actual = receipt()

        self.assertEqual(validate_case_receipt_structure(actual), [])
        self.assertEqual(validate_case_receipt_for_gate(actual, expectation()), [])
        self.assertEqual(actual["outcome"], "passed")
        self.assertTrue(actual["run_id"].startswith("sha256:"))
        self.assertEqual(actual["oracles"]["ui"]["outcome"], "not_required_for_stage")
        for oracle in ("durable_state", "process", "side_effects", "delivery"):
            self.assertEqual(actual["oracles"][oracle]["outcome"], "passed")

    def test_receipt_must_match_case_stage_shas_target_and_scenarios(self) -> None:
        actual = receipt()
        wrong = expectation()
        wrong["head_sha"] = "f" * 40
        wrong["canonical_target"] = "binary:--different-smoke"
        wrong["scenario_ids"] = ["HLT-001"]

        errors = validate_case_receipt_for_gate(actual, wrong)

        self.assertTrue(any("head SHA" in error for error in errors), errors)
        self.assertTrue(any("canonical target" in error for error in errors), errors)
        self.assertTrue(any("scenario IDs" in error for error in errors), errors)

    def test_required_oracle_cannot_be_not_required_for_stage(self) -> None:
        actual = receipt()
        actual["oracles"]["process"] = {
            "outcome": "not_required_for_stage",
            "reason": "incorrect downgrade",
            "observations": [],
        }

        errors = validate_case_receipt_for_gate(actual, expectation())

        self.assertTrue(any("required oracle process" in error for error in errors), errors)

    def test_stage_not_required_oracle_cannot_claim_passed(self) -> None:
        actual = receipt()
        actual["oracles"]["ui"] = {
            "outcome": "passed",
            "observations": ["desktop_completion_visible"],
        }

        errors = validate_case_receipt_for_gate(actual, expectation())

        self.assertTrue(any("oracle ui must be not_required_for_stage" in error for error in errors), errors)

    def test_invalid_policy_cannot_make_every_oracle_not_required(self) -> None:
        bad_expectation = expectation()
        bad_expectation["oracle_policy"] = {
            name: "not_required_for_stage"
            for name in ("ui", "durable_state", "process", "side_effects", "delivery")
        }
        actual = receipt()
        actual["oracles"] = {
            name: {
                "outcome": "not_required_for_stage",
                "reason": "incorrect_policy_downgrade",
                "observations": [],
            }
            for name in ("ui", "durable_state", "process", "side_effects", "delivery")
        }
        actual["outcome"] = "passed"
        actual["run_id"] = case_receipt_run_id(actual, bad_expectation["oracle_policy"])

        errors = validate_case_receipt_for_gate(actual, bad_expectation)

        self.assertTrue(any("invalid oracle policy" in error for error in errors), errors)

    def test_any_failed_oracle_and_cleanup_leak_fail_closed(self) -> None:
        actual = receipt()
        actual["oracles"]["side_effects"]["outcome"] = "failed"
        actual["cleanup"] = {
            "outcome": "failed",
            "leaked_resources": 2,
            "cleanup_attempted": True,
            "orphan_sweep_performed": True,
            "failure_code": "fixture_resources_remain",
        }

        errors = validate_case_receipt_for_gate(actual, expectation())

        self.assertTrue(any("oracle side_effects failed" in error for error in errors), errors)
        self.assertTrue(any("cleanup did not pass" in error for error in errors), errors)
        self.assertTrue(any("leaked resources" in error for error in errors), errors)

    def test_release_requires_every_oracle_and_exact_artifact_identity(self) -> None:
        actual = receipt("release_artifact")

        errors = validate_case_receipt_for_gate(actual, expectation("release_artifact"))

        self.assertTrue(any("required oracle ui" in error for error in errors), errors)
        actual["oracles"]["ui"] = {"outcome": "passed", "observations": ["desktop_completion_visible"]}
        actual["outcome"] = "passed"
        actual["diagnostic_code"] = "case_observations_accepted"
        actual["run_id"] = case_receipt_run_id(
            actual, expectation("release_artifact")["oracle_policy"]
        )
        self.assertEqual(
            validate_case_receipt_for_gate(actual, expectation("release_artifact")),
            [],
        )

        actual["build_identity"]["artifact_sha256"] = None
        self.assertTrue(
            any(
                "release artifact digest" in error
                for error in validate_case_receipt_for_gate(actual, expectation("release_artifact"))
            )
        )

    def test_fixture_and_implementation_digests_are_bound_to_expectation(self) -> None:
        actual = receipt()
        actual["implementation"]["driver_sha256"] = "f" * 64
        actual["fixture"]["manifest_sha256"] = "0" * 64

        errors = validate_case_receipt_for_gate(actual, expectation())

        self.assertTrue(any("driver digest" in error for error in errors), errors)
        self.assertTrue(any("fixture manifest digest" in error for error in errors), errors)

    def test_runner_and_fixture_projection_are_bound(self) -> None:
        actual = receipt()
        actual["runner"] = {"name": "macos-14", "os": "macos", "arch": "aarch64"}
        actual["fixture"]["manifest"]["seed"] = "e2e-001-tampered"

        errors = validate_case_receipt_for_gate(actual, expectation())

        self.assertTrue(any("runner does not match" in error for error in errors), errors)
        self.assertTrue(any("fixture manifest digest contradicts" in error for error in errors), errors)

    def test_release_rejects_wrong_raw_and_arbitrary_artifact_identity(self) -> None:
        raw = raw_smoke()
        raw["build_git_sha"] = "f" * 40
        declared = build_identity(True)
        declared["artifact_sha256"] = "9" * 64
        declared["version"] = "totally-wrong"
        actual = build_e2e001_case_receipt(
            raw,
            expectation("release_artifact"),
            fixture_manifest(),
            runner=runner(),
            build_identity=declared,
        )

        errors = validate_case_receipt_for_gate(actual, expectation("release_artifact"))

        self.assertTrue(any("build identity does not match" in error for error in errors), errors)
        self.assertTrue(any("required oracle delivery" in error for error in errors), errors)

    def test_wrong_raw_case_identity_and_weak_exit_status_do_not_pass(self) -> None:
        raw = raw_smoke()
        raw["case_id"] = "E2E-999"
        raw["supervisor_hard_kill_issued"] = False
        raw["phase_one_was_hard_killed"] = True
        actual = build_e2e001_case_receipt(
            raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )

        self.assertEqual(actual["oracles"]["durable_state"]["outcome"], "failed")
        self.assertEqual(actual["oracles"]["process"]["outcome"], "failed")
        self.assertNotEqual(validate_case_receipt_for_gate(actual, expectation()), [])

    def test_failed_smoke_still_yields_a_redacted_structural_receipt(self) -> None:
        raw = raw_smoke()
        raw["ok"] = False
        raw["error"] = "token=secret at /Users/alice/private-project"
        actual = build_e2e001_case_receipt(
            raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )

        self.assertEqual(validate_case_receipt_structure(actual), [])
        self.assertEqual(actual["outcome"], "failed")
        self.assertEqual(actual["diagnostic_code"], "unattended_smoke_failed")
        self.assertNotIn("secret", str(actual))
        self.assertNotIn("/Users/", str(actual))

    def test_bad_unattended_counts_are_mapped_to_named_failed_oracles(self) -> None:
        raw = raw_smoke()
        raw["user_message_count"] = 2
        raw["live_owner_count"] = 1
        raw["side_effect_receipt_count"] = 2
        actual = build_e2e001_case_receipt(
            raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )

        self.assertEqual(actual["oracles"]["durable_state"]["outcome"], "failed")
        self.assertEqual(actual["oracles"]["process"]["outcome"], "failed")
        self.assertEqual(actual["oracles"]["side_effects"]["outcome"], "failed")
        self.assertNotEqual(validate_case_receipt_for_gate(actual, expectation()), [])

    def test_cleanup_failure_is_retained_even_when_smoke_oracles_pass(self) -> None:
        raw = raw_smoke()
        raw["cleanup_ok"] = False
        actual = build_e2e001_case_receipt(
            raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )

        self.assertEqual(actual["cleanup"]["outcome"], "failed")
        self.assertEqual(actual["outcome"], "failed")
        self.assertTrue(
            any("cleanup did not pass" in error for error in validate_case_receipt_for_gate(actual, expectation()))
        )

    def test_cleanup_evidence_is_observed_not_fabricated(self) -> None:
        raw = raw_smoke()
        raw.pop("leaked_resource_count")
        raw["cleanup_attempted"] = False
        raw["orphan_sweep_performed"] = False
        actual = build_e2e001_case_receipt(
            raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )

        self.assertIsNone(actual["cleanup"]["leaked_resources"])
        self.assertFalse(actual["cleanup"]["cleanup_attempted"])
        self.assertFalse(actual["cleanup"]["orphan_sweep_performed"])
        self.assertEqual(actual["cleanup"]["outcome"], "failed")

    def test_receipt_privacy_scan_rejects_absolute_paths(self) -> None:
        actual = receipt()
        actual["diagnostic_code"] = "failed in C:\\Users\\alice\\private-project"

        errors = validate_case_receipt_structure(actual)

        self.assertTrue(any("absolute path" in error for error in errors), errors)

    def test_diagnostic_codes_reject_secret_words_and_raw_unknowns_do_not_change_digest(self) -> None:
        first_raw = raw_smoke()
        first_raw["error"] = "token=secret at /Users/alice/private"
        first_raw["unknown_payload"] = {"password": "hunter2"}
        second_raw = raw_smoke()
        second_raw["error"] = "another private error"
        first = build_e2e001_case_receipt(
            first_raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )
        second = build_e2e001_case_receipt(
            second_raw,
            expectation(),
            fixture_manifest(),
            runner=runner(),
            build_identity=build_identity(),
        )

        self.assertEqual(first["evidence_sha256"], second["evidence_sha256"])
        first["diagnostic_code"] = "token_secret"
        self.assertTrue(
            any("diagnostic code" in error for error in validate_case_receipt_structure(first))
        )

    def test_run_id_binds_runner_build_identity_policy_and_oracles(self) -> None:
        actual = receipt()
        for mutate in (
            lambda value: value["runner"].update({"arch": "aarch64"}),
            lambda value: value["build_identity"].update({"source_sha": "f" * 40}),
            lambda value: value["oracles"]["ui"].update({"reason": "another_reason"}),
        ):
            changed = copy.deepcopy(actual)
            mutate(changed)
            errors = validate_case_receipt_for_gate(changed, expectation())
            self.assertTrue(any("run_id" in error for error in errors), errors)

    def test_passed_oracle_observations_are_exactly_bound(self) -> None:
        for observations in (
            ["totally_unrelated"],
            ["AKIAIOSFODNN7EXAMPLE"],
            ["xoxb-example-secret"],
            ["sk-proj-example-secret"],
            ["github_pat_example_secret"],
        ):
            actual = receipt()
            actual["oracles"]["process"]["observations"] = observations
            actual["run_id"] = case_receipt_run_id(
                actual, expectation()["oracle_policy"]
            )

            errors = validate_case_receipt_for_gate(actual, expectation())

            self.assertTrue(
                any("process observations" in error for error in errors), errors
            )

    def test_not_required_oracle_reason_is_exactly_bound(self) -> None:
        actual = receipt()
        actual["oracles"]["ui"]["reason"] = "plausible_but_untrusted_reason"
        actual["run_id"] = case_receipt_run_id(
            actual, expectation()["oracle_policy"]
        )

        errors = validate_case_receipt_for_gate(actual, expectation())

        self.assertTrue(any("not-required reason" in error for error in errors), errors)

    def test_common_credential_shapes_are_rejected_recursively(self) -> None:
        for credential in (
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-123456789-abcdef",
            "sk-proj-abcdefghijklmnopqrstuvwxyz",
            "github_pat_abcdefghijklmnopqrstuvwxyz",
        ):
            actual = receipt()
            actual["runner"]["name"] = credential

            errors = validate_case_receipt_structure(actual)

            self.assertTrue(any("secret-like value" in error for error in errors), errors)

    def test_malformed_nested_types_return_errors_instead_of_raising(self) -> None:
        malformed_scenarios = receipt()
        malformed_scenarios["scenario_ids"] = [{}]
        malformed_fixture = receipt()
        malformed_fixture["fixture"]["manifest"]["capabilities"] = [
            {"id": {}, "schema_version": 1}
        ]
        malformed_oracle = receipt()
        malformed_oracle["oracles"]["process"] = []

        self.assertNotEqual(validate_case_receipt_structure(malformed_scenarios), [])
        self.assertNotEqual(validate_case_receipt_structure(malformed_fixture), [])
        self.assertNotEqual(validate_case_receipt_for_gate(malformed_oracle, expectation()), [])
        self.assertNotEqual(validate_case_receipt_for_gate([], expectation()), [])
        self.assertNotEqual(validate_case_receipt_for_gate(receipt(), []), [])

    def test_structure_rejects_passed_cleanup_with_leaks_and_failed_oracle(self) -> None:
        leaked = receipt()
        leaked["cleanup"]["leaked_resources"] = 3
        failed_oracle = receipt()
        failed_oracle["oracles"]["process"] = {
            "outcome": "failed",
            "observations": [],
            "failure_code": "process_assertions_failed",
        }

        self.assertTrue(
            any("passed cleanup" in error for error in validate_case_receipt_structure(leaked))
        )
        self.assertTrue(
            any("passed receipt" in error for error in validate_case_receipt_structure(failed_oracle))
        )


if __name__ == "__main__":
    unittest.main()
