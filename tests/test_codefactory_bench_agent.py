import asyncio
import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch

from harbor.environments.base import ExecResult
from harbor.models.agent.context import AgentContext

from codefactory_bench.agent import CodeFactoryAgent


class FakeNetworkMode:
    def __init__(self, value: str) -> None:
        self.value = value


class FakeNetworkPolicy:
    def __init__(self, mode: str) -> None:
        self.network_mode = FakeNetworkMode(mode)


class FakeEnvironment:
    def __init__(
        self,
        results: list[ExecResult] | None = None,
        network_mode: str = "no-network",
        project_manifests: str = "",
        project_manifests_after_command: dict[str, str] | None = None,
    ) -> None:
        self.calls: list[dict[str, object]] = []
        self.results = list(results or [])
        self.network_policy = FakeNetworkPolicy(network_mode)
        self.project_manifests = project_manifests
        self.project_manifests_after_command = dict(project_manifests_after_command or {})

    async def exec(
        self,
        command: str,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        timeout_sec: int | None = None,
        user: str | int | None = None,
    ) -> ExecResult:
        self.calls.append(
            {
                "command": command,
                "cwd": cwd,
                "env": env,
                "timeout_sec": timeout_sec,
                "user": user,
            }
        )
        if command == "pwd -P":
            return ExecResult(stdout="/workspace\n", stderr="", return_code=0)
        if command.startswith("find . -maxdepth 3"):
            return ExecResult(stdout=self.project_manifests, stderr="", return_code=0)
        for expected, manifests in self.project_manifests_after_command.items():
            if expected in command:
                self.project_manifests = manifests
                break
        if self.results:
            return self.results.pop(0)
        return ExecResult(stdout="ok", stderr="", return_code=0)


