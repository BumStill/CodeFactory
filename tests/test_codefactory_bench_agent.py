import asyncio
import unittest

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


class CodeFactoryBenchAgentTest(unittest.TestCase):
    def test_codefactory_agent_has_harbor_import_path(self) -> None:
        self.assertEqual(
            CodeFactoryAgent.import_path(),
            "codefactory_bench.agent:CodeFactoryAgent",
        )
        self.assertEqual(CodeFactoryAgent.name(), "codefactory-headless-baseline")

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


if __name__ == "__main__":
    unittest.main()
