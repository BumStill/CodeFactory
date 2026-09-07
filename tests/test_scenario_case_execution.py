"""Failure-first tests for the trusted E2E-001 execution/aggregate boundary."""

from __future__ import annotations

import copy
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.governance import scenario_execution as execution
from tools.governance.scenario_case_receipt import build_e2e001_case_receipt
from tools.governance import scenario_case_execution as case_execution


ROOT = Path(__file__).resolve().parents[1]
TARGET = "binary:--unattended-long-task-smoke"
BASE = "a" * 40
HEAD = "b" * 40


def plan() -> dict:
    registry = json.loads((ROOT / "docs/testing/scenario-registry.json").read_text())
    result = execution.build_execution_plan(
        registry, ["src-tauri/src/agent/unattended_smoke.rs"],
        base_sha=BASE, head_sha=HEAD,
    )
    return result


def raw() -> dict:
    value = json.loads((ROOT / "tests/fixtures/scenarios/e2e-001/raw-pass.json").read_text())
    value["build_git_sha"] = HEAD
    return value


class TrustedCaseExecutionTests(unittest.TestCase):
    def test_dirty_candidate_cannot_claim_exact_head_execution(self):
        selected = plan()
        def git(command, **kwargs):
            return execution.subprocess.CompletedProcess(command, 1 if "diff" in command else 0, HEAD, "")
        with patch.object(case_execution.platform, "system", return_value="Windows"), \
             patch.object(case_execution.platform, "machine", return_value="AMD64"), \
             patch.object(case_execution.subprocess, "run", side_effect=git):
            errors = case_execution.validate_case_execution_inputs(selected, ROOT, "windows-latest", ROOT)
        self.assertTrue(any("dirty" in error for error in errors))

    def test_preflight_checks_actual_platform_head_and_every_protected_driver(self):
        selected = plan()
        with tempfile.TemporaryDirectory() as folder:
            candidate = Path(folder)
            for relative in (*case_execution.DRIVER_FILES, "src-tauri/src/lib.rs", "src-tauri/Cargo.toml"):
                path = candidate / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, path)
            with patch.object(case_execution.platform, "system", return_value="Windows"), \
                 patch.object(case_execution.platform, "machine", return_value="AMD64"), \
                 patch.object(case_execution.subprocess, "run") as git:
                git.return_value.returncode = 0
                git.return_value.stdout = HEAD + "\n"
                check = lambda: case_execution.validate_case_execution_inputs(selected, candidate, "windows-latest", ROOT)
                self.assertEqual(check(), [])
                for relative in case_execution.DRIVER_FILES:
                    with self.subTest(relative=relative):
                        path = candidate / relative
                        original = path.read_bytes()
                        path.write_bytes(original + b"\n// replacement\n")
                        self.assertTrue(check())
                        path.write_bytes(original)
                git.return_value.stdout = BASE
                self.assertTrue(check())
                git.return_value.stdout = HEAD
                with patch.object(case_execution.platform, "machine", return_value="aarch64"):
                    self.assertTrue(check())

    def test_canonical_library_prefix_cannot_be_cfg_disabled_or_path_redirected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            library = root / "src-tauri/src/lib.rs"
            library.parent.mkdir(parents=True)
            (root / "src-tauri/Cargo.toml").write_text('[package]\nname="codefactory"\n[lib]\nname="codefactory_lib"\n')
            library.write_text(case_execution.LIB_PREFIX + "mod product;\n")
            self.assertEqual(case_execution.validate_canonical_entry_points(root), [])
            for prefix in (
                '#[cfg(any())]\n' + case_execution.LIB_PREFIX,
                case_execution.LIB_PREFIX.replace('pub mod', '#[path="fake.rs"]\npub mod'),
                '// pub mod unattended_smoke_cli;\nmod fake;\n',
            ):
                library.write_text(prefix)
                self.assertTrue(case_execution.validate_canonical_entry_points(root))

    def test_protected_file_bundle_rejects_symlinked_parent(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            actual = root / "actual"
            actual.mkdir()
            (actual / "source.rs").write_text("canonical")
            try:
                (root / "linked").symlink_to(actual, target_is_directory=True)
            except OSError as error:
                self.skipTest(f"symlinks unavailable: {type(error).__name__}")
            with self.assertRaises(ValueError):
                case_execution.bundle_digest(root, ("linked/source.rs",))

    def test_candidate_cannot_add_a_cargo_runner_override_config(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            library = root / "src-tauri/src/lib.rs"
            library.parent.mkdir(parents=True)
            library.write_text(case_execution.LIB_PREFIX)
            (root / "src-tauri/Cargo.toml").write_text('[package]\nname="codefactory"\n[lib]\nname="codefactory_lib"\n')
            cargo_dir = root / ".cargo"
            cargo_dir.mkdir()
            (cargo_dir / "config").write_text('[target.\'cfg(windows)\']\nrunner=["python", "fake.py"]\n')
            self.assertTrue(case_execution.validate_canonical_entry_points(root))

    def test_raw_projection_erases_bad_types_and_private_values(self):
        observed = raw()
        observed.update(error="private", objective_status="/Users/private", ok="false", human_prompt_count=False)
        projected = case_execution.project_raw_observations(observed)
        self.assertIsNone(projected["objective_status"])
        self.assertIsNone(projected["ok"])
        self.assertIsNone(projected["human_prompt_count"])
        self.assertNotIn("private", json.dumps(projected))

    def test_selected_binary_requires_one_case_even_when_selected_via_scenario(self):
        actual = plan()
        self.assertEqual(len(actual["case_plans"]), 1)
        case = actual["case_plans"][0]
        expected = case["expectation"]
        self.assertEqual(expected["case_id"], "E2E-001")
        self.assertEqual(expected["canonical_target"], TARGET)
        self.assertEqual(expected["stage"], "pull_request")
        self.assertEqual(expected["head_sha"], HEAD)
        self.assertEqual(expected["oracle_policy"]["ui"], "not_required_for_stage")
        self.assertEqual(expected["build_identity"]["executable_build_sha"], HEAD)
        self.assertEqual(len(expected["driver_sha256"]), 64)
        self.assertEqual(len(expected["verifier_sha256"]), 64)

    def test_unrelated_plan_has_no_case_or_runner(self):
        registry = json.loads((ROOT / "docs/testing/scenario-registry.json").read_text())
        actual = execution.build_execution_plan(registry, ["docs/notes.md"], base_sha=BASE, head_sha=HEAD)
        self.assertEqual(actual["case_plans"], [])
        self.assertEqual(actual["runners"], {})

    def _runner_receipt(self, selected: dict, runner: str) -> dict:
        cases = []
        for case in selected["case_plans"]:
            expected = case["expectation"]
            if expected["runner"]["name"] != runner:
                continue
            observations = raw()
            receipt = build_e2e001_case_receipt(
                observations, expected, case["fixture_manifest"],
                runner=expected["runner"], build_identity=expected["build_identity"],
            )
            from tools.governance.scenario_case_execution import project_raw_observations
            cases.append({"case_id": "E2E-001", "raw_observations": project_raw_observations(observations), "receipt": receipt})
        return {
            "schema_version": execution.SCHEMA_VERSION, "base_sha": BASE,
            "head_sha": HEAD, "runner": runner, "cases": cases,
            "receipt_errors": [],
            "targets": [{"target": t, "outcome": "passed", "command_sha256": "c" * 64,
                         **({"alias_of": selected["target_aliases"][t]} if t in selected["target_aliases"] else {})}
                        for t in selected["runners"][runner]],
        }

    def _aggregate(self, selected: dict, receipts: list[dict]) -> dict:
        with tempfile.TemporaryDirectory() as folder:
            paths = []
            for i, receipt in enumerate(receipts):
                path = Path(folder) / f"{i}.json"
                path.write_text(json.dumps(receipt))
                paths.append(path)
            return execution.aggregate_receipts(selected, paths)

    def _valid(self) -> tuple[dict, dict]:
        selected = plan()
        receipts = [self._runner_receipt(selected, r) for r in selected["runners"]]
        return selected, self._aggregate(selected, receipts)

    def test_raw_receipt_is_recomputed_at_aggregate_and_final_gate(self):
        selected, aggregate = self._valid()
        self.assertEqual(execution.validate_aggregate_receipt(selected, aggregate), [])
        self.assertEqual(aggregate["cases"][0]["receipt"]["outcome"], "passed")
        aggregate["cases"][0]["raw_observations"]["worker_reaped"] = False
        self.assertTrue(execution.validate_aggregate_receipt(selected, aggregate))

    def test_uploaded_receipt_comparison_preserves_json_scalar_types(self):
        selected, aggregate = self._valid()
        aggregate["cases"][0]["receipt"]["schema_version"] = 2.0
        self.assertTrue(execution.validate_aggregate_receipt(selected, aggregate))

    def test_missing_duplicate_and_unknown_cases_are_rejected(self):
        selected, aggregate = self._valid()
        for cases in ([], aggregate["cases"] * 2,
                      [{**aggregate["cases"][0], "case_id": "E2E-999"}]):
            with self.subTest(cases=cases):
                changed = copy.deepcopy(aggregate)
                changed["cases"] = cases
                self.assertTrue(execution.validate_aggregate_receipt(selected, changed))

    def test_forged_passed_receipt_cannot_hide_raw_oracle_or_identity_failure(self):
        selected, aggregate = self._valid()
        for field, value in {
            "worker_reaped": False, "leaked_resource_count": 1,
            "build_git_sha": "c" * 40, "ok": "true", "user_message_count": True,
            "human_prompt_count": False, "descendant_process_count": False,
        }.items():
            with self.subTest(field=field):
                changed = copy.deepcopy(aggregate)
                changed["cases"][0]["raw_observations"][field] = value
                self.assertTrue(execution.validate_aggregate_receipt(selected, changed))

    def test_raw_free_text_is_never_accepted_into_aggregate(self):
        selected, aggregate = self._valid()
        aggregate["cases"][0]["raw_observations"]["error"] = "sensitive diagnostic"
        self.assertTrue(execution.validate_aggregate_receipt(selected, aggregate))

    def test_noncase_private_fields_are_rejected_and_not_reexported(self):
        selected, aggregate = self._valid()
        aggregate["targets"][0]["private_raw"] = {"cwd": "/Users/private", "token": "private"}
        self.assertTrue(execution.validate_aggregate_receipt(selected, aggregate))
        receipts = [self._runner_receipt(selected, r) for r in selected["runners"]]
        receipts[0]["targets"][0]["private_raw"] = {"cwd": "/Users/private", "token": "private"}
        actual = self._aggregate(selected, receipts)
        self.assertNotIn("/Users/private", json.dumps(actual))
        self.assertTrue(execution.validate_aggregate_receipt(selected, actual))

    def test_object_diagnostic_still_produces_anonymous_failure_receipt(self):
        selected = plan()
        receipts = [self._runner_receipt(selected, r) for r in selected["runners"]]
        for value in ({"token": "private"}, ["/Users/private"]):
            receipts[0]["targets"][0]["detail"] = value
            actual = self._aggregate(selected, receipts)
            self.assertTrue(execution.validate_aggregate_receipt(selected, actual))
            self.assertNotIn("private", json.dumps(actual))

    def test_extra_duplicate_wrong_runner_and_missing_digest_targets_fail(self):
        selected, aggregate = self._valid()
        mutations = [
            lambda r: r["targets"].append(copy.deepcopy(r["targets"][0])),
            lambda r: r["targets"].append({"target": "rust:unplanned", "outcome": "passed"}),
            lambda r: r["targets"][0].update(runner="unplanned-runner"),
            lambda r: r["targets"][0].pop("command_sha256"),
        ]
        for mutate in mutations:
            changed = copy.deepcopy(aggregate)
            mutate(changed)
            self.assertTrue(execution.validate_aggregate_receipt(selected, changed))

    def test_malformed_or_duplicate_runner_artifacts_fail_even_if_coverage_is_complete(self):
        selected = plan()
        receipts = [self._runner_receipt(selected, r) for r in selected["runners"]]
        for extra in (None, [], {**receipts[0], "head_sha": "c" * 40}, receipts[0]):
            with self.subTest(extra=extra):
                aggregate = self._aggregate(selected, [*receipts, extra])
                self.assertTrue(execution.validate_aggregate_receipt(selected, aggregate))

    def test_executor_retains_failure_case_and_cleans_its_temporary_directory(self):
        selected = plan()
        selected["required_targets"] = [TARGET]
        selected["runners"] = {"windows-latest": [TARGET]}
        captured = []

        def execute(target, repo, receipt_dir, policy):
            captured.append(receipt_dir)
            observation = raw()
            observation["cleanup_ok"] = False
            observation["error"] = "private diagnostic must not leave the runner"
            (receipt_dir / "unattended-long-task-smoke.json").write_text(json.dumps(observation))
            return False, "command exited 1"

        with patch.object(execution, "_execute_concrete_target", side_effect=execute), \
             patch.object(execution, "_validate_case_execution_inputs", return_value=[]):
            actual = execution.execute_plan(selected, ROOT, "windows-latest")
        self.assertEqual(actual["cases"][0]["receipt"]["outcome"], "failed")
        self.assertNotIn("private diagnostic", json.dumps(actual))
        self.assertTrue(captured)
        self.assertTrue(all(not path.exists() for path in captured))

    def test_nonzero_exit_cannot_be_rescued_by_a_passing_raw_receipt(self):
        with tempfile.TemporaryDirectory() as folder:
            output_dir = Path(folder)

            def run(command, repo, *, env):
                self.assertIn("--bin", command[:command.index("--")])
                self.assertIn("--locked", command[:command.index("--")])
                self.assertEqual(env["CODEFACTORY_BUILD_GIT_SHA"], HEAD)
                Path(command[-1]).write_text(json.dumps(raw()))
                return 1, ""

            with patch.object(execution, "_run_command", side_effect=run):
                passed, _ = execution._execute_concrete_target(
                    TARGET, ROOT, output_dir,
                    {"case_build_head_sha": HEAD, "binary_receipt_oracles": {TARGET.split(":", 1)[1]: {"field": "ok", "equals": True}}},
                )
            self.assertFalse(passed)


if __name__ == "__main__":
    unittest.main()
