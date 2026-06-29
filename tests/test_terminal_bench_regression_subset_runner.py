import argparse
import unittest
from unittest import mock

from tools.benchmark import run_terminal_bench_21_regression_subset as runner


class TerminalBenchRegressionSubsetRunnerTest(unittest.TestCase):
    def args(self, **overrides):
        values = {
            "min_docker_cpus": 4.0,
            "min_docker_memory_gb": 6.0,
            "min_docker_free_gb": 20.0,
            "preflight_timeout_sec": 30,
            "skip_resource_preflight": False,
            "endpoint": "deepseek",
            "override_storage_mb": None,
        }
        values.update(overrides)
        return argparse.Namespace(**values)

    def test_resource_preflight_blocks_low_docker_cpu_before_provider_run(self) -> None:
        def fake_capture(command, timeout):
            if command[:2] == ["docker", "info"]:
                return runner.CapturedCommand(
                    0,
                    '{"NCPU":2,"MemTotal":8589934592}',
                )
            return runner.CapturedCommand(0, "root_free_gb=80.00\n")

        with mock.patch.object(runner, "run_capture", side_effect=fake_capture):
            result = runner.run_preflight(self.args())

        self.assertFalse(result.ok)
        self.assertTrue(any("2.00 CPUs" in item for item in result.blockers))

    def test_resource_preflight_accepts_sufficient_docker_resources(self) -> None:
        def fake_capture(command, timeout):
            if command[:2] == ["docker", "info"]:
                return runner.CapturedCommand(
                    0,
                    '{"NCPU":4,"MemTotal":8589934592}',
                )
            return runner.CapturedCommand(0, "root_free_gb=80.00\n")

        with mock.patch.object(runner, "run_capture", side_effect=fake_capture):
            result = runner.run_preflight(self.args())

        self.assertTrue(result.ok)
        self.assertEqual(result.blockers, [])


if __name__ == "__main__":
    unittest.main()
