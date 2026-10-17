"""M82 synthetic run-attempt regressions; no production receipts."""
import copy
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from tools.governance import scenario_execution as execution


class RerunReceiptTests(unittest.TestCase):
    def setUp(self):
        self.plan = {
            "schema_version": execution.SCHEMA_VERSION,
            "base_sha": "a" * 40, "head_sha": "b" * 40,
            "required_targets": ["rust:first", "rust:second"],
            "runners": {"windows-latest": ["rust:first", "rust:second"]},
            "case_plans": [], "scenario_ids": [], "e2e_ids": [], "blockers": [],
        }

    def receipt(self, attempt, passed):
        return {
            "schema_version": execution.SCHEMA_VERSION,
            "base_sha": self.plan["base_sha"], "head_sha": self.plan["head_sha"],
            "run_attempt": attempt, "runner": "windows-latest",
            "targets": [{"target": target, "outcome": "passed" if target in passed else "failed",
                         **({"command_sha256": "d" * 64} if target in passed else {"detail": "target_execution_failed"})}
                        for target in self.plan["required_targets"]],
            "cases": [], "receipt_errors": [],
        }

    def aggregate(self, receipts):
        with tempfile.TemporaryDirectory() as folder:
            paths = []
            for index, receipt in enumerate(receipts):
                path = Path(folder) / str(index) / "runner-receipt.json"
                path.parent.mkdir()
                path.write_text(json.dumps(receipt))
                paths.append(path)
            return execution.aggregate_receipts(self.plan, paths)

    def test_failed_first_successful_second_attempt(self):
        result = self.aggregate([self.receipt(1, ["rust:first"]), self.receipt(2, ["rust:second"])])
        self.assertEqual(execution.validate_aggregate_receipt(self.plan, result), [])

    def test_receipt_exists_only_in_attempt_two(self):
        result = self.aggregate([self.receipt(2, self.plan["required_targets"])])
        self.assertEqual(execution.validate_aggregate_receipt(self.plan, result), [])

    def test_failure_or_absence_is_not_success(self):
        for receipts in ([], [self.receipt(1, ["rust:first"]), self.receipt(2, [])]):
            self.assertTrue(execution.validate_aggregate_receipt(self.plan, self.aggregate(receipts)))

    def test_wrong_head_duplicate_attempt_and_invalid_digest_rejected(self):
        valid = self.receipt(2, self.plan["required_targets"])
        wrong = copy.deepcopy(valid)
        wrong["head_sha"] = "c" * 40
        bad_digest = copy.deepcopy(valid)
        bad_digest["run_attempt"] = 1
        bad_digest["targets"][0]["command_sha256"] = "invalid"
        for receipts in ([wrong, valid], [valid, valid], [bad_digest, valid]):
            self.assertTrue(execution.validate_aggregate_receipt(self.plan, self.aggregate(receipts)))

    def test_legacy_single_attempt_still_accepted(self):
        receipt = self.receipt(1, self.plan["required_targets"])
        receipt.pop("run_attempt")
        self.assertEqual(execution.validate_aggregate_receipt(self.plan, self.aggregate([receipt])), [])

    def test_complex_case_failure_can_be_superseded_not_forged(self):
        from tests.test_scenario_case_execution import TrustedCaseExecutionTests, plan, raw
        from tools.governance.scenario_case_execution import build_case_entry
        selected = plan()
        helper = TrustedCaseExecutionTests()
        good = [helper._runner_receipt(selected, runner) for runner in selected["runners"]]
        for receipt in good:
            receipt["run_attempt"] = 2
        failed = copy.deepcopy(next(receipt for receipt in good if receipt["cases"]))
        failed["run_attempt"] = 1
        observations = raw()
        observations["cleanup_ok"] = False
        failed["cases"] = [build_case_entry(observations, selected["case_plans"][0])]
        for target in failed["targets"]:
            target.update(outcome="failed", detail="case_observations_rejected")
        for receipts in ([failed, *good], [*good, failed]):
            aggregate = helper._aggregate(selected, receipts)
            self.assertEqual(execution.validate_aggregate_receipt(selected, aggregate), [])
        failed["cases"][0]["receipt"]["outcome"] = "passed"
        self.assertTrue(execution.validate_aggregate_receipt(selected, helper._aggregate(selected, [failed, *good])))

    def test_workflow_uploads_per_attempt_and_downloads_all_runner_attempts(self):
        workflow = (Path(__file__).resolve().parents[1] / ".github/workflows/scenario-execution.yml").read_text()
        for name in ("windows", "macos"):
            self.assertIn("name: affected-scenario-runner-" + name + "-attempt-${{ github.run_attempt }}", workflow)
        self.assertIn("name: affected-scenario-receipt-${{ github.event.pull_request.head.sha }}-attempt-${{ github.run_attempt }}", workflow)
        self.assertIn("pattern: affected-scenario-runner-*", workflow)
        self.assertIn("merge-multiple: false", workflow)
        self.assertIn("ref: ${{ github.event.pull_request.base.sha }}", workflow)

    @patch.object(execution, "_github_bytes")
    @patch.object(execution, "_github_json")
    def test_gate_selects_attempt_two_artifact_not_stale_failure(self, api, download):
        valid = self.aggregate([self.receipt(1, self.plan["required_targets"])])
        valid["receipt_errors"] = []
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as bundle:
            bundle.writestr("scenario-execution-receipt.json", json.dumps(valid))
        download.return_value = archive.getvalue()
        api.side_effect = [
            {"workflow_runs": [{"id": 42, "run_attempt": 2, "event": "pull_request", "head_sha": self.plan["head_sha"], "status": "completed", "conclusion": "success"}]},
            {"artifacts": [
                {"name": "affected-scenario-receipt-" + self.plan["head_sha"] + "-attempt-1", "expired": False, "archive_download_url": "https://api.github.test/stale"},
                {"name": "affected-scenario-receipt-" + self.plan["head_sha"] + "-attempt-2", "expired": False, "archive_download_url": "https://api.github.test/current"},
            ]},
        ]
        self.assertEqual(execution.await_github_receipt(self.plan, repository="synthetic/repo", workflow="scenario-execution.yml", token="synthetic", timeout_seconds=1), [])
        download.assert_called_once_with("https://api.github.test/current", "synthetic")
        self.assertIn("/actions/runs/42/artifacts", api.call_args_list[1].args[0])


if __name__ == "__main__":
    unittest.main()
