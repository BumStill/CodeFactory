from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
import zipfile
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from tools.governance import validate_scenario_test_governance as scenario_governance
from tools.governance import scenario_execution

from tools.governance.run_scenario_harness_gate import (
    TRUST_ROOT_FILES,
    validate_trust_root_immutability,
)
from tools.governance.scenario_execution import (
    build_execution_plan,
    validate_aggregate_receipt,
)
from tools.governance.validate_scenario_test_governance import (
    _automation_gate_stages,
    _changed_files,
    _impacted_scenarios,
    _is_version_manifest_only_patch,
    load_registry,
    validate_change_contract,
    validate_gate_readiness,
    validate_impacted_execution,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = REPO_ROOT / "docs" / "testing" / "scenario-registry.json"


class ScenarioRegistryTests(unittest.TestCase):
    def test_delegated_binary_binding_requires_the_whole_explicit_call_chain(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github/workflows/release.yml"
            verifier = root / "scripts/verify-release.sh"
            workflow.parent.mkdir(parents=True)
            verifier.parent.mkdir(parents=True)
            policy = {
                "target_bindings": {
                    "binary": {
                        "workflows": [".github/workflows/release.yml"],
                        "workflow_stages": {
                            ".github/workflows/release.yml": ["release_artifact"]
                        },
                        "delegated_scripts": {
                            ".github/workflows/release.yml": [
                                "scripts/verify-release.sh"
                            ]
                        },
                    }
                }
            }
            workflow.write_text("run: scripts/verify-release.sh\n", encoding="utf-8")
            verifier.write_text(
                '"$EXECUTABLE" --browser-chrome-attach-smoke "$RECEIPT"\n',
                encoding="utf-8",
            )

            self.assertEqual(
                _automation_gate_stages(
                    "binary:--browser-chrome-attach-smoke", policy, root
                ),
                {"release_artifact"},
            )

            workflow.write_text("run: scripts/another-verifier.sh\n", encoding="utf-8")
            self.assertEqual(
                _automation_gate_stages(
                    "binary:--browser-chrome-attach-smoke", policy, root
                ),
                set(),
            )
            workflow.write_text("run: scripts/verify-release.sh\n", encoding="utf-8")
            verifier.write_text("echo no exact binary gate\n", encoding="utf-8")
            self.assertEqual(
                _automation_gate_stages(
                    "binary:--browser-chrome-attach-smoke", policy, root
                ),
                set(),
            )

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


class VersionManifestImpactTests(unittest.TestCase):
    files = [
        "package.json",
        "src-tauri/Cargo.lock",
        "src-tauri/Cargo.toml",
        "src-tauri/tauri.conf.json",
    ]

    @staticmethod
    def patch(new_version: str = "1.81.35") -> str:
        sections = []
        for path in VersionManifestImpactTests.files:
            key = '  "version": ' if path.endswith(".json") else "version = "
            old = f'{key}"1.81.34"'
            new = f'{key}"{new_version}"'
            sections.append(
                f"diff --git a/{path} b/{path}\n"
                f"--- a/{path}\n"
                f"+++ b/{path}\n"
                "@@ -1 +1 @@\n"
                f"-{old}\n+{new}\n"
            )
        return "".join(sections)

    def test_exact_synchronized_version_bump_has_no_scenario_impact(self) -> None:
        self.assertTrue(_is_version_manifest_only_patch(self.files, self.patch()))

    def test_manifest_change_with_a_script_edit_remains_a_global_change(self) -> None:
        patch = self.patch() + (
            "diff --git a/package.json b/package.json\n"
            "--- a/package.json\n+++ b/package.json\n@@ -9 +9 @@\n"
            '-    "test": "old"\n+    "test": "new"\n'
        )
        self.assertFalse(_is_version_manifest_only_patch(self.files, patch))

    def test_mismatched_target_versions_fail_closed(self) -> None:
        patch = self.patch().replace(
            '+version = "1.81.35"', '+version = "1.81.36"', 1
        )
        self.assertFalse(_is_version_manifest_only_patch(self.files, patch))

    def test_partial_manifest_set_is_not_exempt(self) -> None:
        self.assertFalse(
            _is_version_manifest_only_patch(self.files[:-1], self.patch())
        )


if __name__ == "__main__":
    unittest.main()


class ImpactedExecutionTests(unittest.TestCase):
    """The PR gate enforces what a change can reach, not the whole catalog."""

    def setUp(self) -> None:
        self.registry = load_registry(REGISTRY_PATH)

    def test_non_product_change_is_not_gated(self) -> None:
        self.assertEqual(
            validate_impacted_execution(
                self.registry, ["docs/testing/scenario-registry.json"], "pull_request", REPO_ROOT
            ),
            [],
        )

    def test_impacted_scenario_without_a_complex_e2e_case_passes(self) -> None:
        errors = validate_impacted_execution(
            self.registry, ["src/components/MessageInput.tsx"], "pull_request", REPO_ROOT
        )
        self.assertEqual(errors, [])

    def test_unrelated_catalog_debt_does_not_block_a_change_that_cannot_reach_it(self) -> None:
        # validate_gate_readiness reports the whole catalog; running that on every
        # product PR made the repository unmergeable. The per-change gate must not
        # inherit it.
        catalog = validate_gate_readiness(self.registry, "pull_request")
        self.assertTrue(catalog, "expected the catalog sweep to still report debt")
        reported = " ".join(
            validate_impacted_execution(
                self.registry, ["src/components/MessageInput.tsx"], "pull_request", REPO_ROOT
            )
        )
        for case_id in ("E2E-002", "E2E-003"):
            self.assertNotIn(case_id, reported)

    def test_release_batch_does_not_expand_a_release_workflow_change_to_the_catalog(self) -> None:
        impacted = _impacted_scenarios(
            [".github/workflows/release.yml"],
            self.registry,
        )
        explicitly_mapped = {
            scenario["id"]
            for scenario in self.registry["scenarios"]
            if ".github/workflows/release.yml" in scenario.get("change_patterns", [])
        }
        self.assertEqual(impacted, explicitly_mapped)
        self.assertNotEqual(
            impacted,
            {scenario["id"] for scenario in self.registry["scenarios"]},
        )

    def test_auto_release_workflow_has_an_explicit_scenario_mapping(self) -> None:
        impacted = _impacted_scenarios(
            [".github/workflows/auto-release.yml"],
            self.registry,
        )
        self.assertEqual(impacted, {"HLT-001", "HLT-004", "UI-005"})

    def test_unrelated_release_e2e_debt_does_not_block_the_affected_release_slice(self) -> None:
        registry = {
            "scenarios": [
                {
                    "id": "X-001",
                    "change_patterns": ["src/a.ts"],
                    "gates": ["release_artifact"],
                    "automated_by": ["binary:--history-session-smoke"],
                },
                {
                    "id": "Y-001",
                    "change_patterns": ["src/b.ts"],
                    "gates": ["release_artifact"],
                    "automated_by": [],
                },
            ],
            "complex_e2e_cases": [
                {
                    "id": "E2E-A",
                    "covers": ["X-001"],
                    "change_patterns": ["src/a.ts"],
                    "execution": {"release_artifact": "exact release executable"},
                    "automated_by": ["binary:--history-session-smoke"],
                },
                {
                    "id": "E2E-B",
                    "covers": ["Y-001"],
                    "change_patterns": ["src/b.ts"],
                    "execution": {"release_artifact": "missing exact artifact test"},
                    "automated_by": [],
                },
            ],
            "gate_policy": self.registry["gate_policy"],
        }
        errors = validate_impacted_execution(
            registry,
            ["src/a.ts"],
            "release_artifact",
            REPO_ROOT,
            expand_global_files=False,
        )
        self.assertEqual(errors, [])

    def test_affected_release_case_without_a_bound_exact_artifact_target_fails(self) -> None:
        registry = {
            "scenarios": [
                {
                    "id": "X-001",
                    "change_patterns": ["src/a.ts"],
                    "gates": ["release_artifact"],
                    "automated_by": ["rust:invalid_plan_revision_never_poison_receipts_or_the_next_tool"],
                }
            ],
            "complex_e2e_cases": [
                {
                    "id": "E2E-X",
                    "covers": ["X-001"],
                    "change_patterns": ["src/a.ts"],
                    "execution": {"release_artifact": "exact release executable"},
                    "automated_by": [
                        "rust:invalid_plan_revision_never_poison_receipts_or_the_next_tool"
                    ],
                }
            ],
            "gate_policy": self.registry["gate_policy"],
        }
        errors = validate_impacted_execution(
            registry,
            ["src/a.ts"],
            "release_artifact",
            REPO_ROOT,
            expand_global_files=False,
        )
        self.assertTrue(
            any("E2E-X" in error and "release_artifact" in error for error in errors),
            errors,
        )

    def test_unmapped_release_product_file_fails_closed(self) -> None:
        errors = validate_impacted_execution(
            self.registry,
            ["src-tauri/src/unmapped_release_surface.rs"],
            "release_artifact",
            REPO_ROOT,
            expand_global_files=False,
            fail_on_unmapped=True,
        )
        self.assertTrue(any("coverage gap" in error for error in errors), errors)

    def test_a_case_covering_an_impacted_scenario_without_running_automation_fails(self) -> None:
        registry = {
            "scenarios": [
                {
                    "id": "X-001",
                    "change_patterns": ["src/thing.ts"],
                    "gates": ["pull_request"],
                    "automated_by": ["path:src/thing.test.ts"],
                }
            ],
            "complex_e2e_cases": [
                {
                    "id": "E2E-X",
                    "covers": ["X-001"],
                    "change_patterns": ["src/thing.ts"],
                    "execution": {"pull_request": "designed only"},
                    "automation_status": "designed",
                    "automated_by": [],
                    "remaining_gaps": ["no automation at all"],
                    "pull_request_gate": {
                        "status": "designed",
                        "required_targets": [],
                        "remaining_gaps": ["no automation at all"],
                    },
                }
            ],
            "gate_policy": self.registry["gate_policy"],
        }
        errors = validate_impacted_execution(registry, ["src/thing.ts"], "pull_request", REPO_ROOT)
        self.assertTrue(any("E2E-X" in error for error in errors), errors)

    def test_partially_automated_impacted_case_fails_even_when_one_target_runs(self) -> None:
        registry = {
            "scenarios": [
                {
                    "id": "X-001",
                    "change_patterns": ["src/thing.ts"],
                    "gates": ["pull_request"],
                    "automated_by": ["pnpm:test:startup-session:headless"],
                }
            ],
            "complex_e2e_cases": [
                {
                    "id": "E2E-X",
                    "covers": ["X-001"],
                    "change_patterns": ["src/thing.ts"],
                    "execution": {"pull_request": "partially automated"},
                    "automation_status": "partially_implemented",
                    "automated_by": ["pnpm:test:startup-session:headless"],
                    "remaining_gaps": ["release artifact not automated"],
                    "pull_request_gate": {
                        "status": "partially_implemented",
                        "required_targets": ["pnpm:test:startup-session:headless"],
                        "remaining_gaps": ["one PR oracle is still missing"],
                    },
                }
            ],
            "gate_policy": self.registry["gate_policy"],
        }
        errors = validate_impacted_execution(
            registry,
            ["src/thing.ts"],
            "pull_request",
            REPO_ROOT,
        )
        self.assertTrue(
            any("E2E-X" in error and "not implemented" in error for error in errors),
            errors,
        )

    def test_recovery_control_change_is_not_blocked_by_workspace_or_browser_e2e_debt(self) -> None:
        errors = validate_impacted_execution(
            self.registry,
            ["src-tauri/src/agent/objective.rs"],
            "pull_request",
            REPO_ROOT,
        )
        self.assertEqual(errors, [])

    def test_completed_browser_lifecycle_slice_passes_when_all_direct_targets_run(self) -> None:
        errors = validate_impacted_execution(
            self.registry,
            ["src-tauri/src/tools/browser_session.rs"],
            "pull_request",
            REPO_ROOT,
        )
        self.assertEqual(errors, [])

    def test_implemented_case_requires_every_declared_pr_target_to_run(self) -> None:
        registry = {
            "scenarios": [
                {
                    "id": "X-001",
                    "change_patterns": ["src/thing.ts"],
                    "gates": ["pull_request"],
                    "automated_by": ["path:src/thing.test.ts"],
                }
            ],
            "complex_e2e_cases": [
                {
                    "id": "E2E-X",
                    "covers": ["X-001"],
                    "change_patterns": ["src/thing.ts"],
                    "execution": {"pull_request": "real process smoke"},
                    "automation_status": "partially_implemented",
                    "automated_by": ["path:src/thing.test.ts"],
                    "remaining_gaps": ["release artifact not automated"],
                    "pull_request_gate": {
                        "status": "implemented",
                        "required_targets": [
                            "path:src/thing.test.ts",
                            "binary:--missing-real-process-smoke",
                        ],
                        "remaining_gaps": [],
                    },
                }
            ],
            "gate_policy": self.registry["gate_policy"],
        }
        errors = validate_impacted_execution(
            registry, ["src/thing.ts"], "pull_request", REPO_ROOT
        )
        self.assertTrue(
            any("--missing-real-process-smoke" in error for error in errors), errors
        )


class TrustRootScopeTests(unittest.TestCase):
    """A PR may not redefine either its judge or the workflow proving execution."""

    def test_required_execution_workflows_remain_in_the_trust_root(self) -> None:
        self.assertIn(".github/workflows/ci.yml", TRUST_ROOT_FILES)
        self.assertIn(".github/workflows/release.yml", TRUST_ROOT_FILES)
        self.assertIn("tools/governance/run_scenario_harness_gate.py", TRUST_ROOT_FILES)

    def _tree(self, root: Path) -> None:
        for relative in TRUST_ROOT_FILES:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("original\n", encoding="utf-8")

    def test_amending_ci_or_the_runner_requires_external_governance_bootstrap(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            base = Path(raw)
            policy, candidate = base / "policy", base / "candidate"
            self._tree(policy)
            self._tree(candidate)

            (candidate / ".github/workflows/ci.yml").write_text("changed\n", encoding="utf-8")
            errors = validate_trust_root_immutability(candidate, policy)
            self.assertTrue(any("ci.yml" in error for error in errors), errors)

            (candidate / ".github/workflows/ci.yml").write_text("original\n", encoding="utf-8")
            (candidate / "tools/governance/run_scenario_harness_gate.py").write_text(
                "changed\n", encoding="utf-8"
            )
            errors = validate_trust_root_immutability(candidate, policy)
            self.assertTrue(
                any("run_scenario_harness_gate.py" in error for error in errors), errors
            )


class AffectedScenarioExecutionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = load_registry(REGISTRY_PATH)

    def test_only_impacted_scenarios_and_e2e_targets_enter_the_plan(self) -> None:
        plan = build_execution_plan(
            self.registry,
            ["src-tauri/src/tools/browser_session.rs"],
            base_sha="a" * 40,
            head_sha="b" * 40,
        )

        self.assertIn("RTE-003", plan["scenario_ids"])
        self.assertIn("E2E-008", plan["e2e_ids"])
        self.assertIn(
            "binary:--browser-chrome-attach-smoke", plan["required_targets"]
        )
        self.assertIn("macos-14", plan["runners"])
        self.assertIn("windows-latest", plan["runners"])
        self.assertNotIn("E2E-011", plan["e2e_ids"])

    def test_unrelated_change_has_no_execution_runner(self) -> None:
        plan = build_execution_plan(
            self.registry,
            ["docs/notes.md"],
            base_sha="a" * 40,
            head_sha="b" * 40,
        )

        self.assertEqual(plan["scenario_ids"], [])
        self.assertEqual(plan["e2e_ids"], [])
        self.assertEqual(plan["required_targets"], [])
        self.assertEqual(plan["runners"], {})

    def test_receipt_must_match_both_shas_and_every_required_target(self) -> None:
        plan = {
            "schema_version": 1,
            "base_sha": "a" * 40,
            "head_sha": "b" * 40,
            "required_targets": ["rust:first", "binary:--real-smoke"],
        }
        valid = {
            "schema_version": 1,
            "base_sha": "a" * 40,
            "head_sha": "b" * 40,
            "targets": [
                {"target": "rust:first", "outcome": "passed"},
                {"target": "binary:--real-smoke", "outcome": "passed"},
            ],
        }
        self.assertEqual(validate_aggregate_receipt(plan, valid), [])

        missing = json.loads(json.dumps(valid))
        missing["targets"].pop()
        self.assertTrue(
            any("missing successful execution receipt" in error for error in validate_aggregate_receipt(plan, missing))
        )

        wrong_head = json.loads(json.dumps(valid))
        wrong_head["head_sha"] = "c" * 40
        self.assertTrue(
            any("head SHA" in error for error in validate_aggregate_receipt(plan, wrong_head))
        )

    def test_execution_and_receipt_surfaces_are_part_of_the_trust_root(self) -> None:
        self.assertIn(".github/workflows/scenario-execution.yml", TRUST_ROOT_FILES)
        self.assertIn("tools/governance/scenario_execution.py", TRUST_ROOT_FILES)

    def test_one_trusted_context_aggregates_an_unprivileged_execution_workflow(self) -> None:
        execution = (REPO_ROOT / ".github/workflows/scenario-execution.yml").read_text(
            encoding="utf-8"
        )
        gate = (REPO_ROOT / ".github/workflows/scenario-gate.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("  pull_request:\n", execution)
        self.assertNotIn("pull_request_target:", execution)
        self.assertIn("permissions:\n  contents: read", execution)
        self.assertIn("needs.plan.outputs.run_windows == 'true'", execution)
        self.assertIn("needs.plan.outputs.run_macos == 'true'", execution)
        self.assertIn("actions: read", gate)
        self.assertIn("scenario_execution.py await-github", gate)
        self.assertNotIn("scenario-gate-policy", execution + gate)

    @patch("tools.governance.scenario_execution._github_bytes")
    @patch("tools.governance.scenario_execution._github_json")
    def test_trusted_gate_accepts_only_the_exact_head_workflow_artifact(
        self, github_json, github_bytes
    ) -> None:
        plan = {
            "schema_version": 1,
            "base_sha": "a" * 40,
            "head_sha": "b" * 40,
            "required_targets": ["rust:first"],
            "blockers": [],
        }
        github_json.side_effect = [
            {
                "workflow_runs": [
                    {
                        "id": 1,
                        "event": "pull_request",
                        "head_sha": "c" * 40,
                        "status": "completed",
                        "conclusion": "success",
                    },
                    {
                        "id": 2,
                        "run_attempt": 1,
                        "event": "pull_request",
                        "head_sha": "b" * 40,
                        "status": "completed",
                        "conclusion": "success",
                    },
                ]
            },
            {
                "artifacts": [
                    {
                        "name": "affected-scenario-receipt-" + "b" * 40,
                        "expired": False,
                        "archive_download_url": "https://api.github.test/artifact",
                    }
                ]
            },
        ]
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as bundle:
            bundle.writestr(
                "scenario-execution-receipt.json",
                json.dumps(
                    {
                        "schema_version": 1,
                        "base_sha": "a" * 40,
                        "head_sha": "b" * 40,
                        "targets": [{"target": "rust:first", "outcome": "passed"}],
                    }
                ),
            )
        github_bytes.return_value = archive.getvalue()

        self.assertEqual(
            scenario_execution.await_github_receipt(
                plan,
                repository="owner/repo",
                workflow="scenario-execution.yml",
                token="test-token",
                timeout_seconds=1,
            ),
            [],
        )
        self.assertIn("/actions/runs/2/artifacts", github_json.call_args_list[1].args[0])
