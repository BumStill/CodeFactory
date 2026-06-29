import asyncio
import json
import threading
import unittest
from http import client as http_client
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from unittest import mock

from harbor.environments.base import ExecResult
from harbor.models.agent.context import AgentContext

from codefactory_bench.agent import CodeFactoryAgent


class FakeEnvironment:
    def __init__(self) -> None:
        self.calls: list[dict[str, object]] = []

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
        return ExecResult(stdout="fake stdout", stderr="", return_code=0)


class FakeEnvironmentWithResults(FakeEnvironment):
    def __init__(self, results: list[ExecResult]) -> None:
        super().__init__()
        self.results = results

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
        return ExecResult(stdout="fake stdout", stderr="", return_code=0)


class FakeEnvironmentRaises(FakeEnvironment):
    def __init__(self, exc: Exception) -> None:
        super().__init__()
        self.exc = exc

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
        raise self.exc


def start_fake_chat_server(responses: list[dict[str, object]]):
    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            length = int(self.headers.get("content-length", "0"))
            requests.append(json.loads(self.rfile.read(length).decode("utf-8")))
            response = responses.pop(0)
            status = int(response.pop("_status", 200))
            body = json.dumps(response).encode("utf-8")
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            return

    server = HTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, requests


def assistant_tool_call(command: str) -> dict[str, object]:
    return {
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "run_shell",
                                "arguments": json.dumps({"command": command}),
                            },
                        }
                    ],
                }
            }
        ]
    }


def assistant_final(content: str) -> dict[str, object]:
    return {"choices": [{"message": {"role": "assistant", "content": content}}]}


def assistant_final_with_usage(
    content: str,
    prompt_tokens: int,
    completion_tokens: int,
) -> dict[str, object]:
    return {
        "choices": [{"message": {"role": "assistant", "content": content}}],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        },
    }


def provider_error(status: int, message: str) -> dict[str, object]:
    return {
        "_status": status,
        "error": {
            "message": message,
            "type": "invalid_request_error",
            "code": "invalid_request_error",
        },
    }


