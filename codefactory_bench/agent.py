from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class CodeFactoryAgent(BaseAgent):
    """Minimal Harbor adapter for closing CodeFactory benchmark evaluation loops.

    This baseline does not load user credentials or call a model. It proves that
    Harbor can execute a CodeFactory-owned agent class in the task sandbox and
    that CodeFactory can import the resulting job artifacts.
    """

    SUPPORTS_WINDOWS: bool = False

    def __init__(
        self,
        logs_dir: Path,
        model_name: str | None = None,
        extra_env: dict[str, str] | None = None,
        agent_timeout_sec: float | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(logs_dir=logs_dir, model_name=model_name, **kwargs)
        self._extra_env = extra_env or {}
        self._agent_timeout_sec = int(agent_timeout_sec or 120)

    @staticmethod
    def name() -> str:
        return "codefactory-headless-baseline"

    def version(self) -> str:
        package_json = Path(__file__).resolve().parents[1] / "package.json"
        try:
            return json.loads(package_json.read_text()).get("version") or "0.0.0"
        except Exception:
            return "0.0.0"

    async def setup(self, environment: BaseEnvironment) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        (self.logs_dir / "setup.json").write_text(
            json.dumps(
                {
                    "agent": self.name(),
                    "mode": "baseline-no-model",
                    "model_name": self.model_name,
                },
                indent=2,
                sort_keys=True,
            )
        )

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)

        instruction_hash = hashlib.sha256(instruction.encode("utf-8")).hexdigest()
        (self.logs_dir / "instruction.txt").write_text(instruction)
        (self.logs_dir / "instruction.sha256").write_text(instruction_hash)

        diagnostic_command = r"""
set -u
echo "agent=codefactory-headless-baseline"
echo "mode=baseline-no-model"
echo "cwd=$(pwd)"
echo "user=$(id -un 2>/dev/null || true)"
echo "uid=$(id -u 2>/dev/null || true)"
echo "kernel=$(uname -a 2>/dev/null || true)"
echo "workspace_files="
find . -maxdepth 2 -type f 2>/dev/null | sed 's#^\./##' | sort | head -200
"""

        env = {
            "CODEFACTORY_BENCHMARK_POLICY": "benchmark-sandbox",
            "CODEFACTORY_AGENT_MODE": "baseline-no-model",
            **self._extra_env,
        }
        result = await environment.exec(
            diagnostic_command,
            env=env,
            timeout_sec=self._agent_timeout_sec,
        )

        output = result.stdout or ""
        if result.stderr:
            output = f"{output}\n[stderr]\n{result.stderr}"
        (self.logs_dir / "codefactory-headless.txt").write_text(output)

        context.metadata = {
            **(context.metadata or {}),
            "agent": self.name(),
            "mode": "baseline-no-model",
            "instruction_sha256": instruction_hash,
            "exec_return_code": result.return_code,
            "diagnostic_stdout_bytes": len(result.stdout or ""),
            "diagnostic_stderr_bytes": len(result.stderr or ""),
        }
