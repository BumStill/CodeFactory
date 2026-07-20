import argparse
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools.benchmark import run_terminal_bench_21_regression_subset as runner


class TerminalBenchRegressionSubsetRunnerTest(unittest.TestCase):
    def args(self, **overrides):
        values = {
            "min_docker_cpus": 4.0,
            "min_docker_memory_gb": 6.0,
            "min_docker_free_gb": 20.0,
            "preflight_timeout_sec": 30,
            "preflight_retries": 1,
            "skip_resource_preflight": False,
            "endpoint": "deepseek",
            "concurrency": 1,
            "override_storage_mb": None,
            "trial_hard_timeout_sec": 1200,
            "heavy_verifier_hard_timeout_sec": 2400,
            "heavy_verifier_timeout_multiplier": 1.0,
            "watchdog_poll_interval_sec": 15,
            "model_timeout_sec": 120,
            "shell_timeout_sec": 300,
            "agent_wall_timeout_sec": 780,
            "secret_timeout_sec": 20,
            "model": None,
            "docker_apt_proxy": None,
            "verifier_proxy": None,
            "provider_proxy": None,
            "provider_bridge_retries": 2,
            "verifier_uv_http_timeout_sec": None,
            "verifier_uv_torch_backend": None,
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
        commands = []

        def fake_capture(command, timeout):
            commands.append(command)
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
        self.assertTrue(any("python:3.10-slim-bookworm" in command for command in commands))
        self.assertTrue(any("ubuntu:24.04" in command for command in commands))

    def test_resource_preflight_uses_configured_docker_apt_proxy(self) -> None:
        commands = []

        def fake_capture(command, timeout):
            commands.append(command)
            if command[:2] == ["docker", "info"]:
                return runner.CapturedCommand(
                    0,
                    '{"NCPU":4,"MemTotal":8589934592}',
                )
            return runner.CapturedCommand(0, "root_free_gb=80.00\n")

        with mock.patch.object(runner, "run_capture", side_effect=fake_capture):
            result = runner.run_preflight(
                self.args(docker_apt_proxy="http://192.168.5.2:7897")
            )

        self.assertTrue(result.ok)
        smoke_commands = [
            command for command in commands if command[:2] == ["docker", "run"]
        ]
        self.assertTrue(smoke_commands)
        self.assertTrue(
            any("99codefactory-proxy" in " ".join(command) for command in smoke_commands)
        )
        self.assertTrue(any("docker apt proxy" in item for item in result.details))

    def test_resource_preflight_blocks_failed_curl_bootstrap(self) -> None:
        def fake_capture(command, timeout):
            if command[:2] == ["docker", "info"]:
                return runner.CapturedCommand(
                    0,
                    '{"NCPU":4,"MemTotal":8589934592}',
                )
            return runner.CapturedCommand(
                100,
                "root_free_gb=80.00\nE: Unable to locate package curl\n",
            )

        with mock.patch.object(runner, "run_capture", side_effect=fake_capture):
            result = runner.run_preflight(self.args())

        self.assertFalse(result.ok)
        self.assertTrue(
            any("verifier bootstrap smoke failed" in item for item in result.blockers)
        )

    def test_resource_preflight_blocks_ubuntu_bootstrap_failure(self) -> None:
        def fake_capture(command, timeout):
            if command[:2] == ["docker", "info"]:
                return runner.CapturedCommand(
                    0,
                    '{"NCPU":4,"MemTotal":8589934592}',
                )
            if "ubuntu:24.04" in command:
                return runner.CapturedCommand(
                    100,
                    (
                        "root_free_gb=80.00\n"
                        "Err:1 http://archive.ubuntu.com/ubuntu noble InRelease\n"
                        "  Connection failed [IP: 198.18.0.5 80]\n"
                    ),
                )
            return runner.CapturedCommand(0, "root_free_gb=80.00\n")

        with mock.patch.object(runner, "run_capture", side_effect=fake_capture):
            result = runner.run_preflight(self.args())

        self.assertFalse(result.ok)
        self.assertTrue(any("ubuntu-noble" in item for item in result.blockers))
        self.assertTrue(any("archive.ubuntu.com" in item for item in result.details))

    def test_resource_preflight_retries_transient_ubuntu_bootstrap_failure(self) -> None:
        ubuntu_attempts = 0

        def fake_capture(command, timeout):
            nonlocal ubuntu_attempts
            if command[:2] == ["docker", "info"]:
                return runner.CapturedCommand(
                    0,
                    '{"NCPU":4,"MemTotal":8589934592}',
                )
            if "ubuntu:24.04" in command:
                ubuntu_attempts += 1
                if ubuntu_attempts == 1:
                    return runner.CapturedCommand(
                        100,
                        "root_free_gb=80.00\nCould not connect to proxy\n",
                    )
            return runner.CapturedCommand(0, "root_free_gb=80.00\n")

        with mock.patch.object(runner, "run_capture", side_effect=fake_capture):
            result = runner.run_preflight(self.args())

        self.assertTrue(result.ok)
        self.assertEqual(result.blockers, [])
        self.assertEqual(ubuntu_attempts, 2)
        self.assertTrue(
            any("bootstrap smoke retrying ubuntu-noble" in item for item in result.details)
        )

    def test_build_env_enables_partial_import_diagnostic(self) -> None:
        subset = {"tasks": [{"name": "write-compressor"}, {"name": "kv-store-grpc"}]}

        env = runner.build_env(self.args(concurrency=2), subset)

        self.assertEqual(env["CODEFACTORY_BENCH_ALLOW_PARTIAL_IMPORT"], "1")
        self.assertNotIn("CODEFACTORY_BENCH_VERIFIER_UV_HTTP_TIMEOUT_SEC", env)
        self.assertNotIn("CODEFACTORY_BENCH_VERIFIER_UV_TORCH_BACKEND", env)
        self.assertNotIn("CODEFACTORY_BENCH_VERIFIER_TIMEOUT_MULTIPLIER", env)
        self.assertEqual(
            env["CODEFACTORY_BENCH_TASK_NAMES"], "write-compressor,kv-store-grpc"
        )

    def test_build_env_does_not_override_harbor_agent_timeout_by_default(self) -> None:
        subset = {"tasks": [{"name": "terminal-bench/example"}]}

        env = runner.build_env(self.args(agent_wall_timeout_sec=0), subset)

        self.assertNotIn("CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC", env)

    def test_build_env_passes_official_per_task_agent_budgets_without_overriding_harbor(self) -> None:
        subset = {
            "tasks": [
                {"name": "build-cython-ext", "agent_timeout_sec": 900},
                {"name": "filter-js-from-html", "agent_timeout_sec": 1800},
            ]
        }

        env = runner.build_env(self.args(agent_wall_timeout_sec=0), subset)

        self.assertEqual(
            json.loads(env["CODEFACTORY_BENCH_TASK_AGENT_TIMEOUTS_JSON"]),
            {"build-cython-ext": 900, "filter-js-from-html": 1800},
        )
        self.assertNotIn("CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC", env)

    def test_timeout_multiplier_marks_run_noncomparable(self) -> None:
        args = self.args(
            trial_hard_timeout_sec=0,
            heavy_verifier_hard_timeout_sec=0,
            heavy_verifier_timeout_multiplier=3.0,
        )

        self.assertEqual(runner.comparable_label(args), "no")
        self.assertTrue(
            any("verifier timeout multiplier" in note for note in runner.comparability_notes(args))
        )

    def test_verifier_runtime_override_is_explicit_and_noncomparable(self) -> None:
        args = self.args(
            trial_hard_timeout_sec=0,
            agent_wall_timeout_sec=0,
            verifier_uv_http_timeout_sec=120,
            verifier_uv_torch_backend="cpu",
        )
        subset = {"tasks": [{"name": "terminal-bench/example"}]}

        env = runner.build_env(args, subset)

        self.assertEqual(env["CODEFACTORY_BENCH_VERIFIER_UV_HTTP_TIMEOUT_SEC"], "120")
        self.assertEqual(env["CODEFACTORY_BENCH_VERIFIER_UV_TORCH_BACKEND"], "cpu")
        self.assertEqual(runner.comparable_label(args), "no")
        self.assertTrue(
            any("verifier runtime environment" in note for note in runner.comparability_notes(args))
        )

    def test_build_env_sets_harbor_verifier_timeout_multiplier_for_heavy_task(
        self,
    ) -> None:
        subset = {"tasks": [{"name": "terminal-bench/torch-tensor-parallelism"}]}

        env = runner.build_env(
            self.args(heavy_verifier_timeout_multiplier=3.0), subset
        )

        self.assertEqual(env["CODEFACTORY_BENCH_VERIFIER_TIMEOUT_MULTIPLIER"], "3")

    def test_build_env_passes_docker_apt_proxy_to_agent(self) -> None:
        subset = {"tasks": [{"name": "protein-assembly"}]}

        env = runner.build_env(
            self.args(docker_apt_proxy="http://192.168.5.2:7897"), subset
        )

        self.assertEqual(
            env["CODEFACTORY_BENCH_DOCKER_APT_PROXY"],
            "http://192.168.5.2:7897",
        )

    def test_build_env_passes_provider_proxy_to_bridge_process(self) -> None:
        subset = {"tasks": [{"name": "build-cython-ext"}]}

        env = runner.build_env(
            self.args(provider_proxy="http://127.0.0.1:7897"), subset
        )

        self.assertEqual(env["HTTPS_PROXY"], "http://127.0.0.1:7897")
        self.assertEqual(env["https_proxy"], "http://127.0.0.1:7897")
        self.assertEqual(env["ALL_PROXY"], "http://127.0.0.1:7897")
        self.assertEqual(env["NO_PROXY"], runner.LOOPBACK_NO_PROXY)
        self.assertEqual(env["no_proxy"], runner.LOOPBACK_NO_PROXY)

    def test_build_env_passes_verifier_proxy_to_bridge_process(self) -> None:
        subset = {"tasks": [{"name": "mteb-retrieve"}]}

        env = runner.build_env(
            self.args(verifier_proxy="http://host.docker.internal:7897"), subset
        )

        self.assertEqual(
            env["CODEFACTORY_BENCH_VERIFIER_PROXY"],
            "http://host.docker.internal:7897",
        )

    def test_agent_preflight_reuses_explicit_executable_without_building(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "codefactory-agent-headless"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
            env = {"CODEFACTORY_BENCH_AGENT_BINARY": str(binary)}

            with mock.patch.object(runner, "run_capture") as capture:
                result = runner.prepare_agent_binary(env, timeout_sec=900)

        self.assertTrue(result.ok)
        self.assertEqual(env["CODEFACTORY_BENCH_AGENT_BINARY"], str(binary.resolve()))
        capture.assert_not_called()
        self.assertTrue(any("explicit" in detail for detail in result.details))

    def test_agent_preflight_builds_current_source_and_injects_binary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo_root = Path(tmp)
            manifest = repo_root / "src-tauri/Cargo.toml"
            binary = repo_root / "src-tauri/target/debug/codefactory-agent-headless"
            manifest.parent.mkdir(parents=True)
            manifest.write_text("[workspace]\n")
            env = {}

            def fake_capture(command, timeout):
                self.assertEqual(timeout, 900)
                self.assertEqual(
                    command,
                    [
                        "cargo",
                        "build",
                        "--manifest-path",
                        str(manifest),
                        "-p",
                        "codefactory-agent-headless",
                    ],
                )
                binary.parent.mkdir(parents=True)
                binary.write_text("#!/bin/sh\nexit 0\n")
                binary.chmod(0o755)
                return runner.CapturedCommand(0, "Finished dev profile")

            with (
                mock.patch.object(runner, "REPO_ROOT", repo_root),
                mock.patch.object(runner, "run_capture", side_effect=fake_capture),
            ):
                result = runner.prepare_agent_binary(env, timeout_sec=900)

        self.assertTrue(result.ok)
        self.assertEqual(env["CODEFACTORY_BENCH_AGENT_BINARY"], str(binary.resolve()))
        self.assertTrue(any("built from current source" in detail for detail in result.details))

    def test_agent_preflight_blocks_when_build_does_not_produce_binary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo_root = Path(tmp)
            manifest = repo_root / "src-tauri/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text("[workspace]\n")
            env = {}

            with (
                mock.patch.object(runner, "REPO_ROOT", repo_root),
                mock.patch.object(
                    runner,
                    "run_capture",
                    return_value=runner.CapturedCommand(0, "Finished dev profile"),
                ),
            ):
                result = runner.prepare_agent_binary(env, timeout_sec=900)

        self.assertFalse(result.ok)
        self.assertNotIn("CODEFACTORY_BENCH_AGENT_BINARY", env)
        self.assertTrue(any("did not produce" in blocker for blocker in result.blockers))

    def test_bind_mount_preflight_blocks_when_container_write_is_not_visible(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo_root = Path(tmp)
            with (
                mock.patch.object(runner, "REPO_ROOT", repo_root),
                mock.patch.object(
                    runner,
                    "run_capture",
                    return_value=runner.CapturedCommand(0, "host marker readable\n"),
                ),
            ):
                result = runner.run_bind_mount_preflight(timeout_sec=30)

        self.assertFalse(result.ok)
        self.assertTrue(
            any("container-to-host" in blocker for blocker in result.blockers)
        )

    def test_bind_mount_preflight_accepts_bidirectional_write(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo_root = Path(tmp)

            def fake_capture(command, timeout):
                self.assertEqual(timeout, 30)
                probe_dir = repo_root / ".codefactory/benchmark-preflight"
                (probe_dir / "container-to-host.txt").write_text(
                    runner.BIND_MOUNT_PROBE_TOKEN
                )
                return runner.CapturedCommand(0, "host marker readable\n")

            with (
                mock.patch.object(runner, "REPO_ROOT", repo_root),
                mock.patch.object(runner, "run_capture", side_effect=fake_capture),
            ):
                result = runner.run_bind_mount_preflight(timeout_sec=30)

        self.assertTrue(result.ok)
        self.assertTrue(any("bidirectional" in detail for detail in result.details))

    def test_parse_output_captures_provider_result_status(self) -> None:
        parsed = runner.parse_output(
            "\n".join(
                [
                    "provider_bridge_result status=failed exit_code=Some(1) job_path=/tmp/job",
                    "provider_bridge_imported run=run-id dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=1 mean_reward=1.000",
                    "provider_bridge_trial task=terminal-bench/write-compressor reward=1 failure_class=None",
                ]
            )
        )

        self.assertEqual(parsed["result"]["status"], "failed")
        self.assertEqual(parsed["result"]["exit_code"], "Some(1)")
        self.assertEqual(parsed["imported"]["run"], "run-id")
        self.assertEqual(len(parsed["trials"]), 1)

    def test_parse_output_captures_failed_without_partial_import(self) -> None:
        parsed = runner.parse_output(
            "\n".join(
                [
                    "provider_bridge_result status=failed exit_code=Some(1) job_path=/tmp/job",
                    "provider_bridge_no_partial_import job_path=/tmp/job",
                ]
            )
        )

        self.assertEqual(parsed["result"]["status"], "failed")
        self.assertTrue(parsed["no_partial_import"])
        self.assertIsNone(parsed["imported"])

    def test_provider_bridge_transient_no_partial_failure_retries(self) -> None:
        calls = []

        def fake_run_command(env, watchdog):
            calls.append((env, watchdog))
            if len(calls) == 1:
                return (
                    1,
                    "\n".join(
                        [
                            "provider_bridge_result status=failed exit_code=Some(1) job_path=/tmp/job",
                            "provider_bridge_no_partial_import job_path=/tmp/job",
                            "ReadError",
                        ]
                    ),
                    [],
                )
            return (
                0,
                "\n".join(
                    [
                        "provider_bridge_result status=completed exit_code=Some(0) job_path=/tmp/job",
                        "provider_bridge_imported run=run-id dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=1 mean_reward=1.000",
                    ]
                ),
                [],
            )

        with mock.patch.object(runner, "run_command", side_effect=fake_run_command):
            exit_code, output, parsed, interventions = runner.run_command_with_retries(
                self.args(provider_bridge_retries=1),
                {"CODEFACTORY_BENCH_JOB_ROOT": "/tmp"},
                {"tasks": [{"name": "protein-assembly"}]},
            )

        self.assertEqual(exit_code, 0)
        self.assertEqual(len(calls), 2)
        self.assertIn("Provider bridge attempt 1/2", output)
        self.assertEqual(parsed["imported"]["mean_reward"], "1.000")
        self.assertEqual(interventions, [])

    def test_provider_bridge_transient_partial_failure_retries(self) -> None:
        calls = []

        def fake_run_command(env, watchdog):
            calls.append((env, watchdog))
            if len(calls) == 1:
                return (
                    1,
                    "\n".join(
                        [
                            "provider_bridge_result status=failed exit_code=Some(1) job_path=/tmp/job",
                            "provider_bridge_imported run=partial-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=4 mean_reward=0.500",
                            "provider_bridge_trial task=terminal-bench/write-compressor reward=1 failure_class=None",
                            "ConnectError",
                        ]
                    ),
                    [],
                )
            return (
                0,
                "\n".join(
                    [
                        "provider_bridge_result status=completed exit_code=Some(0) job_path=/tmp/job",
                        "provider_bridge_imported run=final-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=6 mean_reward=1.000",
                    ]
                ),
                [],
            )

        with mock.patch.object(runner, "run_command", side_effect=fake_run_command):
            exit_code, output, parsed, interventions = runner.run_command_with_retries(
                self.args(provider_bridge_retries=1),
                {"CODEFACTORY_BENCH_JOB_ROOT": "/tmp"},
                {"tasks": [{"name": "write-compressor"}]},
            )

        self.assertEqual(exit_code, 0)
        self.assertEqual(len(calls), 2)
        self.assertIn("transient-provider-network-failure", output)
        self.assertEqual(parsed["imported"]["run"], "final-run")
        self.assertEqual(interventions, [])

    def test_provider_bridge_transient_partial_failure_retries_when_cargo_test_exits_zero(
        self,
    ) -> None:
        calls = []

        def fake_run_command(env, watchdog):
            calls.append((env, watchdog))
            if len(calls) == 1:
                return (
                    0,
                    "\n".join(
                        [
                            "provider_bridge_result status=failed exit_code=Some(1) job_path=/tmp/job",
                            "provider_bridge_imported run=partial-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=8 mean_reward=0.375",
                            "provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some(\"environment\")",
                            "ConnectError",
                        ]
                    ),
                    [],
                )
            return (
                0,
                "\n".join(
                    [
                        "provider_bridge_result status=completed exit_code=Some(0) job_path=/tmp/job",
                        "provider_bridge_imported run=final-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=18 mean_reward=0.667",
                    ]
                ),
                [],
            )

        with mock.patch.object(runner, "run_command", side_effect=fake_run_command):
            exit_code, output, parsed, interventions = runner.run_command_with_retries(
                self.args(provider_bridge_retries=1),
                {"CODEFACTORY_BENCH_JOB_ROOT": "/tmp"},
                {"tasks": [{"name": "mteb-retrieve"}]},
            )

        self.assertEqual(exit_code, 0)
        self.assertEqual(len(calls), 2)
        self.assertIn("transient-provider-network-failure", output)
        self.assertEqual(parsed["imported"]["run"], "final-run")
        self.assertEqual(interventions, [])

    def test_provider_bridge_completed_environment_download_failure_retries(
        self,
    ) -> None:
        calls = []
        with tempfile.TemporaryDirectory() as tmp:
            job_path = Path(tmp) / "job"
            stdout_path = (
                job_path
                / "torch-tensor-parallelism__abc123"
                / "verifier"
                / "test-stdout.txt"
            )
            stdout_path.parent.mkdir(parents=True)
            stdout_path.write_text(
                "\n".join(
                    [
                        "  × Failed to download `torch==2.7.0`",
                        "  ├─▶ Request failed after 3 retries",
                        "  ╰─▶ tls handshake eof",
                    ]
                )
            )

            def fake_run_command(env, watchdog):
                calls.append((env, watchdog))
                if len(calls) == 1:
                    return (
                        0,
                        "\n".join(
                            [
                                f"provider_bridge_result status=completed exit_code=Some(0) job_path={job_path}",
                                "provider_bridge_imported run=env-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=1 mean_reward=0.000",
                                "provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=0 failure_class=Some(\"environment\")",
                            ]
                        ),
                        [],
                    )
                return (
                    0,
                    "\n".join(
                        [
                            "provider_bridge_result status=completed exit_code=Some(0) job_path=/tmp/job",
                            "provider_bridge_imported run=final-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=1 mean_reward=1.000",
                            "provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=1 failure_class=None",
                        ]
                    ),
                    [],
                )

            with mock.patch.object(runner, "run_command", side_effect=fake_run_command):
                exit_code, output, parsed, interventions = runner.run_command_with_retries(
                    self.args(provider_bridge_retries=1),
                    {"CODEFACTORY_BENCH_JOB_ROOT": "/tmp"},
                    {"tasks": [{"name": "torch-tensor-parallelism"}]},
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(len(calls), 2)
        self.assertIn("transient-provider-network-failure", output)
        self.assertEqual(parsed["imported"]["run"], "final-run")
        self.assertEqual(interventions, [])

    def test_provider_bridge_completed_verifier_apt_bootstrap_failure_retries_without_environment_class(
        self,
    ) -> None:
        calls = []
        with tempfile.TemporaryDirectory() as tmp:
            job_path = Path(tmp) / "job"
            stdout_path = (
                job_path
                / "torch-tensor-parallelism__abc123"
                / "verifier"
                / "test-stdout.txt"
            )
            stdout_path.parent.mkdir(parents=True)
            stdout_path.write_text(
                "\n".join(
                    [
                        "Err:1 http://archive.ubuntu.com/ubuntu noble InRelease",
                        "  Error reading from server. Remote end closed connection",
                        "E: Unable to locate package curl",
                        "/tests/test.sh: line 10: /root/.local/bin/env: No such file or directory",
                        "/tests/test.sh: line 19: uvx: command not found",
                    ]
                )
            )

            def fake_run_command(env, watchdog):
                calls.append((env, watchdog))
                if len(calls) == 1:
                    return (
                        0,
                        "\n".join(
                            [
                                f"provider_bridge_result status=completed exit_code=Some(0) job_path={job_path}",
                                "provider_bridge_imported run=apt-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=1 mean_reward=0.000",
                                "provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=0 failure_class=Some(\"verification\")",
                            ]
                        ),
                        [],
                    )
                return (
                    0,
                    "\n".join(
                        [
                            "provider_bridge_result status=completed exit_code=Some(0) job_path=/tmp/job",
                            "provider_bridge_imported run=final-run dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some(\"deepseek\") comparable=true trials=1 mean_reward=1.000",
                            "provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=1 failure_class=None",
                        ]
                    ),
                    [],
                )

            with mock.patch.object(runner, "run_command", side_effect=fake_run_command):
                exit_code, output, parsed, interventions = runner.run_command_with_retries(
                    self.args(provider_bridge_retries=1),
                    {"CODEFACTORY_BENCH_JOB_ROOT": "/tmp"},
                    {"tasks": [{"name": "torch-tensor-parallelism"}]},
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(len(calls), 2)
        self.assertIn("transient-provider-network-failure", output)
        self.assertEqual(parsed["imported"]["run"], "final-run")
        self.assertEqual(interventions, [])

    def test_report_marks_failed_without_partial_import_as_blocker(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = self.args(trial_hard_timeout_sec=0)
            subset = {
                "id": "subset",
                "source_run_id": "source",
                "tasks": [{"name": "filter-js-from-html"}],
            }
            parsed = {
                "result": {
                    "status": "failed",
                    "exit_code": "Some(1)",
                    "job_path": str(Path(tmp) / "job"),
                },
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(Path(tmp) / "job"),
                },
                "imported": None,
                "trials": [],
                "no_partial_import": True,
            }
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(args, subset, 1, "output", parsed, [])

            text = report.read_text()
            self.assertIn("- exit_code: `1`", text)
            self.assertIn("- official_comparable: `no`", text)
            self.assertIn("## Blocker", text)
            self.assertIn("before Harbor produced an importable partial job", text)

    def test_preflight_blocker_report_is_never_officially_comparable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = self.args()
            subset = {
                "id": "subset",
                "source_run_id": "source",
                "tasks": [{"name": "filter-js-from-html"}],
            }
            preflight = runner.PreflightResult(
                ok=False,
                blockers=["Docker is unavailable"],
                details=["docker info failed"],
            )
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_preflight_blocker_report(args, subset, preflight)

            text = report.read_text()
            self.assertIn("- official_comparable: `no`", text)
            self.assertIn("- harbor_started: `no`", text)
            self.assertIn("- trials: `0`", text)

    def test_watchdog_stops_stale_trial_container(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            job_path = Path(tmp)
            trial_path = job_path / "query-optimize__zBaaXr3"
            trial_path.mkdir()
            (trial_path / "config.json").write_text("{}")

            watchdog = runner.BenchmarkWatchdog(timeout_sec=10)
            watchdog.job_path = job_path
            watchdog._first_seen[trial_path.name] = 0

            def fake_run(command, **kwargs):
                if command[:3] == ["docker", "ps", "--format"]:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        stdout="query-optimize__zbaaxr3-main-1\nother-main-1\n",
                    )
                if command[:2] == ["docker", "stop"]:
                    return subprocess.CompletedProcess(command, 0, stdout="")
                raise AssertionError(f"unexpected command: {command}")

            with mock.patch.object(runner.subprocess, "run", side_effect=fake_run):
                messages = watchdog.check_once(now=11)

            self.assertEqual(len(messages), 1)
            interventions = watchdog.interventions()
            self.assertEqual(interventions[0].trial, "query-optimize__zBaaXr3")
            self.assertEqual(interventions[0].containers, ["query-optimize__zbaaxr3-main-1"])
            self.assertEqual(interventions[0].action, "docker-stop")

    def test_watchdog_uses_heavy_verifier_timeout_override(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            job_path = Path(tmp)
            trial_path = job_path / "torch-tensor-parallelism__abc123"
            trial_path.mkdir()
            (trial_path / "config.json").write_text("{}")

            watchdog = runner.BenchmarkWatchdog(
                timeout_sec=1200,
                trial_timeout_overrides={"torch-tensor-parallelism": 2400},
            )
            watchdog.job_path = job_path
            watchdog._first_seen[trial_path.name] = 0

            def fake_run(command, **kwargs):
                if command[:3] == ["docker", "ps", "--format"]:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        stdout="torch-tensor-parallelism__abc123-main-1\n",
                    )
                if command[:2] == ["docker", "stop"]:
                    return subprocess.CompletedProcess(command, 0, stdout="")
                raise AssertionError(f"unexpected command: {command}")

            with mock.patch.object(runner.subprocess, "run", side_effect=fake_run):
                self.assertEqual(watchdog.check_once(now=1500), [])
                messages = watchdog.check_once(now=2401)

            self.assertEqual(len(messages), 1)
            interventions = watchdog.interventions()
            self.assertEqual(interventions[0].trial, "torch-tensor-parallelism__abc123")
            self.assertEqual(
                interventions[0].containers,
                ["torch-tensor-parallelism__abc123-main-1"],
            )

    def test_report_marks_watchdog_intervention_non_comparable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = self.args(trial_hard_timeout_sec=1200)
            subset = {"id": "subset", "source_run_id": "source", "tasks": [{"name": "query-optimize"}]}
            parsed = {
                "result": {
                    "status": "failed",
                    "exit_code": "Some(1)",
                    "job_path": str(Path(tmp) / "job"),
                },
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(Path(tmp) / "job"),
                },
                "imported": {
                    "run": "run-id",
                    "dataset": "terminal-bench/terminal-bench-2-1",
                    "agent": "codefactory-headless",
                    "model": 'Some("deepseek-v4-pro")',
                    "comparable": "true",
                    "trials": "1",
                    "mean_reward": "0.000",
                },
                "trials": [
                    {
                        "task": "terminal-bench/query-optimize",
                        "reward": "0",
                        "failure_class": 'Some("verification")',
                    }
                ],
            }
            intervention = runner.WatchdogIntervention(
                trial="query-optimize__zBaaXr3",
                elapsed_sec=1200,
                containers=["query-optimize__zbaaxr3-main-1"],
                action="docker-stop",
            )
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(args, subset, 0, "output", parsed, [intervention])

            text = report.read_text()
            self.assertIn("- official_comparable: `no`", text)
            self.assertIn("## Provider Bridge", text)
            self.assertIn("## Partial Import Note", text)
            self.assertIn("## Watchdog Interventions", text)
            self.assertIn("query-optimize__zBaaXr3", text)

    def test_report_marks_enabled_watchdog_non_comparable_even_without_intervention(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = self.args(trial_hard_timeout_sec=1200)
            subset = {"id": "subset", "source_run_id": "source", "tasks": [{"name": "write-compressor"}]}
            parsed = {
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(Path(tmp) / "job"),
                },
                "imported": {
                    "run": "run-id",
                    "dataset": "terminal-bench/terminal-bench-2-1",
                    "agent": "codefactory-headless",
                    "model": 'Some("deepseek-v4-pro")',
                    "comparable": "true",
                    "trials": "1",
                    "mean_reward": "1.000",
                },
                "trials": [
                    {
                        "task": "terminal-bench/write-compressor",
                        "reward": "1",
                        "failure_class": "None",
                    }
                ],
            }
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(args, subset, 0, "output", parsed, [])

            text = report.read_text()
            self.assertIn("- official_comparable: `no`", text)
            self.assertIn("runner-level trial hard timeout watchdog was enabled", text)

    def test_report_marks_outer_timeout_non_comparable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = self.args()
            subset = {
                "id": "subset",
                "source_run_id": "source",
                "tasks": [{"name": "write-compressor"}],
            }
            parsed = {
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(Path(tmp) / "job"),
                }
            }
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(
                    args,
                    subset,
                    124,
                    "BENCHMARK_RUN_TIMEOUT: exceeded 1800 seconds",
                    parsed,
                    [],
                )

            text = report.read_text()
            self.assertIn("- official_comparable: `no`", text)
            self.assertIn("benchmark process exceeded its outer wall timeout", text)

    def test_report_uses_imported_run_comparability(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = self.args(
                trial_hard_timeout_sec=0,
                agent_wall_timeout_sec=0,
            )
            subset = {
                "id": "subset",
                "source_run_id": "source",
                "tasks": [{"name": "mteb-retrieve"}],
            }
            job_path = Path(tmp) / "job"
            parsed = {
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(job_path),
                },
                "result": {
                    "status": "completed",
                    "exit_code": "0",
                    "job_path": str(job_path),
                },
                "imported": {
                    "run": "run-id",
                    "dataset": "terminal-bench/terminal-bench-2-1",
                    "agent": "codefactory-headless",
                    "model": 'Some("deepseek-v4-pro")',
                    "comparable": "false",
                    "trials": "1",
                    "mean_reward": "0.000",
                },
                "trials": [
                    {
                        "task": "terminal-bench/mteb-retrieve",
                        "reward": "0",
                        "failure_class": 'Some("verification")',
                    }
                ],
            }
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(args, subset, 0, "output", parsed, [])

            text = report.read_text()
            self.assertIn("- official_comparable: `no`", text)
            self.assertIn("imported Harbor run was marked non-comparable", text)

    def test_detects_verifier_browser_environment_warnings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            stdout_path = (
                Path(tmp)
                / "filter-js-from-html__abc123"
                / "verifier"
                / "test-stdout.txt"
            )
            stdout_path.parent.mkdir(parents=True)
            stdout_path.write_text(
                "\n".join(
                    [
                        "RUN BATCH 0",
                        "Failed to create driver or process file: Message: Unable to obtain driver for chrome",
                        "<jemalloc>: (This is the expected behaviour if you are running under QEMU)",
                        "WARNING: Retrying after connection broken by RemoteDisconnected('Remote end closed connection without response')",
                    ]
                )
            )

            warnings = runner.detect_verifier_environment_warnings(tmp)

        self.assertEqual(len(warnings), 3)
        self.assertEqual(warnings[0].trial, "filter-js-from-html__abc123")
        self.assertEqual(warnings[0].category, "browser-driver-unavailable")
        self.assertEqual(warnings[1].category, "emulated-browser-runtime")
        self.assertEqual(warnings[2].category, "verifier-python-bootstrap-network")

    def test_report_includes_verifier_environment_warnings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            job_path = Path(tmp) / "job"
            stdout_path = (
                job_path
                / "filter-js-from-html__abc123"
                / "verifier"
                / "test-stdout.txt"
            )
            stdout_path.parent.mkdir(parents=True)
            stdout_path.write_text(
                "Failed to create driver or process file: Message: Unable to obtain driver for chrome\n"
            )
            args = self.args(trial_hard_timeout_sec=0)
            subset = {
                "id": "subset",
                "source_run_id": "source",
                "tasks": [{"name": "filter-js-from-html"}],
            }
            parsed = {
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(job_path),
                },
                "imported": {
                    "run": "run-id",
                    "dataset": "terminal-bench/terminal-bench-2-1",
                    "agent": "codefactory-headless",
                    "model": 'Some("deepseek-v4-pro")',
                    "comparable": "true",
                    "trials": "1",
                    "mean_reward": "1.000",
                },
                "trials": [
                    {
                        "task": "terminal-bench/filter-js-from-html",
                        "reward": "1",
                        "failure_class": "None",
                    }
                ],
            }
            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(args, subset, 0, "output", parsed, [])

            text = report.read_text()
            self.assertIn("## Verifier Environment Warnings", text)
            self.assertIn("browser-driver-unavailable", text)
            self.assertIn("Unable to obtain driver for chrome", text)

    def test_report_aggregates_rust_agent_usage_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            job_path = Path(tmp) / "job"
            metadata_path = job_path / "task__abc" / "agent" / "run-metadata.json"
            metadata_path.parent.mkdir(parents=True)
            metadata_path.write_text(
                '{"runtime_subject":"rust-core","usage":{"prompt_tokens":10,'
                '"completion_tokens":4,"total_tokens":14,"model_requests":2},'
                '"tool_calls":3}'
            )
            args = self.args(trial_hard_timeout_sec=0, heavy_verifier_hard_timeout_sec=0)
            subset = {"id": "subset", "tasks": [{"name": "task"}]}
            parsed = {
                "result": {"status": "completed", "exit_code": "0", "job_path": str(job_path)},
                "preview": {
                    "model": "deepseek-v4-pro",
                    "task_limit": "1",
                    "concurrency": "1",
                    "override_storage_mb": "<none>",
                    "job_path": str(job_path),
                },
                "imported": {
                    "run": "run-id",
                    "dataset": "terminal-bench/terminal-bench-2-1",
                    "agent": "codefactory-headless",
                    "model": 'Some("deepseek-v4-pro")',
                    "comparable": "true",
                    "trials": "1",
                    "mean_reward": "1.000",
                },
                "trials": [{"task": "task", "reward": "1", "failure_class": "None"}],
            }

            with mock.patch.object(runner, "EVIDENCE_DIR", Path(tmp)):
                report = runner.write_report(args, subset, 0, "output", parsed, [])

            text = report.read_text()
            self.assertIn("## Agent Usage", text)
            self.assertIn("- model_requests: `2`", text)
            self.assertIn("- total_tokens: `14`", text)
            self.assertIn("- tool_calls: `3`", text)


if __name__ == "__main__":
    unittest.main()
