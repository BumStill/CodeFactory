from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from tools.governance import validate_scenario_test_governance as scenario_governance

from tools.governance.validate_scenario_test_governance import (
    _changed_files,
    load_registry,
    validate_change_contract,
    validate_gate_readiness,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = REPO_ROOT / "docs" / "testing" / "scenario-registry.json"


class ScenarioRegistryTests(unittest.TestCase):
    def test_repository_registry_is_valid_and_unifies_existing_surfaces(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        self.assertEqual(validate_registry(registry, REPO_ROOT), [])

        ids = {scenario["id"] for scenario in registry["scenarios"]}
        self.assertTrue(
            {
                "HLT-001",
                "HLT-002",
                "HLT-003",
                "HLT-004",
                "CXD-001",
                "CXD-002",
                "UI-001",
                "UI-011",
                "RTE-001",
            }.issubset(ids)
        )
        self.assertGreaterEqual(len(registry["scenarios"]), 18)
        self.assertGreaterEqual(len(registry["complex_e2e_cases"]), 6)

    def test_duplicate_ids_and_unknown_categories_are_rejected(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        broken = json.loads(json.dumps(registry))
        broken["scenarios"][1]["id"] = broken["scenarios"][0]["id"]
        broken["scenarios"][2]["category"] = "invented"
        errors = validate_registry(broken, REPO_ROOT)
        self.assertTrue(any("duplicate scenario id" in error for error in errors))
        self.assertTrue(any("unknown category" in error for error in errors))

    def test_complex_e2e_requires_cross_layer_oracles(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        broken = json.loads(json.dumps(registry))
        broken["complex_e2e_cases"][0]["oracles"].pop("durable_state")
        errors = validate_registry(broken, REPO_ROOT)
        self.assertTrue(any("durable_state" in error for error in errors))

    def test_complex_e2e_must_name_automation_that_exists(self) -> None:
        # A complex E2E is gated exactly like a scenario, so it must be held to
        # the same bar: claiming a gate while automating nothing is how a
        # "covered" case silently stops covering anything.
        registry = load_registry(REGISTRY_PATH)
        case = registry["complex_e2e_cases"][0]

        case["automated_by"] = []
        errors = validate_registry(registry, REPO_ROOT)
        self.assertTrue(
            any("must name at least one automation target" in error for error in errors),
            errors,
        )

        case["automated_by"] = ["rust:this_test_does_not_exist_anywhere"]
        errors = validate_registry(registry, REPO_ROOT)
        self.assertTrue(
            any("points at missing automation" in error for error in errors),
            errors,
        )

    def test_automation_status_may_not_claim_more_than_it_runs(self) -> None:
        # Guards the exact rot found on 2026-08-24: cases carried
        # pull_request/nightly/release gates and a "partially_implemented"
        # status with zero automated_by entries and no recorded gaps, so the
        # registry read as coverage while nothing ran and nothing said so.
        registry = load_registry(REGISTRY_PATH)
        case = registry["complex_e2e_cases"][0]

        case["automation_status"] = "partially_implemented"
        case["automated_by"] = []
        errors = validate_registry(registry, REPO_ROOT)
        self.assertTrue(
            any("must name at least one automation target" in e for e in errors), errors
        )

        case["automated_by"] = ["rust:invalid_plan_revision_never_poison_receipts_or_the_next_tool"]
        case["remaining_gaps"] = []
        errors = validate_registry(registry, REPO_ROOT)
        self.assertTrue(
            any("must record remaining_gaps" in e for e in errors), errors
        )

        case["automation_status"] = "implemented"
        case["remaining_gaps"] = ["still missing something"]
        errors = validate_registry(registry, REPO_ROOT)
        self.assertTrue(
            any("implemented but still records remaining_gaps" in e for e in errors),
            errors,
        )

        case["automation_status"] = "designed"
        errors = validate_registry(registry, REPO_ROOT)
        self.assertTrue(
            any("only designed but already names automation" in e for e in errors),
            errors,
        )

    def test_a_rust_target_must_be_a_real_test_function(self) -> None:
        # Substring matching lets a declaration survive the test being renamed
        # or deleted, as long as the name still appears anywhere in any .rs
        # file — a comment, a string literal, a failure code. That is exactly
        # how a scenario keeps reporting coverage after its test is gone.
        from tools.governance.validate_scenario_test_governance import _automation_exists

        # A real failure-code literal that appears in .rs sources but is not a test.
        self.assertFalse(
            _automation_exists("rust:provider_external_state_uncertain", REPO_ROOT)
        )
        # A genuine test function still resolves.
        self.assertTrue(
            _automation_exists(
                "rust:invalid_plan_revision_never_poison_receipts_or_the_next_tool",
                REPO_ROOT,
            )
        )

    def test_registry_loader_reports_invalid_json(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "registry.json"
            path.write_text("{", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "invalid scenario registry JSON"):
                load_registry(path)

    def test_registry_loader_rejects_a_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "actual.json"
            target.write_text("{}", encoding="utf-8")
            link = root / "scenario-registry.json"
            link.symlink_to(target)
            with self.assertRaisesRegex(ValueError, "must not be a symlink"):
                load_registry(link)

    def test_raw_production_identity_fields_are_rejected(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        broken = json.loads(json.dumps(registry))
        broken["complex_e2e_cases"][0]["fixture"]["session_id"] = "real-id"
        errors = validate_registry(broken, REPO_ROOT)
        self.assertTrue(any("forbidden production data key" in error for error in errors))

    def test_release_gate_requires_exact_artifact_evidence_level(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        broken = json.loads(json.dumps(registry))
        broken["scenarios"][0]["required_evidence"].remove("L4")
        errors = validate_registry(broken, REPO_ROOT)
        self.assertTrue(any("release_artifact gate requires L4" in error for error in errors))

    def test_every_automation_target_is_bound_to_a_declared_gate_stage(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        broken = json.loads(json.dumps(registry))
        rte = next(item for item in broken["scenarios"] if item["id"] == "RTE-003")
        rte["gates"] = ["pull_request"]
        errors = validate_registry(broken, REPO_ROOT)
        self.assertTrue(
            any(
                "RTE-003" in error
                and "--browser-chrome-attach-smoke" in error
                and "declared hard-gate stage" in error
                for error in errors
            ),
            errors,
        )


class ScenarioChangeContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = load_registry(REGISTRY_PATH)

    def test_critical_path_change_requires_all_impacted_p0_scenarios(self) -> None:
        errors = validate_change_contract(
            title="fix(chat): keep historical sessions resumable",
            body="Scenario-Test: HLT-003",
            files=["src-tauri/src/commands/chat.rs"],
            registry=self.registry,
        )
        self.assertTrue(any("HLT-004" in error for error in errors))

    def test_declared_impacted_scenarios_pass(self) -> None:
        self.assertEqual(
            validate_change_contract(
                title="fix(chat): keep historical sessions resumable",
                body="Scenario-Test: RTE-004, HLT-003, HLT-004, HLT-005, CXD-001",
                files=["src-tauri/src/commands/chat.rs"],
                registry=self.registry,
            ),
            [],
        )

    def test_unknown_scenario_id_is_rejected(self) -> None:
        errors = validate_change_contract(
            title="feat(ui): add a workspace action",
            body="Scenario-Test: UI-999",
            files=["src/pages/Workspace/WorkspacePage.tsx"],
            registry=self.registry,
        )
        self.assertTrue(any("unknown scenario" in error for error in errors))

    def test_non_product_change_does_not_require_a_declaration(self) -> None:
        self.assertEqual(
            validate_change_contract(
                title="docs: explain scenario testing",
                body="",
                files=["docs/testing/scenario-testing-governance.md"],
                registry=self.registry,
            ),
            [],
        )

    def test_test_harness_repairs_do_not_masquerade_as_product_changes(self) -> None:
        self.assertEqual(
            validate_change_contract(
                title="test: repair the usage acceptance oracle",
                body="",
                files=[
                    "scripts/verify-token-usage-headless.mjs",
                    "src/acceptance/usage.tsx",
                ],
                registry=self.registry,
            ),
            [],
        )

    def test_release_workflow_is_a_product_surface_and_fails_closed(self) -> None:
        errors = validate_change_contract(
            title="ci: alter artifact publication",
            body="",
            files=[".github/workflows/release.yml"],
            registry=self.registry,
        )
        self.assertTrue(any("Scenario-Test" in error for error in errors), errors)

    def test_product_change_cannot_bypass_contract_with_a_chore_title(self) -> None:
        errors = validate_change_contract(
            title="chore: quietly change recovery semantics",
            body="Scenario-Test: not-applicable - maintenance only",
            files=["src-tauri/src/agent/objective.rs"],
            registry=self.registry,
        )
        self.assertTrue(any("not-applicable" in error for error in errors), errors)

    def test_p1_product_change_is_a_hard_gate_too(self) -> None:
        errors = validate_change_contract(
            title="refactor workspace behavior",
            body="Scenario-Test: not-applicable - refactor only",
            files=["src/components/MessageInput.tsx"],
            registry=self.registry,
        )
        self.assertTrue(any("UI-001" in error for error in errors), errors)

    def test_unmapped_product_file_fails_closed(self) -> None:
        errors = validate_change_contract(
            title="fix runtime configuration",
            body="Scenario-Test: HLT-001",
            files=["src-tauri/src/new_runtime_surface.rs"],
            registry=self.registry,
        )
        self.assertTrue(any("coverage gap" in error for error in errors), errors)

    def test_missing_base_sha_is_an_error_not_a_warning(self) -> None:
        files, error = _changed_files(REPO_ROOT, "")
        self.assertEqual(files, [])
        self.assertIn("base SHA", error or "")

    def test_push_event_validates_registry_without_requiring_a_pr_body(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            event_path.write_text(
                json.dumps(
                    {
                        "ref": "refs/heads/main",
                        "before": "base-sha",
                        "after": "head-sha",
                    }
                ),
                encoding="utf-8",
            )
            with (
                patch("sys.argv", ["validate_scenario_test_governance.py", "--ci"]),
                patch.dict(
                    os.environ,
                    {
                        "SCENARIO_TEST_EVENT_PATH": str(event_path),
                        "SCENARIO_TEST_BASE_SHA": "base-sha",
                    },
                    clear=False,
                ),
                patch.object(
                    scenario_governance,
                    "_changed_files",
                    return_value=(["src-tauri/src/commands/session.rs"], None),
                ),
                redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(scenario_governance.main(), 0)

    def test_pull_request_event_still_requires_the_scenario_declaration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            event_path.write_text(
                json.dumps(
                    {
                        "pull_request": {
                            "title": "fix: change session behavior",
                            "body": "",
                        }
                    }
                ),
                encoding="utf-8",
            )
            output = io.StringIO()
            with (
                patch("sys.argv", ["validate_scenario_test_governance.py", "--ci"]),
                patch.dict(
                    os.environ,
                    {
                        "SCENARIO_TEST_EVENT_PATH": str(event_path),
                        "SCENARIO_TEST_BASE_SHA": "base-sha",
                    },
                    clear=False,
                ),
                patch.object(
                    scenario_governance,
                    "_changed_files",
                    return_value=(["src-tauri/src/commands/session.rs"], None),
                ),
                redirect_stdout(output),
            ):
                self.assertEqual(scenario_governance.main(), 1)
            self.assertIn("Scenario-Test", output.getvalue())

    def test_unrecognized_ci_event_still_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            event_path.write_text(json.dumps({"workflow_run": {}}), encoding="utf-8")
            output = io.StringIO()
            with (
                patch("sys.argv", ["validate_scenario_test_governance.py", "--ci"]),
                patch.dict(
                    os.environ,
                    {"SCENARIO_TEST_EVENT_PATH": str(event_path)},
                    clear=False,
                ),
                redirect_stdout(output),
            ):
                self.assertEqual(scenario_governance.main(), 1)
            self.assertIn("could not determine CI event type", output.getvalue())


class ScenarioHardGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = load_registry(REGISTRY_PATH)

    def test_every_active_scenario_has_a_pull_request_gate(self) -> None:
        errors = validate_gate_readiness(self.registry, "pull_request")
        self.assertFalse(
            any("manual_canary is not a hard gate" in error for error in errors),
            errors,
        )

    def test_partial_or_designed_complex_e2e_freezes_the_gate(self) -> None:
        errors = validate_gate_readiness(self.registry, "pull_request")
        self.assertTrue(any("E2E-001" in error and "not gate-ready" in error for error in errors), errors)
        self.assertTrue(any("E2E-009" in error and "not gate-ready" in error for error in errors), errors)

    def test_release_gate_rejects_unimplemented_exact_artifact_cases(self) -> None:
        errors = validate_gate_readiness(self.registry, "release_artifact")
        self.assertTrue(any("release_artifact" in error for error in errors), errors)

    def test_promoted_headless_gates_do_not_wait_for_network_idle(self) -> None:
        # Vite/WebSocket/polling pages may never become network-idle. A hard
        # gate must navigate to DOM readiness and then wait on its real UI
        # oracle, otherwise an unrelated long-lived connection creates a
        # deterministic 30-second false failure.
        promoted = (
            "verify-repository-intent-headless.mjs",
            "verify-reconnect-banner-headless.mjs",
            "verify-startup-session-headless.mjs",
            "verify-sidebar-expansion-headless.mjs",
            "verify-image-preview-headless.mjs",
            "verify-streaming-markdown-headless.mjs",
            "verify-permission-mode-headless.mjs",
            "verify-token-usage-headless.mjs",
        )
        offenders = [
            name
            for name in promoted
            if 'waitUntil: "networkidle"'
            in (REPO_ROOT / "scripts" / name).read_text(encoding="utf-8")
        ]
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
