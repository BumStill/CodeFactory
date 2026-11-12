"""M223 acceptance test: drive Auto Release's real wait loop against a stub `gh`.

This is not a structural "the YAML contains a string" check. It extracts the
actual `Wait for guarded version bump merge` shell step out of
`.github/workflows/auto-release.yml`, runs it under `bash -euo pipefail` with a
fake `gh` on PATH, and asserts what the release does for each kind of GitHub
weather:

* a harness-level failure (runner setup) is rerun and the release proceeds;
* a real red test stops the release and is never rerun;
* a rate-limited/unreadable PR is "not yet known", not a failed version;
* the automatic retry budget is finite, so a sick GitHub is reported honestly.

Skipped when `bash` or `jq` is unavailable (the workflow itself runs on ubuntu).
"""

from __future__ import annotations

import json
import os
import shutil
import tempfile
import textwrap
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = REPO_ROOT / ".github/workflows/auto-release.yml"
STEP_NAME = "- name: Wait for guarded version bump merge"

REQUIRED = (
    "governance-baseline",
    "agent-bridge-linux",
    "check-frontend",
    "check-rust",
    "check-rust-macos-security",
    "check-rust-unix",
    "docker-sandbox",
    "remote-real-app-gui",
    "scenario-gate-pr",
)

GH_STUB = r'''
import json, os, subprocess, sys

args = sys.argv[1:]
with open(os.environ["GH_STUB_STATE"], encoding="utf-8") as fh:
    state = json.load(fh)
with open(os.environ["GH_STUB_LOG"], "a", encoding="utf-8") as fh:
    fh.write(" ".join(args) + "\n")


def emit(doc):
    jq = args[args.index("--jq") + 1] if "--jq" in args else None
    text = json.dumps(doc)
    if jq:
        proc = subprocess.run(["jq", "-c", jq], input=text, text=True,
                              capture_output=True)
        sys.stdout.write(proc.stdout)
        sys.stderr.write(proc.stderr)
        return proc.returncode
    sys.stdout.write(text)
    return 0


if args[:2] == ["pr", "view"]:
    queue = state.get("pr_view", [])
    if not queue:
        sys.stderr.write("gh stub: no more pr view fixtures\n")
        sys.exit(1)
    doc = queue.pop(0)
    # Persist the consumed fixture: each observation must advance the story.
    with open(os.environ["GH_STUB_STATE"], "w", encoding="utf-8") as fh:
        json.dump(state, fh)
    if doc.get("__error__"):
        sys.stderr.write("gh: HTTP 502 Bad Gateway (transient)\n")
        sys.exit(1)
    sys.exit(emit(doc))

if args[:2] == ["pr", "merge"]:
    with open(os.environ["GH_STUB_LOG"], "a", encoding="utf-8") as fh:
        fh.write("MERGE-SUBMITTED\n")
    sys.exit(0)

if args[:2] == ["run", "list"]:
    sys.exit(emit(state.get("run_list", [])))

if args[:2] == ["run", "view"]:
    sys.exit(emit(state.get("run_view", {}).get(args[2], {"jobs": []})))

if args[:2] == ["run", "rerun"]:
    with open(os.environ["GH_STUB_LOG"], "a", encoding="utf-8") as fh:
        fh.write("RERUN " + args[2] + " " + " ".join(args[3:]) + "\n")
    sys.exit(0)

sys.stderr.write("gh stub: unsupported " + " ".join(args) + "\n")
sys.exit(1)
'''


def extract_wait_step() -> str:
    return extract_step(WORKFLOW, STEP_NAME)


def extract_step(workflow: Path, step_name: str) -> str:
    lines = workflow.read_text(encoding="utf-8").splitlines()
    start = next(
        index
        for index, line in enumerate(lines)
        if line.strip() == step_name
    )
    run_at = next(
        index for index in range(start, len(lines)) if lines[index].strip() == "run: |"
    )
    body: list[str] = []
    for line in lines[run_at + 1 :]:
        if line.startswith("      - name:") or line.startswith("      - uses:"):
            break
        body.append(line[10:] if line.startswith(" " * 10) else line.strip())
    return textwrap.dedent("\n".join(body))


def rollup(overrides: dict, started: str = "2026-11-03T22:00:00Z") -> list:
    return [
        {"name": name, "conclusion": overrides.get(name, "SUCCESS"), "startedAt": started}
        for name in REQUIRED
    ]


def pr_view(state: str, checks: list, merge_state: str = "BLOCKED") -> dict:
    document = {
        "state": state,
        "mergeStateStatus": merge_state,
        "headRefOid": "abc123",
        "mergeCommit": None,
        "statusCheckRollup": checks,
    }
    if state == "MERGED":
        document["mergeCommit"] = {"oid": "deadbeef"}
    return document