class CodeFactoryBenchAgentTest(unittest.TestCase):
    @staticmethod
    def _running_linux_process_group_members(pgid: int) -> list[tuple[int, str]]:
        members: list[tuple[int, str]] = []
        for stat_path in Path("/proc").glob("[0-9]*/stat"):
            try:
                stat = stat_path.read_text()
                fields = stat[stat.rfind(")") + 2 :].split()
                state = fields[0]
                process_group = int(fields[2])
            except (IndexError, OSError, ValueError):
                continue
            if process_group == pgid and state != "Z":
                members.append((int(stat_path.parent.name), state))
        return members

    def test_harbor_import_path_and_identity_are_stable(self) -> None:
        self.assertEqual(CodeFactoryAgent.import_path(), "codefactory_bench.agent:CodeFactoryAgent")
        self.assertEqual(CodeFactoryAgent.name(), "codefactory-headless")

    def test_timed_out_tool_request_cleans_its_managed_process_group(self) -> None:
        class TimeoutEnvironment(FakeEnvironment):
            async def exec(self, command: str, **kwargs: object) -> ExecResult:
                self.calls.append({"command": command, **kwargs})
                if "codefactory-tool-cleanup" in command:
                    return ExecResult(stdout="cleaned", stderr="", return_code=0)
                raise TimeoutError("tool exceeded environment timeout")

        environment = TimeoutEnvironment()
        agent = CodeFactoryAgent(logs_dir=Path("unused"), model_name="test-model")

        result = asyncio.run(
            agent._execute_tool_request(
                {
                    "id": "call-timeout",
                    "command": "sqlite3 database.sqlite < slow.sql",
                    "timeout_sec": 1,
                },
                environment,
                "/workspace",
            )
        )

        self.assertIsNone(result["return_code"])
        self.assertIn("TimeoutError", str(result["error"]))
        self.assertEqual(len(environment.calls), 2)
        self.assertIn("setsid", str(environment.calls[0]["command"]))
        self.assertIn("sqlite3 database.sqlite < slow.sql", str(environment.calls[0]["command"]))
        self.assertIn("codefactory-tool-cleanup", str(environment.calls[1]["command"]))
        self.assertIn("kill", str(environment.calls[1]["command"]))

    def test_managed_tool_command_preserves_bash_syntax(self) -> None:
        managed, _ = CodeFactoryAgent._managed_tool_command(
            "call-bash", "[[ -n bash-ok ]] && printf bash-ok"
        )

        self.assertIn("setsid /bin/bash -c", managed)

    @unittest.skipUnless(
        shutil.which("setsid") and Path("/bin/bash").exists(),
        "requires Linux setsid and bash",
    )
    def test_managed_tool_command_executes_real_bash_syntax(self) -> None:
        managed, pidfile = CodeFactoryAgent._managed_tool_command(
            "call-real-bash", "[[ -n bash-ok ]] && printf bash-ok"
        )

        completed = subprocess.run(
            managed,
            shell=True,
            executable="/bin/bash",
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout, "bash-ok")
        self.assertFalse(Path(pidfile).exists())

    @unittest.skipUnless(
        shutil.which("setsid") and Path("/bin/bash").exists(),
        "requires Linux setsid and bash",
    )
    def test_cleanup_terminates_a_real_managed_process_group(self) -> None:
        class LocalEnvironment:
            async def exec(
                self,
                command: str,
                cwd: str | None = None,
                env: dict[str, str] | None = None,
                timeout_sec: int | None = None,
                user: str | int | None = None,
            ) -> ExecResult:
                del user
                process = await asyncio.create_subprocess_exec(
                    "/bin/bash",
                    "-lc",
                    command,
                    cwd=cwd,
                    env={**os.environ, **(env or {})},
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                )
                stdout, stderr = await asyncio.wait_for(
                    process.communicate(), timeout=timeout_sec
                )
                return ExecResult(
                    stdout=stdout.decode(errors="replace"),
                    stderr=stderr.decode(errors="replace"),
                    return_code=process.returncode,
                )

        with tempfile.TemporaryDirectory() as tmp:
            agent = CodeFactoryAgent(logs_dir=Path(tmp), model_name="test-model")
            managed, pidfile = agent._managed_tool_command(
                "call-real-cleanup", "sleep 30"
            )
            launcher = subprocess.Popen(
                managed,
                shell=True,
                executable="/bin/bash",
                cwd=tmp,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                deadline = time.monotonic() + 5
                while not Path(pidfile).exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertTrue(Path(pidfile).exists(), "managed PGID was not recorded")
                pgid = int(Path(pidfile).read_text().strip())

                asyncio.run(
                    agent._cleanup_managed_process_group(
                        LocalEnvironment(), tmp, pidfile
                    )
                )

                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    launcher.poll()
                    members = self._running_linux_process_group_members(pgid)
                    if not members:
                        break
                    time.sleep(0.05)
                else:
                    self.fail(
                        f"managed process group {pgid} has running members: {members}"
                    )
            finally:
                if launcher.poll() is None:
                    launcher.kill()
                launcher.wait(timeout=5)
                Path(pidfile).unlink(missing_ok=True)

    def test_setup_records_shared_contract_and_never_inspects_verifier(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            env = FakeEnvironment()
            agent = CodeFactoryAgent(
                logs_dir=Path(tmp),
                extra_env={"CODEFACTORY_BENCH_DOCKER_APT_PROXY": "http://192.168.5.2:7897"},
            )

            asyncio.run(agent.setup(env))

            setup = json.loads((Path(tmp) / "setup.json").read_text())
            self.assertEqual(setup["integrity"]["contamination_scan"], "pass")
            self.assertRegex(setup["execution_contract_sha256"], r"^[0-9a-f]{64}$")
            self.assertEqual(len(env.calls), 1)
            command = str(env.calls[0]["command"])
            self.assertIn("99codefactory-proxy", command)
            self.assertNotIn("/" + "tests", command)

    def test_baseline_records_contract_hash_in_context(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            env = FakeEnvironment()
            context = AgentContext()
            agent = CodeFactoryAgent(logs_dir=Path(tmp), model_name=None)

            asyncio.run(agent.run("Inspect this workspace", env, context))

            assert context.metadata is not None
            self.assertEqual(context.metadata["mode"], "baseline-no-model")
            self.assertRegex(context.metadata["execution_contract_sha256"], r"^[0-9a-f]{64}$")
            self.assertEqual(context.metadata["integrity"]["contamination_scan"], "pass")

    def _write_sidecar(self, root: Path, body: str) -> Path:
        sidecar = root / "fake-sidecar"
        sidecar.write_text("#!/usr/bin/env python3\n" + body)
        sidecar.chmod(0o755)
        return sidecar

    def test_model_run_bridges_sidecar_tool_requests_to_harbor(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(
                root,
                """import json, sys
start = json.loads(sys.stdin.readline())
print(json.dumps({"type":"tool_request","id":"call_1","command":"python -m unittest","timeout_sec":30}), flush=True)
result = json.loads(sys.stdin.readline())
print(json.dumps({"type":"finished","final_text":"verified","execution_contract_sha256":start["execution_contract_sha256"],"completion_evidence":{"ready": result["return_code"] == 0},"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}}), flush=True)
""",
            )
            env = FakeEnvironment(
                [ExecResult(stdout="OK", stderr="", return_code=0)]
            )
            context = AgentContext()
            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            asyncio.run(agent.run("Implement and verify the requested change", env, context))

            self.assertEqual(len(env.calls), 4)
            self.assertEqual(env.calls[0]["command"], "pwd -P")
            self.assertTrue(str(env.calls[1]["command"]).startswith("find . -maxdepth 3"))
            self.assertIn("python -m unittest", str(env.calls[2]["command"]))
            self.assertEqual(env.calls[2]["cwd"], "/workspace")
            self.assertTrue(str(env.calls[3]["command"]).startswith("find . -maxdepth 3"))
            trajectory = json.loads((root / "trajectory.json").read_text())
            self.assertEqual(trajectory["runtime_subject"], "rust-core")
            self.assertEqual(trajectory["steps"][0]["type"], "tool_request")
            self.assertEqual(trajectory["steps"][1]["type"], "tool_result")
            assert context.metadata is not None
            self.assertTrue(context.metadata["completion_evidence"]["ready"])
            self.assertEqual(context.metadata["usage"]["total_tokens"], 14)
            self.assertEqual(context.metadata["working_directory"], "/workspace")

    def test_model_run_adopts_project_created_after_sidecar_start(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(
                root,
                """import json, sys
start = json.loads(sys.stdin.readline())
assert start["working_directory"] == "/workspace", start
print(json.dumps({"type":"tool_request","id":"clone","command":"git clone fixture pyknotid","timeout_sec":30}), flush=True)
clone = json.loads(sys.stdin.readline())
assert clone["next_working_directory"] == "/workspace/pyknotid", clone
print(json.dumps({"type":"tool_request","id":"install","command":"pip install -e .","timeout_sec":30}), flush=True)
install = json.loads(sys.stdin.readline())
assert install["working_directory"] == "/workspace/pyknotid", install
print(json.dumps({"type":"finished","final_text":"verified","execution_contract_sha256":start["execution_contract_sha256"],"completion_evidence":{"ready":True},"usage":{}}), flush=True)
""",
            )
            environment = FakeEnvironment(
                results=[
                    ExecResult(stdout="cloned", stderr="", return_code=0),
                    ExecResult(stdout="installed", stderr="", return_code=0),
                ],
                project_manifests_after_command={
                    "git clone fixture pyknotid": "./pyknotid/setup.py\0"
                },
            )
            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            asyncio.run(agent.run("Clone, fix, install, and verify", environment, AgentContext()))

            commands = [str(call["command"]) for call in environment.calls]
            self.assertTrue(any("git clone fixture pyknotid" in command for command in commands))
            install_call = next(
                call
                for call in environment.calls
                if "pip install -e ." in str(call["command"])
            )
            self.assertEqual(install_call["cwd"], "/workspace/pyknotid")

    def test_model_run_inherits_harbor_network_policy(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(
                root,
                """import json, sys
start = json.loads(sys.stdin.readline())
print(json.dumps({"type":"finished","final_text":"ok","execution_contract_sha256":start["execution_contract_sha256"],"completion_evidence":{"ready":True},"usage":{},"observed_allow_network":start["allow_network"]}), flush=True)
""",
            )
            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            asyncio.run(
                agent.run(
                    "Install the declared dependency",
                    FakeEnvironment(network_mode="public"),
                    AgentContext(),
                )
            )

            metadata = json.loads((root / "run-metadata.json").read_text())
            self.assertEqual(metadata["network_policy"], "public")
            allow_network, policy = agent._resolve_network_policy(
                FakeEnvironment(network_mode="public")
            )
            self.assertTrue(allow_network)
            self.assertEqual(policy, "public")

    def test_no_network_harbor_policy_remains_denied(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("unused"), model_name="test-model")
        allow_network, policy = agent._resolve_network_policy(
            FakeEnvironment(network_mode="no-network")
        )
        self.assertFalse(allow_network)
        self.assertEqual(policy, "no-network")

    def test_internal_wall_timeout_is_opt_in(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("unused"), model_name="test-model")
        self.assertIsNone(agent._agent_wall_timeout_sec())

        configured = CodeFactoryAgent(
            logs_dir=Path("unused"),
            model_name="test-model",
            extra_env={"CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC": "1800"},
        )
        self.assertEqual(configured._agent_wall_timeout_sec(), 1800)

    def test_headless_default_step_budget_matches_product_execute_mode(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(
                root,
                """import json, sys
start = json.loads(sys.stdin.readline())
assert start["max_steps"] == 80, start["max_steps"]
print(json.dumps({"type":"finished","final_text":"ok","execution_contract_sha256":start["execution_contract_sha256"],"completion_evidence":{"ready":True},"usage":{}}), flush=True)
""",
            )
            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            asyncio.run(agent.run("Complete a long coding task", FakeEnvironment(), AgentContext()))

    def test_sidecar_receives_harbor_wall_time_as_execution_budget(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            logs_dir = root / "build-cython-ext__trial" / "agent"
            logs_dir.mkdir(parents=True)
            sidecar = self._write_sidecar(
                root,
                """import json, sys
start = json.loads(sys.stdin.readline())
assert start["wall_time_budget_sec"] == 900, start
assert start["working_directory"] == "/workspace/pyknotid", start
print(json.dumps({"type":"finished","final_text":"ok","execution_contract_sha256":start["execution_contract_sha256"],"completion_evidence":{"ready":True},"usage":{}}), flush=True)
""",
            )
            agent = CodeFactoryAgent(
                logs_dir=logs_dir,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                    "CODEFACTORY_BENCH_TASK_AGENT_TIMEOUTS_JSON": json.dumps(
                        {"build-cython-ext": 900, "filter-js-from-html": 1800}
                    ),
                },
            )

            environment = FakeEnvironment(project_manifests="./pyknotid/setup.py\0")
            asyncio.run(agent.run("Complete a timed coding task", environment, AgentContext()))

    def test_sidecar_secret_is_passed_only_in_start_message_and_not_logged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(
                root,
                """import json, sys
start = json.loads(sys.stdin.readline())
assert start["api_key"] == "super-secret"
print(json.dumps({"type":"finished","final_text":"ok","execution_contract_sha256":start["execution_contract_sha256"],"completion_evidence":{"ready":True},"usage":{}}), flush=True)
""",
            )
            env = FakeEnvironment()
            context = AgentContext()
            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "super-secret",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            asyncio.run(agent.run("Do the task", env, context))

            all_logs = "\n".join(path.read_text() for path in root.glob("*.json*"))
            self.assertNotIn("super-secret", all_logs)

    def test_malformed_sidecar_protocol_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(
                root,
                """import sys
sys.stdin.readline()
print("not-json", flush=True)
""",
            )
            env = FakeEnvironment()
            context = AgentContext()
            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            with self.assertRaisesRegex(RuntimeError, "protocol"):
                asyncio.run(agent.run("Inspect the workspace", env, context))

            self.assertEqual(len(env.calls), 2)
            self.assertEqual(env.calls[0]["command"], "pwd -P")
            self.assertTrue(str(env.calls[1]["command"]).startswith("find . -maxdepth 3"))

    def test_exited_sidecar_preserves_original_stderr_instead_of_cleanup_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sidecar = self._write_sidecar(root, "raise SystemExit(1)\n")

            class FakeStdin:
                def write(self, _value: bytes) -> None:
                    return None

                async def drain(self) -> None:
                    return None

                def is_closing(self) -> bool:
                    return False

                def close(self) -> None:
                    return None

            class ToolThenEmptyStdout:
                def __init__(self) -> None:
                    self.lines = [
                        json.dumps(
                            {
                                "type": "tool_request",
                                "id": "call-usage",
                                "command": "printf ok",
                                "timeout_sec": 5,
                                "usage": {
                                    "prompt_tokens": 120,
                                    "completion_tokens": 30,
                                    "total_tokens": 150,
                                    "model_requests": 2,
                                },
                            }
                        ).encode("utf-8")
                        + b"\n",
                        json.dumps(
                            {
                                "type": "event",
                                "name": "usage_snapshot",
                                "usage": {
                                    "prompt_tokens": 200,
                                    "completion_tokens": 50,
                                    "total_tokens": 250,
                                    "model_requests": 3,
                                },
                            }
                        ).encode("utf-8")
                        + b"\n",
                        b"",
                    ]

                async def readline(self) -> bytes:
                    return self.lines.pop(0)

            class ErrorStderr:
                async def read(self) -> bytes:
                    return b"model request failed: upstream response body truncated"

            class AlreadyExitedProcess:
                stdin = FakeStdin()
                stdout = ToolThenEmptyStdout()
                stderr = ErrorStderr()
                returncode = 1

                def kill(self) -> None:
                    raise ProcessLookupError()

                async def wait(self) -> int:
                    return 1

            agent = CodeFactoryAgent(
                logs_dir=root,
                model_name="test-model",
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_AGENT_BINARY": str(sidecar),
                },
            )

            with self.assertRaisesRegex(
                RuntimeError,
                "upstream response body truncated",
            ) as raised:
                with patch(
                    "codefactory_bench.agent.asyncio.create_subprocess_exec",
                    new=AsyncMock(return_value=AlreadyExitedProcess()),
                ):
                    asyncio.run(
                        agent.run("Inspect the workspace", FakeEnvironment(), AgentContext())
                    )

            self.assertNotIn("ProcessLookupError", str(raised.exception))
            metadata = json.loads((root / "run-metadata.json").read_text())
            self.assertEqual(metadata["runtime_subject"], "rust-core")
            self.assertEqual(metadata["mode"], "model-backed")
            self.assertEqual(metadata["status"], "failed")
            self.assertEqual(metadata["failure"]["type"], "RuntimeError")
            self.assertEqual(
                metadata["usage"],
                {
                    "prompt_tokens": 200,
                    "completion_tokens": 50,
                    "total_tokens": 250,
                    "model_requests": 3,
                },
            )
            self.assertEqual(metadata["integrity"]["contamination_scan"], "pass")


if __name__ == "__main__":
    unittest.main()
