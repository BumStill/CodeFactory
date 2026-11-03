"""M223: release waits retry transient GitHub infrastructure, never real failures.

The three Auto Release runs that died on 2026-11-03/04 while waiting on the
guarded version PR were infrastructure faults dressed up as red checks. These
tests pin both halves of the fix: the classifier's decision table (fed with the
same shapes ``gh pr view`` and ``gh run view --json jobs`` really print), and
the wiring that makes Auto Release consult it before failing a release.
"""

from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "tools/release/classify_ci_infra.py"
AUTO_RELEASE = REPO_ROOT / ".github/workflows/auto-release.yml"

ALL_REQUIRED = (
    "governance-baseline",
    "agent-bridge-linux",
    "check-frontend",
    "check-rust",
    "remote-real-app-gui",
    "scenario-gate-pr",
)


def check(name: str, conclusion: str, started: str = "2026-11-03T22:00:00Z") -> dict:
    return {"name": name, "conclusion": conclusion, "startedAt": started}


def failed_job(name: str, steps: list[str], conclusion: str = "failure") -> dict:
    return {
        "name": name,
        "conclusion": conclusion,
        "steps": [{"name": step, "conclusion": "failure"} for step in steps],
    }


def run_view(run_id: int, jobs: list[dict]) -> dict:
    """Shape of ``gh run view <id> --json jobs``."""

    return {"run_id": run_id, "jobs": {"jobs": jobs}}


def run_classifier(payload: dict) -> tuple[int, dict]:
    result = subprocess.run(
        [sys.executable, str(SCRIPT)],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    decision = json.loads(result.stdout) if result.returncode == 0 and result.stdout.strip() else {}
    return result.returncode, decision


class ClassifierDecisionTests(unittest.TestCase):
    def test_pending_check_waits(self) -> None:
        status, decision = run_classifier({"checks": [check("governance-baseline", "")]})
        self.assertEqual(status, 0)
        self.assertEqual(decision["decision"], "wait")
        self.assertIn("check-rust", decision["reason"])

    def test_all_required_success_merges(self) -> None:
        status, decision = run_classifier(
            {"checks": [check(name, "SUCCESS") for name in ALL_REQUIRED]}
        )
        self.assertEqual(status, 0)
        self.assertEqual(decision["decision"], "merge")

    def test_runner_setup_failure_is_retried(self) -> None:
        status, decision = run_classifier(
            {
                "checks": [check("check-rust", "FAILURE")],
                "runs": [run_view(41642171366, [failed_job("check-rust", ["Set up job"])])],
            }
        )
        self.assertEqual(status, 0)
        self.assertEqual(decision["decision"], "retry_infra")
        self.assertEqual(decision["retry_run_ids"], [41642171366])
        self.assertEqual(decision["checks"]["check-rust"], "infra")

    def test_cancelled_checkout_failure_is_retried(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-frontend", "CANCELLED")],
                "runs": [
                    run_view(
                        99,
                        [failed_job("check-frontend", ["Run actions/checkout@v4"], "cancelled")],
                    )
                ],
            }
        )
        self.assertEqual(decision["decision"], "retry_infra")
        self.assertEqual(decision["retry_run_ids"], [99])

    def test_startup_failure_without_steps_is_retried(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-rust", "STARTUP_FAILURE")],
                "retry_run_ids": [4711],
            }
        )
        self.assertEqual(decision["decision"], "retry_infra")
        self.assertEqual(decision["retry_run_ids"], [4711])

    def test_cancelled_check_without_step_evidence_is_retried(self) -> None:
        # A harness-cancelled job carries no test result; a bounded rerun is
        # the honest answer, not a failed release.
        _, decision = run_classifier(
            {
                "required": ["check-rust"],
                "checks": [check("check-rust", "CANCELLED")],
                "retry_run_ids": [31337],
            }
        )
        self.assertEqual(decision["decision"], "retry_infra")
        self.assertEqual(decision["retry_run_ids"], [31337])

    def test_real_test_failure_is_never_retried(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-rust", "FAILURE")],
                "runs": [
                    run_view(5, [failed_job("check-rust", ["Run cargo test --quiet"])])
                ],
            }
        )
        self.assertEqual(decision["decision"], "fail")
        self.assertIn("check-rust", decision["reason"])
        self.assertEqual(decision["retry_run_ids"], [])

    def test_real_failure_outranks_a_co_occurring_infra_signal(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-frontend", "FAILURE")],
                "runs": [run_view(7, [failed_job("check-frontend", ["Run pnpm test"])])],
                "log_lines": {
                    "check-frontend": [
                        "Get https://registry.npmjs.org: HTTP 502",
                        "AssertionError: expected 2 to equal 3",
                    ]
                },
            }
        )
        self.assertEqual(decision["decision"], "fail")

    def test_infra_signal_in_the_log_is_retried_when_the_step_is_silent(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-frontend", "FAILURE")],
                "runs": [run_view(8, [failed_job("check-frontend", ["Run actions/setup-node@v4"])])],
                "log_lines": {"check-frontend": ["Get https://nodejs.org: Service Unavailable"]},
            }
        )
        self.assertEqual(decision["decision"], "retry_infra")

    def test_mixed_checks_fail_instead_of_retrying(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [
                    check("check-rust", "FAILURE"),
                    check("check-frontend", "FAILURE"),
                ],
                "runs": [
                    run_view(
                        11,
                        [
                            failed_job("check-rust", ["Set up job"]),
                            failed_job("check-frontend", ["Run pnpm test"]),
                        ],
                    )
                ],
            }
        )
        self.assertEqual(decision["decision"], "fail")

    def test_unrecognised_failure_is_fail_closed(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("scenario-gate-pr", "FAILURE")],
                "runs": [run_view(3, [failed_job("scenario-gate-pr", ["Mystery"])])],
            }
        )
        self.assertEqual(decision["decision"], "fail")
        self.assertIn("no recognisable infrastructure signal", decision["reason"])

    def test_failure_without_any_evidence_is_fail_closed(self) -> None:
        _, decision = run_classifier({"checks": [check("check-rust", "FAILURE")]})
        self.assertEqual(decision["decision"], "fail")

    def test_retry_budget_is_bounded(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-rust", "FAILURE")],
                "runs": [run_view(1, [failed_job("check-rust", ["Set up job"])])],
                "infra_retries_used": 2,
                "max_infra_retries": 2,
            }
        )
        self.assertEqual(decision["decision"], "fail")
        self.assertIn("2 automatic retries", decision["reason"])

    def test_infra_failure_without_run_id_does_not_loop(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-rust", "FAILURE")],
                "failed_steps": [{"check": "check-rust", "step": "Set up job"}],
            }
        )
        self.assertEqual(decision["decision"], "fail")
        self.assertIn("no rerunnable workflow run id", decision["reason"])

    def test_newest_attempt_wins_over_a_superseded_failure(self) -> None:
        # A rerun appends a check run for the same head SHA; the FAILURE it
        # replaced must not keep failing the release it was meant to rescue.
        _, decision = run_classifier(
            {
                "required": ["check-rust"],
                "checks": [
                    check("check-rust", "FAILURE", "2026-11-03T22:00:00Z"),
                    check("check-rust", "SUCCESS", "2026-11-03T23:10:00Z"),
                ],
            }
        )
        self.assertEqual(decision["decision"], "merge")
        self.assertEqual(decision["checks"]["check-rust"], "success")

    def test_runner_capacity_annotation_is_retried(self) -> None:
        # Incident 3 (auto-release run 41689888439): the required check was
        # reported as failed with no failed step, because GitHub never placed
        # the job. Its annotation is the only evidence there is.
        _, decision = run_classifier(
            {
                "checks": [check("remote-real-app-gui", "FAILURE")],
                "annotations": {
                    "remote-real-app-gui": [
                        "The job was not acquired by Runner of type hosted even "
                        "after multiple attempts"
                    ]
                },
                "retry_run_ids": [41689888439],
            }
        )
        self.assertEqual(decision["decision"], "retry_infra")
        self.assertEqual(decision["retry_run_ids"], [41689888439])

    def test_capacity_constraint_annotation_on_a_build_job_is_retried(self) -> None:
        _, decision = run_classifier(
            {
                "required": ["build-macos"],
                "checks": [check("build-macos", "CANCELLED")],
                "annotations": {
                    "build-macos": [
                        "The hosted runner lost communication with the server. "
                        "Runner capacity constraints prevented a new machine."
                    ]
                },
                "retry_run_ids": [41669986868],
            }
        )
        self.assertEqual(decision["decision"], "retry_infra")

    def test_annotation_with_a_real_assertion_is_not_retried(self) -> None:
        _, decision = run_classifier(
            {
                "checks": [check("check-rust", "FAILURE")],
                "annotations": {"check-rust": ["assertion `left == right` failed"]},
                "retry_run_ids": [5],
            }
        )
        self.assertEqual(decision["decision"], "fail")

    def test_malformed_payload_is_a_program_error(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT)],
            input="not json",
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(result.returncode, 1)


class AutoReleaseWiringTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = AUTO_RELEASE.read_text(encoding="utf-8")

    def test_wait_step_consults_the_classifier(self) -> None:
        self.assertIn("tools/release/classify_ci_infra.py", self.workflow)

    def test_wait_step_retries_failed_jobs_for_infra_only(self) -> None:
        self.assertIn("gh run rerun", self.workflow)
        self.assertIn("--failed", self.workflow)

    def test_wait_step_no_longer_aborts_on_the_first_red_check(self) -> None:
        # The old loop exited the release on any non-success conclusion; that
        # single line turned a GitHub hiccup into a failed version.
        self.assertNotIn(
            'echo "::error::Version PR #$PR required check $CHECK failed"',
            self.workflow,
        )

    def test_observation_errors_do_not_kill_the_release(self) -> None:
        self.assertIn("could not read version PR", self.workflow)

    def test_wait_budget_covers_an_infrastructure_retry(self) -> None:
        self.assertIn("seq 1 360", self.workflow)

    def test_rollup_attempt_timestamps_reach_the_classifier(self) -> None:
        # Newest-attempt-wins is only possible if the wait loop forwards
        # startedAt instead of pre-collapsing the rollup in shell.
        self.assertIn("startedAt: (.startedAt // \"\")", self.workflow)

    def test_job_annotations_are_collected_as_evidence(self) -> None:
        # A job GitHub never placed has no failed step; its annotation is the
        # only place the reason appears.
        self.assertIn("check-runs/", self.workflow)
        self.assertIn("annotations", self.workflow)

    def test_github_calls_are_retried_before_they_become_evidence(self) -> None:
        self.assertIn("gh_retry()", self.workflow)


if __name__ == "__main__":
    unittest.main()