def infra_run(run_id: int) -> dict:
    return {
        "run_list": [
            {"databaseId": run_id, "event": "pull_request", "conclusion": "failure"}
        ],
        "run_view": {
            str(run_id): {
                "jobs": [
                    {
                        "name": "check-rust",
                        "conclusion": "failure",
                        "steps": [{"name": "Set up job", "conclusion": "failure"}],
                    }
                ]
            }
        },
    }


@unittest.skipUnless(
    shutil.which("bash") and shutil.which("jq"),
    "the Auto Release wait loop runs on bash + jq (ubuntu runners)",
)
class AutoReleaseWaitLoopTests(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls) -> None:
        cls.body = extract_wait_step()

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="m223-wait-loop-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        (self.tmp / "ghstub.py").write_text(GH_STUB, encoding="utf-8")
        stub = self.tmp / "gh"
        stub.write_text(f'#!/bin/sh\nexec python3 "{self.tmp}/ghstub.py" "$@"\n')
        stub.chmod(0o755)

    def run_wait_loop(self, state: dict) -> tuple[int, str, str]:
        state_path = self.tmp / "state.json"
        log_path = self.tmp / "gh.log"
        out_path = self.tmp / "github_output"
        log_path.write_text("", encoding="utf-8")
        out_path.write_text("", encoding="utf-8")
        state_path.write_text(json.dumps(state), encoding="utf-8")

        harness = self.tmp / "harness.sh"
        # The loop's own sleeps are replaced: this test is about decisions,
        # not about how long the release waits between them.
        harness.write_text("sleep() { :; }\n" + self.body + "\n", encoding="utf-8")

        env = dict(os.environ)
        env.update(
            {
                "PATH": f"{self.tmp}{os.pathsep}{env.get('PATH', '')}",
                "GH_STUB_STATE": str(state_path),
                "GH_STUB_LOG": str(log_path),
                "GITHUB_OUTPUT": str(out_path),
                "GITHUB_REPOSITORY": "BumStill/CodeFactory",
                "GH_TOKEN": "stub",
                "PR": "1234",
                "EXPECTED_HEAD": "abc123",
            }
        )
        result = subprocess_run(harness, REPO_ROOT, env)
        return result.returncode, result.stdout + result.stderr, log_path.read_text(
            encoding="utf-8"
        )

    def test_runner_setup_failure_is_rerun_and_the_release_continues(self) -> None:
        state = {
            "pr_view": [
                pr_view("OPEN", rollup({"check-rust": "FAILURE"})),
                # three rounds of stale evidence after the rerun request
                pr_view("OPEN", rollup({"check-rust": "FAILURE"})),
                pr_view("OPEN", rollup({"check-rust": "FAILURE"})),
                pr_view("OPEN", rollup({"check-rust": "FAILURE"})),
                pr_view("OPEN", rollup({})),
                pr_view("MERGED", rollup({})),
            ],
            **infra_run(111),
        }
        code, output, gh_log = self.run_wait_loop(state)
        self.assertEqual(code, 0, output)
        self.assertIn("rerunning failed jobs instead of failing the release", output)
        self.assertNotIn("::error::", output)
        self.assertIn("RERUN 111", gh_log)
        self.assertIn("MERGE-SUBMITTED", gh_log)
        self.assertIn("merge_sha=deadbeef", (self.tmp / "github_output").read_text())

    def test_real_red_test_stops_the_release_without_rerunning(self) -> None:
        state = {
            "pr_view": [pr_view("OPEN", rollup({"check-rust": "FAILURE"}))],
            "run_list": [
                {"databaseId": 222, "event": "pull_request", "conclusion": "failure"}
            ],
            "run_view": {
                "222": {
                    "jobs": [
                        {
                            "name": "check-rust",
                            "conclusion": "failure",
                            "steps": [
                                {"name": "Run cargo test --quiet", "conclusion": "failure"}
                            ],
                        }
                    ]
                }
            },
        }
        code, output, gh_log = self.run_wait_loop(state)
        self.assertEqual(code, 1, output)
        self.assertIn("failed with a real result", output)
        self.assertNotIn("RERUN", gh_log)
        self.assertNotIn("MERGE-SUBMITTED", gh_log)

    def test_unreadable_pr_is_not_a_verdict(self) -> None:
        state = {
            "pr_view": [
                {"__error__": True},
                {"__error__": True},
                pr_view("OPEN", rollup({})),
                pr_view("MERGED", rollup({})),
            ]
        }
        code, output, gh_log = self.run_wait_loop(state)
        self.assertEqual(code, 0, output)
        self.assertIn("could not read version PR #1234", output)
        self.assertNotIn("::error::", output)
        self.assertIn("MERGE-SUBMITTED", gh_log)

    def test_automatic_retries_are_bounded(self) -> None:
        failing = pr_view("OPEN", rollup({"check-rust": "FAILURE"}))
        # Two retries, each followed by three stale grace rounds, then the
        # verdict: the release stops instead of rerunning forever.
        state = {
            "pr_view": [dict(failing) for _ in range(16)],
            **infra_run(333),
        }
        code, output, gh_log = self.run_wait_loop(state)
        self.assertEqual(code, 1, output)
        self.assertIn("persisted after 3 automatic retries", output)
        self.assertEqual(gh_log.count("RERUN 333"), 3, gh_log)


