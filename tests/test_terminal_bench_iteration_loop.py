import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools.benchmark import terminal_bench_21_iteration_loop as loop


class TerminalBenchIterationLoopTest(unittest.TestCase):
    def test_parse_evidence_summary_counts_trials_and_failure_classes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "evidence.md"
            path.write_text(
                "\n".join(
                    [
                        "# Evidence",
                        "",
                        "- subset: `terminal-bench-21-regression-subset-v1`",
                        "- run: `run-1`",
                        "- trials: `3`",
                        "- pass_count: `1`",
                        "- mean_reward: `0.333333`",
                        "",
                        "| Task | Reward | Failure class |",
                        "| --- | ---: | --- |",
                        "| `terminal-bench/a` | `1` | `None` |",
                        "| `terminal-bench/b` | `0` | `Some(\"tool-use\")` |",
                        "| `terminal-bench/c` | `0` | `Some(\"policy\")` |",
                        "",
                    ]
                )
            )

            summary = loop.parse_evidence(path)

            self.assertEqual(summary.run, "run-1")
            self.assertEqual(summary.pass_count, 1)
            self.assertEqual(summary.trials, 3)
            self.assertEqual(summary.mean_reward, 0.333333)
            self.assertEqual(
                summary.failure_counts,
                {"pass": 1, "policy": 1, "tool-use": 1},
            )

    def test_write_canary_subset_uses_requested_tasks(self) -> None:
        source = {
            "id": "subset-v1",
            "tasks": [
                {"name": "write-compressor", "bucket": "passed-smoke"},
                {"name": "mteb-retrieve", "bucket": "verifier-zero"},
            ],
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = loop.write_canary_subset(
                source,
                ["mteb-retrieve"],
                Path(tmp),
            )

            data = json.loads(path.read_text())

            self.assertEqual(data["id"], "subset-v1-canary")
            self.assertEqual(
                [item["name"] for item in data["tasks"]],
                ["mteb-retrieve"],
            )

    def test_parse_evidence_prefers_failure_class_count_section(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "baseline.md"
            path.write_text(
                "\n".join(
                    [
                        "- task_count: `2`",
                        "- pass_count: `1`",
                        "- mean_reward: `0.5`",
                        "",
                        "## Failure Class Counts",
                        "",
                        "| Failure class | Count |",
                        "| --- | ---: |",
                        "| `pass` | `1` |",
                        "| `verification` | `1` |",
                        "",
                        "## Trials",
                        "",
                        "| Task | Reward | Failure class | Failure reason | Trial dir |",
                        "| --- | ---: | --- | --- | --- |",
                        "| `a` | `1.0` | `pass` | `pass` | `/tmp/a` |",
                        "| `b` | `0.0` | `verification` | `verifier-zero` | `/tmp/b` |",
                    ]
                )
            )

            summary = loop.parse_evidence(path)

            self.assertEqual(summary.failure_counts, {"pass": 1, "verification": 1})

    def test_write_iteration_report_records_delta_and_next_queue(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            baseline_path = tmp_path / "baseline.md"
            head_path = tmp_path / "head.md"
            baseline_path.write_text(
                "- run: `baseline`\n- trials: `2`\n- pass_count: `1`\n- mean_reward: `0.5`\n"
            )
            head_path.write_text(
                "- run: `head`\n- trials: `2`\n- pass_count: `2`\n- mean_reward: `1.0`\n"
            )
            baseline = loop.parse_evidence(baseline_path)
            head = loop.parse_evidence(head_path)

            with mock.patch.object(loop, "EVIDENCE_DIR", tmp_path):
                report = loop.write_iteration_report(
                    args=mock.Mock(
                        endpoint="deepseek",
                        model=None,
                        shell_timeout_sec=300,
                        hypothesis="reduce repeated inspection",
                        target_failure_class="tool-use",
                        product_capability_verdict="mixed",
                        product_capability_impact=(
                            "state-aware agent loop avoids destructive follow-up"
                        ),
                        product_example=(
                            "after starting a dev server and confirming readiness, "
                            "CodeFactory stops instead of killing it"
                        ),
                        benchmark_only_boundary=(
                            "the canary task itself is only one benchmark scenario"
                        ),
                    ),
                    scope="canary",
                    subset_path=tmp_path / "subset.json",
                    baseline=baseline,
                    head=head,
                    exit_code=None,
                    ran_command=False,
                    output="",
                )

            text = report.read_text()
            self.assertIn("- pass_count: `1` -> `2` (`+1`)", text)
            self.assertIn("- mean_reward: `0.500000` -> `1.000000` (`+0.500000`)", text)
            self.assertIn("reduce repeated inspection", text)
            self.assertIn("## Product Capability Impact", text)
            self.assertIn("- verdict: mixed", text)
            self.assertIn("state-aware agent loop", text)
            self.assertIn("dev server", text)
            self.assertIn("only one benchmark scenario", text)
            self.assertIn("artifact implementation earlier", text)

    def test_write_iteration_report_marks_mismatched_trial_counts_not_comparable(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            baseline_path = tmp_path / "baseline.md"
            head_path = tmp_path / "head.md"
            baseline_path.write_text(
                "- run: `baseline`\n- trials: `18`\n- pass_count: `4`\n"
                "- mean_reward: `0.222222`\n"
            )
            head_path.write_text(
                "- run: `head`\n- trials: `1`\n- pass_count: `0`\n"
                "- mean_reward: `0.0`\n"
            )
            baseline = loop.parse_evidence(baseline_path)
            head = loop.parse_evidence(head_path)

            with mock.patch.object(loop, "EVIDENCE_DIR", tmp_path):
                report = loop.write_iteration_report(
                    args=mock.Mock(
                        endpoint="deepseek",
                        model=None,
                        shell_timeout_sec=300,
                        hypothesis="target one task",
                        target_failure_class="environment",
                        product_capability_verdict="benchmark-only",
                        product_capability_impact=(
                            "separates evaluation runtime instability from agent output"
                        ),
                        product_example=(
                            "CodeFactory can show a user that a failed run was blocked "
                            "by Docker storage instead of blaming the model patch"
                        ),
                        benchmark_only_boundary=(
                            "this report compares benchmark evidence and does not change "
                            "the agent execution loop"
                        ),
                    ),
                    scope="canary",
                    subset_path=tmp_path / "subset.json",
                    baseline=baseline,
                    head=head,
                    exit_code=0,
                    ran_command=True,
                    output="",
                )

            text = report.read_text()
            self.assertIn("- comparable_delta: `no`", text)
            self.assertIn("different trial counts", text)
            self.assertNotIn("- pass_count: `4` -> `0`", text)

    def test_write_iteration_report_requires_product_capability_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(loop, "EVIDENCE_DIR", Path(tmp)):
                with self.assertRaisesRegex(
                    ValueError,
                    "--product-capability-verdict",
                ):
                    loop.write_iteration_report(
                        args=mock.Mock(
                            endpoint="deepseek",
                            model=None,
                            shell_timeout_sec=300,
                            override_storage_mb=None,
                            hypothesis="target one task",
                            target_failure_class="environment",
                        ),
                        scope="canary",
                        subset_path=Path(tmp) / "subset.json",
                        baseline=None,
                        head=None,
                        exit_code=None,
                        ran_command=False,
                        output="",
                    )

    def test_run_subset_times_out_and_returns_reportable_output(self) -> None:
        class TimeoutProcess:
            pid = 12345
            returncode = None

            def __init__(self) -> None:
                self.calls = 0

            def communicate(self, timeout=None):
                self.calls += 1
                if self.calls == 1:
                    raise loop.subprocess.TimeoutExpired(cmd=["runner"], timeout=timeout)
                return "partial output\n", None

        process = TimeoutProcess()
        with (
            mock.patch.object(loop.subprocess, "Popen", return_value=process),
            mock.patch.object(loop.os, "killpg") as killpg,
        ):
            exit_code, output = loop.run_subset(
                Path("/tmp/subset.json"),
                endpoint="deepseek",
                model=None,
                concurrency=2,
                secret_timeout_sec=20,
                shell_timeout_sec=300,
                run_timeout_sec=1,
            )

        self.assertEqual(exit_code, 124)
        self.assertIn("partial output", output)
        self.assertIn("BENCHMARK_RUN_TIMEOUT: exceeded 1 seconds", output)
        killpg.assert_called_once_with(12345, loop.signal.SIGTERM)

    def test_run_subset_passes_infra_options_to_regression_runner(self) -> None:
        class CompletedProcess:
            pid = 12345
            returncode = 0

            def communicate(self, timeout=None):
                return "done\n", None

        with mock.patch.object(loop.subprocess, "Popen", return_value=CompletedProcess()) as popen:
            exit_code, output = loop.run_subset(
                Path("/tmp/subset.json"),
                endpoint="deepseek",
                model=None,
                concurrency=1,
                secret_timeout_sec=20,
                shell_timeout_sec=300,
                run_timeout_sec=60,
                override_storage_mb=65536,
                provider_bridge_retries=5,
                docker_apt_proxy="http://host.docker.internal:7897",
                verifier_proxy="http://host.docker.internal:7897",
                provider_proxy="http://127.0.0.1:7897",
            )

        command = popen.call_args.args[0]
        self.assertEqual(exit_code, 0)
        self.assertIn("done", output)
        self.assertIn("--shell-timeout-sec", command)
        self.assertIn("300", command)
        self.assertIn("--override-storage-mb", command)
        self.assertIn("65536", command)
        self.assertIn("--provider-bridge-retries", command)
        self.assertIn("5", command)
        self.assertIn("--docker-apt-proxy", command)
        self.assertIn("http://host.docker.internal:7897", command)
        self.assertIn("--verifier-proxy", command)
        self.assertIn("--provider-proxy", command)
        self.assertIn("http://127.0.0.1:7897", command)


if __name__ == "__main__":
    unittest.main()
