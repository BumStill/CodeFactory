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


def start_fake_chat_server(responses: list[dict[str, object]]):
    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            length = int(self.headers.get("content-length", "0"))
            requests.append(json.loads(self.rfile.read(length).decode("utf-8")))
            response = responses.pop(0)
            body = json.dumps(response).encode("utf-8")
            self.send_response(200)
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
            [assistant_tool_call("printf ok"), assistant_final("done")]
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
                    },
                )

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertEqual(len(requests), 2)
                self.assertEqual(env.calls[0]["command"], "printf ok")
                assert context.metadata is not None
                self.assertEqual(context.metadata["mode"], "model-backed")
                self.assertEqual(context.metadata["tool_calls"], 1)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                self.assertEqual(trajectory["mode"], "model-backed")
                self.assertEqual(trajectory["model"], "fake-model")
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
                    },
                )

                asyncio.run(agent.run("fake Terminal-Bench instruction", env, context))

                self.assertEqual(len(requests), 4)
                self.assertEqual(len(env.calls), 2)
                trajectory = json.loads((tmp_path / "trajectory.json").read_text())
                suppressed_steps = [
                    step for step in trajectory["steps"] if step.get("status") == "suppressed"
                ]
                self.assertEqual(len(suppressed_steps), 1)
                self.assertIn("REPEATED COMMAND SUPPRESSED", suppressed_steps[0]["content"])
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
            step=5,
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

    def test_repeat_suppression_only_targets_simple_inspection_commands(self) -> None:
        self.assertTrue(CodeFactoryAgent._is_repeat_suppression_candidate("cat /app/decomp.c"))
        self.assertFalse(CodeFactoryAgent._is_repeat_suppression_candidate("gcc -o enc enc.c"))
        self.assertFalse(
            CodeFactoryAgent._is_repeat_suppression_candidate("cat /app/decomp.c | head")
        )

    def test_repeat_suppression_groups_file_read_inspections(self) -> None:
        self.assertEqual(
            CodeFactoryAgent._repeat_command_key("cat /app/decomp.c"),
            CodeFactoryAgent._repeat_command_key("head -200 /app/decomp.c"),
        )
        self.assertNotEqual(
            CodeFactoryAgent._repeat_command_key("cat /app/decomp.c"),
            CodeFactoryAgent._repeat_command_key("cat /app/data.txt"),
        )


if __name__ == "__main__":
    unittest.main()