def subprocess_run(script: Path, cwd: Path, env: dict):
    import subprocess

    return subprocess.run(
        ["bash", str(script)],
        cwd=str(cwd),
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )


RECOVERY_WORKFLOW = REPO_ROOT / ".github/workflows/release-recover.yml"
RECOVERY_STEP = "- name: Rerun failed jobs when every red job was infrastructure"


@unittest.skipUnless(
    shutil.which("bash") and shutil.which("jq"),
    "the release recovery step runs on bash + jq (ubuntu runners)",
)
class ReleaseRecoveryTests(unittest.TestCase):
    """M223 R3: a runner that never arrived must not leave a Draft release."""

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="m223-recover-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        (self.tmp / "ghstub.py").write_text(GH_STUB, encoding="utf-8")
        stub = self.tmp / "gh"
        stub.write_text(f'#!/bin/sh\nexec python3 "{self.tmp}/ghstub.py" "$@"\n')
        stub.chmod(0o755)

    def run_recovery(self, jobs: list, attempt: int = 1) -> tuple[int, str, str]:
        state_path = self.tmp / "state.json"
        log_path = self.tmp / "gh.log"
        log_path.write_text("", encoding="utf-8")
        state_path.write_text(
            json.dumps({"run_view": {"41669986868": {"jobs": jobs}}}), encoding="utf-8"
        )
        harness = self.tmp / "harness.sh"
        harness.write_text(
            "sleep() { :; }\n"
            + extract_step(RECOVERY_WORKFLOW, RECOVERY_STEP)
            + "\n",
            encoding="utf-8",
        )
        env = dict(os.environ)
        env.update(
            {
                "PATH": f"{self.tmp}{os.pathsep}{env.get('PATH', '')}",
                "GH_STUB_STATE": str(state_path),
                "GH_STUB_LOG": str(log_path),
                "REPO": "BumStill/CodeFactory",
                "GH_TOKEN": "stub",
                "RUN_ID": "41669986868",
                "RUN_ATTEMPT": str(attempt),
                "HEAD_BRANCH": "v1.2.3",
                "MAX_ATTEMPTS": "3",
            }
        )
        result = subprocess_run(harness, REPO_ROOT, env)
        return result.returncode, result.stdout + result.stderr, log_path.read_text(
            encoding="utf-8"
        )

    def test_capacity_cancelled_build_reruns_failed_jobs(self) -> None:
        jobs = [
            {"name": "changelog", "conclusion": "success", "steps": []},
            {"name": "prepare-release", "conclusion": "success", "steps": []},
            {"name": "build-windows", "conclusion": "success", "steps": []},
            # Never placed on a machine: no step ever ran.
            {"name": "build-macos", "conclusion": "cancelled", "steps": []},
            {"name": "finalize", "conclusion": "skipped", "steps": []},
        ]
        code, output, gh_log = self.run_recovery(jobs)
        self.assertEqual(code, 0, output)
        self.assertIn("transient infrastructure failure on build-macos", output)
        self.assertIn("RERUN 41669986868 --repo", gh_log)

    def test_real_build_failure_is_not_rerun(self) -> None:
        jobs = [
            {"name": "changelog", "conclusion": "success", "steps": []},
            {"name": "prepare-release", "conclusion": "success", "steps": []},
            {"name": "build-windows", "conclusion": "success", "steps": []},
            {
                "name": "build-macos",
                "conclusion": "failure",
                "steps": [{"name": "Run cargo tauri build", "conclusion": "failure"}],
            },
            {"name": "finalize", "conclusion": "skipped", "steps": []},
        ]
        code, output, gh_log = self.run_recovery(jobs)
        self.assertEqual(code, 0, output)
        self.assertIn("must not be rerun away", output)
        self.assertNotIn("RERUN", gh_log)

    def test_attempt_budget_stops_the_loop(self) -> None:
        jobs = [{"name": "build-macos", "conclusion": "cancelled", "steps": []}]
        code, output, gh_log = self.run_recovery(jobs, attempt=3)
        self.assertEqual(code, 1, output)
        self.assertIn("exhausted its two infrastructure retries", output)
        self.assertNotIn("RERUN", gh_log)


if __name__ == "__main__":
    unittest.main()