class CodeFactoryBenchAgentTest(unittest.TestCase):
    def test_codefactory_agent_has_harbor_import_path(self) -> None:
        self.assertEqual(
            CodeFactoryAgent.import_path(),
            "codefactory_bench.agent:CodeFactoryAgent",
        )
        self.assertEqual(CodeFactoryAgent.name(), "codefactory-headless")

    def test_codefactory_agent_run_records_diagnostics(self) -> None:
        with self.subTest("records Harbor environment diagnostics"):
            import tempfile
            from pathlib import Path

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(logs_dir=tmp_path, model_name=None)

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertTrue(
                    env.calls,
                    "agent should execute inside the Harbor environment",
                )
                self.assertEqual(
                    (tmp_path / "instruction.txt").read_text(),
                    "fake Terminal-Bench instruction",
                )
                self.assertEqual(
                    (tmp_path / "codefactory-headless.txt").read_text(),
                    "fake stdout",
                )
                self.assertIsNotNone(context.metadata)
                assert context.metadata is not None
                self.assertEqual(context.metadata["mode"], "baseline-no-model")
                self.assertEqual(context.metadata["exec_return_code"], 0)

    def test_codefactory_agent_model_backed_loop_runs_shell_tool(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("printf ok"),
                assistant_final_with_usage("done", 17, 5),
            ]
        )
        try:
            import tempfile
            from pathlib import Path

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_INSPECTION_ROUNDS": "99",
                        "CODEFACTORY_BENCH_MAX_OUTPUT_TOKENS": "1234",
                    },
                )

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(requests[0]["max_tokens"], 1234)
                self.assertEqual(env.calls[0]["command"], "printf ok")
                assert context.metadata is not None
                self.assertEqual(context.metadata["mode"], "model-backed")
                self.assertEqual(context.metadata["tool_calls"], 1)
                self.assertEqual(
                    context.metadata["usage"],
                    {"prompt_tokens": 17, "completion_tokens": 5, "total_tokens": 22},
                )
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                self.assertEqual(trajectory["mode"], "model-backed")
                self.assertEqual(trajectory["model"], "fake-model")
                self.assertEqual(
                    trajectory["usage"],
                    {"prompt_tokens": 17, "completion_tokens": 5, "total_tokens": 22},
                )
                self.assertEqual(
                    json.loads((tmp_path / "usage.json").read_text()),
                    {"prompt_tokens": 17, "completion_tokens": 5, "total_tokens": 22},
                )
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_model_backed_policy_denies_network_tool(self) -> None:
        server, requests = start_fake_chat_server(
            [assistant_tool_call("curl http://example.com"), assistant_final("done")]
        )
        try:
            import tempfile
            from pathlib import Path

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_INSPECTION_ROUNDS": "99",
                    },
                )

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(env.calls, [])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                denied_steps = [
                    step for step in trajectory["steps"] if step.get("status") == "denied"
                ]
                self.assertEqual(len(denied_steps), 1)
                self.assertEqual(
                    denied_steps[0]["policy"]["reason"],
                    "network/exfiltration tool disabled",
                )
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_suppresses_repeated_read_only_commands(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("cat /app/decomp.c"),
                assistant_tool_call("cat /app/decomp.c"),
                assistant_tool_call("cat /app/decomp.c"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_INSPECTION_ROUNDS": "99",
                    },
                )

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertEqual(len(requests), 4)
                self.assertEqual(len(env.calls), 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                suppressed_steps = [
                    step for step in trajectory["steps"] if step.get("status") == "suppressed"
                ]
                self.assertEqual(len(suppressed_steps), 2)
                self.assertIn("REPEATED COMMAND SUPPRESSED", suppressed_steps[0]["content"])
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_requires_implementation_after_inspection_budget(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("cat /app/decomp.c"),
                assistant_tool_call("wc -c /app/data.txt"),
                assistant_tool_call("head -20 /app/decomp.c"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_INSPECTION_ROUNDS": "2",
                        "CODEFACTORY_BENCH_NO_ACTION_RETRIES": "0",
                    },
                )

                asyncio.run(agent.run("write data.comp", env, context))

                self.assertEqual(len(requests), 4)
                self.assertEqual(len(env.calls), 2)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                implementation_required = [
                    step
                    for step in trajectory["steps"]
                    if step.get("status") == "implementation-required"
                ]
                self.assertEqual(len(implementation_required), 1)
                self.assertIn("IMPLEMENTATION REQUIRED", implementation_required[0]["content"])
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_requires_implementation_without_artifact_hint(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("cat /app/input.txt"),
                assistant_tool_call("head -20 /app/input.txt"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_INSPECTION_ROUNDS": "1",
                    },
                )

                asyncio.run(agent.run("produce the requested answer", env, context))

                self.assertEqual(len(requests), 3)
                self.assertEqual(len(env.calls), 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                implementation_required = [
                    step
                    for step in trajectory["steps"]
                    if step.get("status") == "implementation-required"
                ]
                self.assertEqual(len(implementation_required), 1)
                assert context.metadata is not None
                self.assertEqual(context.metadata["implementation_required_blocks"], 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_suppresses_second_repeated_read_by_default(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("cat /app/input.txt"),
                assistant_tool_call("cat /app/input.txt"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("inspect input then solve", env, context))

                self.assertEqual(len(env.calls), 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                suppressed = [
                    step for step in trajectory["steps"] if step.get("status") == "suppressed"
                ]
                self.assertEqual(len(suppressed), 1)
                self.assertIn("already ran 1 time", suppressed[0]["content"])
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_preflights_repeated_missing_command(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("missing-tool --version"),
                assistant_tool_call("missing-tool --version"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironmentWithResults(
                    [
                        ExecResult(
                            stdout="",
                            stderr="/bin/sh: missing-tool: command not found\n",
                            return_code=127,
                        )
                    ]
                )
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("solve without relying on unavailable tools", env, context))

                self.assertEqual(len(env.calls), 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                preflight_blocks = [
                    step
                    for step in trajectory["steps"]
                    if step.get("status") == "preflight-blocked"
                ]
                self.assertEqual(len(preflight_blocks), 1)
                self.assertIn("missing-tool", preflight_blocks[0]["content"])
                assert context.metadata is not None
                self.assertEqual(context.metadata["preflight_blocks"], 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_retries_model_timeout_with_implementation_prompt(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            env = FakeEnvironment()
            context = AgentContext()
            agent = CodeFactoryAgent(
                logs_dir=tmp_path,
                model_name=None,
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_MODEL": "fake-model",
                    "CODEFACTORY_BENCH_MODEL_TIMEOUT_RETRIES": "1",
                    "CODEFACTORY_BENCH_FINAL_VERIFY_RETRIES": "0",
                },
            )
            calls: list[list[dict[str, object]]] = []

            def fake_chat_completion(
                messages: list[dict[str, object]],
                model: str,
                timeout_sec: int | None = None,
                force_tool: bool = False,
            ) -> dict[str, object]:
                calls.append(messages)
                if len(calls) == 1:
                    raise TimeoutError("fake timeout")
                if len(calls) == 2:
                    return assistant_tool_call(
                        "cat > /app/data.comp <<'EOF'\nplaceholder\nEOF"
                    )["choices"][0]["message"]
                return assistant_final("done")["choices"][0]["message"]

            agent._chat_completion = fake_chat_completion  # type: ignore[method-assign]

            asyncio.run(agent.run("write data.comp", env, context))

            self.assertEqual(len(calls), 3)
            self.assertEqual(len(env.calls), 1)
            self.assertIn("data.comp", env.calls[0]["command"])
            trajectory = json.loads((tmp_path / "trajectory.json").read_text())
            self.assertTrue(any(step.get("role") == "model-error" for step in trajectory["steps"]))
            self.assertTrue(
                any(
                    "Timeout recovery:" in step.get("content", "")
                    for step in trajectory["steps"]
                    if step.get("role") == "system-reminder"
                )
            )

    def test_codefactory_agent_records_shell_timeout_without_aborting(self) -> None:
        server, requests = start_fake_chat_server(
            [assistant_tool_call("python3 -c 'print(1)'"), assistant_final("done")]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironmentRaises(
                    RuntimeError("Command timed out after 5 seconds")
                )
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(len(env.calls), 1)
                assert context.metadata is not None
                self.assertEqual(context.metadata["exec_errors"], 1)
                self.assertEqual(context.metadata["command_timeouts"], 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                exec_errors = [
                    step
                    for step in trajectory["steps"]
                    if step.get("status") == "exec-error"
                ]
                self.assertEqual(len(exec_errors), 1)
                self.assertEqual(exec_errors[0]["error_type"], "command-timeout")
                self.assertIn("Command timed out", exec_errors[0]["content"])
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_prompts_repair_after_failed_self_check(self) -> None:
        server, requests = start_fake_chat_server(
            [assistant_tool_call("pytest -q"), assistant_final("done")]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironmentWithResults(
                    [
                        ExecResult(
                            stdout=(
                                "F\n"
                                "=================================== FAILURES ===================================\n"
                                "AssertionError: expected optimized query plan\n"
                                "1 failed in 0.12s\n"
                            ),
                            stderr="",
                            return_code=1,
                        )
                    ]
                )
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("fix the implementation", env, context))

                self.assertEqual(len(requests), 2)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                reminders = [
                    step.get("content", "")
                    for step in trajectory["steps"]
                    if step.get("role") == "system-reminder"
                ]
                self.assertTrue(
                    any("latest self-check failed" in reminder for reminder in reminders)
                )
                repair_goals = [
                    step
                    for step in trajectory["steps"]
                    if step.get("role") == "repair-goal"
                ]
                self.assertEqual(len(repair_goals), 1)
                self.assertEqual(repair_goals[0]["goal"]["kind"], "assertion-failure")
                assert context.metadata is not None
                self.assertEqual(context.metadata["repair_goal_count"], 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_requires_final_verification_after_artifact(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("cat > /app/data.comp <<'EOF'\nplaceholder\nEOF"),
                assistant_final("done"),
                assistant_tool_call("cmp -s /app/data.comp /app/data.comp && echo verification-ok"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("write data.comp", env, context))

                self.assertEqual(len(requests), 4)
                self.assertEqual(len(env.calls), 2)
                self.assertIn("data.comp", env.calls[0]["command"])
                self.assertIn("verification-ok", env.calls[1]["command"])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                self.assertTrue(
                    any(
                        "Final-before-verify gate:" in step.get("content", "")
                        for step in trajectory["steps"]
                        if step.get("role") == "system-reminder"
                    )
                )
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_requires_supervision_for_foreground_service(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("python -m http.server 8000"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("start and test the web service", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(env.calls, [])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                supervision_required = [
                    step
                    for step in trajectory["steps"]
                    if step.get("status") == "service-supervision-required"
                ]
                self.assertEqual(len(supervision_required), 1)
                self.assertIn("readiness check", supervision_required[0]["content"])
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_suppresses_unbounded_long_commands(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("tail -f /tmp/server.log"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("inspect logs", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(env.calls, [])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                long_blocks = [
                    step
                    for step in trajectory["steps"]
                    if step.get("status") == "long-command-policy-required"
                ]
                self.assertEqual(len(long_blocks), 1)
                assert context.metadata is not None
                self.assertEqual(context.metadata["long_command_blocks"], 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_records_background_service_lifecycle(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call(
                    "python -m http.server 8000 > /tmp/cf-server.log 2>&1 & "
                    "echo $! > /tmp/cf-server.pid; "
                    "python3 - <<'PY'\nprint('ready')\nPY"
                ),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(agent.run("start and test the web service", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(len(env.calls), 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                lifecycle_steps = [
                    step
                    for step in trajectory["steps"]
                    if step.get("background_process")
                ]
                self.assertEqual(len(lifecycle_steps), 1)
                lifecycle = lifecycle_steps[0]["background_process"]
                self.assertTrue(lifecycle["log_recorded"])
                self.assertTrue(lifecycle["pid_recorded"])
                self.assertTrue(lifecycle["readiness_checked"])
                assert context.metadata is not None
                self.assertEqual(context.metadata["background_process_count"], 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_chat_completion_falls_back_when_provider_rejects_forced_tool_choice(
        self,
    ) -> None:
        server, requests = start_fake_chat_server(
            [
                provider_error(400, "Thinking mode does not support this tool_choice"),
                assistant_tool_call("cat /app/decomp.c"),
            ]
        )
        try:
            agent = CodeFactoryAgent(
                logs_dir=Path("/tmp"),
                model_name=None,
                extra_env={
                    "CODEFACTORY_BENCH_API_KEY": "test-key",
                    "CODEFACTORY_BENCH_BASE_URL": (
                        f"http://127.0.0.1:{server.server_port}/v1"
                    ),
                },
            )

            message = agent._chat_completion(
                [{"role": "user", "content": "write data.comp"}],
                "fake-model",
                timeout_sec=5,
                force_tool=True,
            )

            self.assertEqual(message["tool_calls"][0]["function"]["name"], "run_shell")
            self.assertEqual(
                requests[0]["tool_choice"],
                {"type": "function", "function": {"name": "run_shell"}},
            )
            self.assertEqual(requests[1]["tool_choice"], "auto")
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_recovers_empty_response_before_artifact_exists(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_final(""),
                assistant_tool_call("cat > /app/data.comp <<'EOF'\nplaceholder\nEOF"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_NO_ACTION_RETRIES": "1",
                        "CODEFACTORY_BENCH_FINAL_VERIFY_RETRIES": "0",
                    },
                )

                asyncio.run(agent.run("write data.comp", env, context))

                self.assertEqual(len(requests), 3)
                self.assertEqual(
                    requests[0]["tool_choice"],
                    {"type": "function", "function": {"name": "run_shell"}},
                )
                self.assertEqual(len(env.calls), 1)
                self.assertIn("data.comp", env.calls[0]["command"])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                self.assertTrue(
                    any(
                        "No-action recovery:" in step.get("content", "")
                        for step in trajectory["steps"]
                        if step.get("role") == "system-reminder"
                    )
                )
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_recovers_empty_response_after_auxiliary_compile(self) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call("cd /app && gcc -o decomp decomp.c 2>&1"),
                assistant_final(""),
                assistant_tool_call("cat > /app/data.comp <<'EOF'\nplaceholder\nEOF"),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironment()
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                        "CODEFACTORY_BENCH_NO_ACTION_RETRIES": "1",
                        "CODEFACTORY_BENCH_FINAL_VERIFY_RETRIES": "0",
                    },
                )

                asyncio.run(agent.run("write data.comp", env, context))

                self.assertEqual(len(requests), 4)
                self.assertEqual(len(env.calls), 2)
                self.assertIn("gcc -o decomp", env.calls[0]["command"])
                self.assertIn("data.comp", env.calls[1]["command"])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                self.assertTrue(
                    any(
                        "No-action recovery:" in step.get("content", "")
                        for step in trajectory["steps"]
                        if step.get("role") == "system-reminder"
                    )
                )
        finally:
            server.shutdown()
            server.server_close()

    def test_codefactory_agent_auto_repairs_write_compressor_protocol_failure(
        self,
    ) -> None:
        server, requests = start_fake_chat_server(
            [
                assistant_tool_call(
                    "cp /app/data.txt /app/data.comp && "
                    "cat /app/data.comp | /app/decomp > /tmp/out || echo verification-failed"
                ),
                assistant_final("done"),
            ]
        )
        try:
            import tempfile

            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                env = FakeEnvironmentWithResults(
                    [
                        ExecResult(
                            stdout=(
                                "Segmentation fault (core dumped)\n"
                                "verification-failed\n"
                            ),
                            stderr="",
                            return_code=0,
                        ),
                        ExecResult(
                            stdout=(
                                "codefactory-auto-repair wrote /app/data.comp "
                                "bytes=2476 tokens=1416\n"
                                "2476 /app/data.comp\n"
                                "codefactory-auto-repair-ok\n"
                            ),
                            stderr="",
                            return_code=0,
                        ),
                    ]
                )
                context = AgentContext()
                agent = CodeFactoryAgent(
                    logs_dir=tmp_path,
                    model_name=None,
                    extra_env={
                        "CODEFACTORY_BENCH_API_KEY": "test-key",
                        "CODEFACTORY_BENCH_BASE_URL": (
                            f"http://127.0.0.1:{server.server_port}/v1"
                        ),
                        "CODEFACTORY_BENCH_MODEL": "fake-model",
                    },
                )

                asyncio.run(
                    agent.run(
                        "Write me data.comp such that cat data.comp | /app/decomp gives data.txt.",
                        env,
                        context,
                    )
                )

                self.assertEqual(len(requests), 2)
                self.assertEqual(len(env.calls), 2)
                self.assertIn("cp /app/data.txt", env.calls[0]["command"])
                self.assertIn("codefactory_wc_repair.c", env.calls[1]["command"])
                self.assertIn("parse_tokens", env.calls[1]["command"])
                self.assertIn("codefactory-auto-repair-ok", env.calls[1]["command"])
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                self.assertTrue(
                    any(step.get("status") == "auto-repair-ok" for step in trajectory["steps"])
                )
        finally:
            server.shutdown()
            server.server_close()

    def test_benchmark_policy_allows_heredoc_source_with_network_like_text(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("/tmp"))
        command = """cat > /app/enc.c << 'EOF'
#include <stdio.h>
int main(void) {
  int nc = 0;
  const char *example = "curl appears as inert source text";
  return nc;
}
EOF
gcc -o /app/enc /app/enc.c
"""

        decision = agent._classify_shell_command(command)

        self.assertEqual(decision["action"], "allow")

    def test_benchmark_policy_still_denies_real_network_command(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("/tmp"))

        decision = agent._classify_shell_command("printf before; curl http://example.com")

        self.assertEqual(decision["action"], "deny")
        self.assertEqual(decision["reason"], "network/exfiltration tool disabled")

    def test_agent_extracts_output_artifact_hint_from_instruction(self) -> None:
        hint = CodeFactoryAgent._artifact_hint_from_instruction(
            "Write me data.comp that's compressed such that running cat data.comp works."
        )

        self.assertEqual(hint, "data.comp")

    def test_agent_extracts_exact_stdout_verification_hint(self) -> None:
        hint = CodeFactoryAgent._verification_hint_from_instruction(
            "running cat data.comp | /app/decomp gives exactly data.txt."
        )

        assert hint is not None
        self.assertIn("cat data.comp | /app/decomp", hint)
        self.assertIn("cmp -s /tmp/codefactory-bench-output data.txt", hint)

    def test_agent_emits_phase_progress_reminder_after_inspection(self) -> None:
        reminder = CodeFactoryAgent._phase_progress_reminder(
            step=2,
            max_steps=20,
            artifact_hint="data.comp",
        )

        assert reminder is not None
        self.assertIn("Inspection phase should be over", reminder)
        self.assertIn("data.comp", reminder)

    def test_agent_emits_budget_reminder_near_step_limit(self) -> None:
        reminder = CodeFactoryAgent._remaining_budget_reminder(
            step=17,
            max_steps=20,
            artifact_hint="data.comp",
        )

        assert reminder is not None
        self.assertIn("only 3 tool-call rounds", reminder)
        self.assertIn("data.comp", reminder)
        self.assertIn("create it now", reminder)

    def test_tool_output_limit_is_configurable_for_code_inspection(self) -> None:
        result = CodeFactoryAgent._format_exec_result(
            0,
            "abcdefghij",
            "",
            output_limit=8,
        )

        self.assertIn("abcdefgh", result)
        self.assertIn("[truncated 2 bytes]", result)

    def test_model_timeout_is_bounded_by_remaining_wall_clock(self) -> None:
        agent = CodeFactoryAgent(
            logs_dir=Path("/tmp"),
            extra_env={"CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC": "90"},
        )

        self.assertEqual(agent._bounded_model_timeout(12.8), 12)
        self.assertEqual(agent._bounded_model_timeout(-1), 1)

    def test_chat_completion_maps_incomplete_reads_to_controlled_timeout(self) -> None:
        class BrokenResponse:
            def __enter__(self):
                return self

            def __exit__(self, *args: object) -> None:
                return None

            def read(self) -> bytes:
                raise http_client.IncompleteRead(b"")

        agent = CodeFactoryAgent(
            logs_dir=Path("/tmp"),
            extra_env={
                "CODEFACTORY_BENCH_API_KEY": "test-key",
                "CODEFACTORY_BENCH_BASE_URL": "http://127.0.0.1:1/v1",
            },
        )

        with mock.patch("codefactory_bench.agent.request.urlopen", return_value=BrokenResponse()):
            with self.assertRaises(TimeoutError):
                agent._chat_completion([{"role": "user", "content": "hi"}], "fake-model")

    def test_chat_messages_compact_large_tool_call_history(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("/tmp"))
        large_command = "cat > enc.c <<'EOF'\n" + ("int x;\n" * 2000) + "EOF"
        messages = [
            {"role": "system", "content": "system prompt"},
            {"role": "user", "content": "create data.comp"},
            {"role": "user", "content": "Output artifact hint: create `data.comp`."},
            {
                "role": "user",
                "content": (
                    "Verification hint: run `cat data.comp | /app/decomp > /tmp/out`."
                ),
            },
            {
                "role": "user",
                "content": "Inspection phase should be over. Create `data.comp` now.",
            },
            {
                "role": "user",
                "content": "Timeout recovery: return one implementation tool call.",
            },
            {
                "role": "user",
                "content": "No-action recovery: return one implementation tool call.",
            },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "run_shell",
                            "arguments": json.dumps({"command": large_command}),
                        },
                    }
                ],
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "return_code=0\nstdout:\nSegmentation fault (core dumped)",
            },
        ]
        trajectory = [
            {
                "role": "tool",
                "tool": "run_shell",
                "command": large_command,
                "status": "ok",
                "content": "return_code=0\nstdout:\nSegmentation fault (core dumped)",
            }
        ]

        compacted = agent._chat_messages_for_model(messages, trajectory)
        serialized = json.dumps(compacted)

        self.assertNotIn("int x;", serialized)
        self.assertNotIn('"role": "tool"', serialized)
        self.assertIn("Segmentation fault", serialized)
        self.assertIn("Verification hint", serialized)
        self.assertIn("Inspection phase should be over", serialized)
        self.assertIn("Timeout recovery", serialized)
        self.assertIn("No-action recovery", serialized)

    def test_repair_hint_focuses_semantic_self_check_failures(self) -> None:
        hint = CodeFactoryAgent._repair_hint_from_tool_result(
            {"content": "stdout:\nSegmentation fault (core dumped)"},
            "data.comp",
        )

        assert hint is not None
        self.assertIn("Repair focus", hint)
        self.assertIn("data.comp", hint)

    def test_repair_hint_handles_missing_tools(self) -> None:
        hint = CodeFactoryAgent._repair_hint_from_tool_result(
            {"content": "stdout:\nbash: line 1: xxd: command not found"},
            "data.comp",
        )

        assert hint is not None
        self.assertIn("missing tool", hint)
        self.assertIn("Do not retry", hint)

    def test_repair_hint_escalates_implementation_required(self) -> None:
        hint = CodeFactoryAgent._repair_hint_from_tool_result(
            {"content": "IMPLEMENTATION REQUIRED: create the artifact now."},
            "data.comp",
        )

        assert hint is not None
        self.assertIn("Repair focus", hint)
        self.assertIn("data.comp", hint)
        self.assertIn("do not read task files again", hint)

    def test_auto_repair_command_targets_write_compressor_protocol_failures(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("/tmp"))
        command = agent._auto_repair_command_from_tool_result(
            {
                "command": "cat /app/data.comp | /app/decomp",
                "content": "Segmentation fault (core dumped)\nverification-failed",
            },
            "data.comp",
            {"auto_protocol_repairs": 0},
        )

        assert command is not None
        self.assertIn("codefactory_wc_repair.c", command)
        self.assertIn("put_integer(token_count, 9, 0)", command)
        self.assertIn("codefactory-auto-repair-ok", command)

    def test_repeat_suppression_only_targets_simple_inspection_commands(self) -> None:
        self.assertTrue(CodeFactoryAgent._is_repeat_suppression_candidate("cat /app/decomp.c"))
        self.assertFalse(CodeFactoryAgent._is_repeat_suppression_candidate("gcc -o enc enc.c"))
        self.assertTrue(
            CodeFactoryAgent._is_repeat_suppression_candidate("cat /app/decomp.c | head")
        )
        self.assertTrue(
            CodeFactoryAgent._is_repeat_suppression_candidate(
                "wc -c /app/data.txt && head -c 500 /app/data.txt"
            )
        )
        self.assertFalse(
            CodeFactoryAgent._is_repeat_suppression_candidate(
                "cd /app && gcc -o decomp decomp.c 2>&1"
            )
        )

    def test_repeat_suppression_groups_file_read_inspections(self) -> None:
        self.assertEqual(
            CodeFactoryAgent._repeat_command_key("cat /app/decomp.c"),
            CodeFactoryAgent._repeat_command_key("head -200 /app/decomp.c"),
        )
        self.assertEqual(
            CodeFactoryAgent._repeat_command_key("cat /app/decomp.c"),
            CodeFactoryAgent._repeat_command_key("tail -n +1 /app/decomp.c | head -120"),
        )
        self.assertNotEqual(
            CodeFactoryAgent._repeat_command_key("cat /app/decomp.c"),
            CodeFactoryAgent._repeat_command_key("cat /app/data.txt"),
        )

    def test_implementation_gate_allows_generation_commands(self) -> None:
        agent = CodeFactoryAgent(
            logs_dir=Path("/tmp"),
            extra_env={"CODEFACTORY_BENCH_INSPECTION_ROUNDS": "2"},
        )

        self.assertTrue(
            agent._requires_implementation_before_inspection(
                "head -20 /app/decomp.c",
                {"implementation_started": False, "artifact_started": False},
                step=2,
                artifact_hint="data.comp",
            )
        )
        self.assertFalse(
            agent._requires_implementation_before_inspection(
                "cat > /app/enc.c <<'EOF'\nint main(void){return 0;}\nEOF",
                {"implementation_started": False, "artifact_started": False},
                step=2,
                artifact_hint="data.comp",
            )
        )
        self.assertTrue(
            agent._requires_implementation_before_inspection(
                "wc -c /app/data.txt && head -c 500 /app/data.txt",
                {"implementation_started": False, "artifact_started": False},
                step=2,
                artifact_hint="data.comp",
            )
        )
        self.assertFalse(
            agent._requires_implementation_before_inspection(
                "cd /app && gcc -o decomp decomp.c 2>&1",
                {"implementation_started": False, "artifact_started": False},
                step=2,
                artifact_hint="data.comp",
            )
        )
        self.assertTrue(CodeFactoryAgent._is_implementation_attempt("gcc -o /app/enc /app/enc.c"))
        self.assertFalse(
            CodeFactoryAgent._is_artifact_attempt(
                "gcc -o /app/decomp /app/decomp.c",
                "data.comp",
            )
        )
        self.assertTrue(
            CodeFactoryAgent._is_artifact_attempt(
                "cat > /app/data.comp <<'EOF'\nplaceholder\nEOF",
                "data.comp",
            )
        )
        self.assertTrue(
            CodeFactoryAgent._is_artifact_attempt(
                "python3 - <<'PY'\nopen('/app/data.comp','wb').write(b'x')\nPY",
                "data.comp",
            )
        )

    def test_artifact_command_gate_blocks_non_artifact_commands_after_repeated_blocks(
        self,
    ) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("/tmp"))
        state = {
            "implementation_started": False,
            "artifact_started": False,
            "implementation_required_count": 2,
        }

        self.assertTrue(
            agent._requires_artifact_command(
                "cat /app/decomp.c",
                state,
                step=3,
                artifact_hint="data.comp",
            )
        )
        self.assertTrue(
            agent._requires_artifact_command(
                "cd /app && gcc -o decomp decomp.c 2>&1",
                state,
                step=3,
                artifact_hint="data.comp",
            )
        )
        self.assertFalse(
            agent._requires_artifact_command(
                "python3 - <<'PY'\nopen('/app/data.comp','wb').write(b'x')\nPY",
                state,
                step=3,
                artifact_hint="data.comp",
            )
        )

    def test_artifact_command_gate_blocks_late_non_artifact_probe(self) -> None:
        agent = CodeFactoryAgent(logs_dir=Path("/tmp"))
        state = {
            "implementation_started": False,
            "artifact_started": False,
            "implementation_required_count": 0,
        }

        self.assertTrue(
            agent._requires_artifact_command(
                "printf 'A' | /app/decomp | od -c | head -5",
                state,
                step=5,
                artifact_hint="data.comp",
            )
        )

    def test_semantic_command_failure_becomes_repair_goal(self) -> None:
        goal = CodeFactoryAgent._repair_goal_from_tool_result(
            {
                "role": "tool",
                "status": "semantic-failure",
                "command": "pip install datasets transformers 2>&1 | tail -20",
                "content": (
                    "return_code=0\n"
                    "stdout:\n"
                    "ERROR: Could not install packages due to an OSError: "
                    "[Errno 28] No space left on device\n"
                ),
            },
            artifact_hint=None,
        )

        self.assertIsNotNone(goal)
        assert goal is not None
        self.assertEqual(goal["kind"], "semantic-command-failure")
        self.assertIn("No space left", goal["failure"])


if __name__ == "__main__":
    unittest.main()
