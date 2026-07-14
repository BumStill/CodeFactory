import asyncio
import json
import tempfile
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
    ) -> None:
        self.calls: list[dict[str, object]] = []
        self.results = list(results or [])
        self.network_policy = FakeNetworkPolicy(network_mode)

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
        if self.results:
            return self.results.pop(0)
        return ExecResult(stdout="ok", stderr="", return_code=0)


class CodeFactoryBenchAgentTest(unittest.TestCase):
    def test_harbor_import_path_and_identity_are_stable(self) -> None:
        self.assertEqual(CodeFactoryAgent.import_path(), "codefactory_bench.agent:CodeFactoryAgent")
        self.assertEqual(CodeFactoryAgent.name(), "codefactory-headless")

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

            self.assertEqual(len(env.calls), 1)
            self.assertEqual(env.calls[0]["command"], "python -m unittest")
            trajectory = json.loads((root / "trajectory.json").read_text())
            self.assertEqual(trajectory["runtime_subject"], "rust-core")
            self.assertEqual(trajectory["steps"][0]["type"], "tool_request")
            self.assertEqual(trajectory["steps"][1]["type"], "tool_result")
            assert context.metadata is not None
            self.assertTrue(context.metadata["completion_evidence"]["ready"])
            self.assertEqual(context.metadata["usage"]["total_tokens"], 14)

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

            asyncio.run(agent.run("Complete a timed coding task", FakeEnvironment(), AgentContext()))

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

            self.assertEqual(env.calls, [])

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

            class EmptyStdout:
                async def readline(self) -> bytes:
                    return b""

            class ErrorStderr:
                async def read(self) -> bytes:
                    return b"model request failed: upstream response body truncated"

            class AlreadyExitedProcess:
                stdin = FakeStdin()
                stdout = EmptyStdout()
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
            self.assertEqual(metadata["integrity"]["contamination_scan"], "pass")


if __name__ == "__main__":
    unittest.main()
