import asyncio
import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
