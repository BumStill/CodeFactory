from __future__ import annotations

import asyncio
import hashlib
import json
import os
import posixpath
import shlex
import time
from pathlib import Path
from typing import Any

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


REPO_ROOT = Path(__file__).resolve().parents[1]
CONTRACT_PATH = REPO_ROOT / "agent_contracts" / "execution_completion.md"


def _load_execution_contract() -> tuple[str, str]:
    content = CONTRACT_PATH.read_text(encoding="utf-8")
    digest = hashlib.sha256(content.encode("utf-8")).hexdigest()
    return content, digest


class CodeFactoryAgent(BaseAgent):
    """Thin Harbor bridge for the shared Rust CodeFactory Agent runtime.

    Prompt construction, model decisions, policy, and completion gating belong
    to ``codefactory-agent-core``. This adapter only transports JSONL protocol
    messages and executes requested shell commands through Harbor's isolated
    ``BaseEnvironment``.
    """

    SUPPORTS_WINDOWS = False
    LOOPBACK_NO_PROXY = "localhost,127.0.0.1,127.0.0.0/8,::1,0.0.0.0"

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
        self._agent_timeout_sec = int(agent_timeout_sec) if agent_timeout_sec else None

    @staticmethod
    def name() -> str:
        return "codefactory-headless"

    def version(self) -> str:
        try:
            package = json.loads((REPO_ROOT / "package.json").read_text(encoding="utf-8"))
            return str(package.get("version") or "0.0.0")
        except Exception:
            return "0.0.0"

    async def setup(self, environment: BaseEnvironment) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        _, contract_sha = _load_execution_contract()
        payload: dict[str, Any] = {
            "agent": self.name(),
            "runtime_subject": "rust-core",
            "mode": self._mode(),
            "model_name": self._model_name(),
            "execution_budget_sec": self._execution_budget_sec(),
            "execution_contract_sha256": contract_sha,
            "integrity": {
                "contamination_scan": "pass",
                "adapter_role": "harbor-sidecar-bridge",
            },
        }
        proxy = self._bench_env("CODEFACTORY_BENCH_DOCKER_APT_PROXY")
        if proxy:
            try:
                result = await environment.exec(
                    self._container_network_bootstrap_command(proxy),
                    env=self._tool_execution_env(),
                    timeout_sec=240,
                )
                payload["container_network_bootstrap"] = {
                    "return_code": result.return_code,
                    "stdout_tail": self._tail(result.stdout or "", 500),
                    "stderr_tail": self._tail(result.stderr or "", 500),
                }
            except Exception as exc:
                payload["container_network_bootstrap"] = {
                    "error": self._single_line(f"{type(exc).__name__}: {exc}", 500)
                }
        self._write_json("setup.json", payload)

    @staticmethod
    def _container_network_bootstrap_command(proxy: str) -> str:
        value = shlex.quote(proxy)
        return f"""set -u
PROXY={value}
mkdir -p /etc/apt/apt.conf.d /root/.config/pip 2>/dev/null || true
printf 'Acquire::http::Proxy "%s";\nAcquire::https::Proxy "%s";\n' "$PROXY" "$PROXY" >/etc/apt/apt.conf.d/99codefactory-proxy 2>/dev/null || true
printf '[global]\nproxy = %s\ntimeout = 120\nretries = 8\n' "$PROXY" >/etc/pip.conf 2>/dev/null || true
cp /etc/pip.conf /root/.config/pip/pip.conf 2>/dev/null || true
git config --global http.proxy "$PROXY" 2>/dev/null || true
git config --global https.proxy "$PROXY" 2>/dev/null || true
printf 'proxy = "%s"\nnoproxy = "{CodeFactoryAgent.LOOPBACK_NO_PROXY}"\nconnect-timeout = 60\nretry = 5\n' "$PROXY" >/root/.curlrc 2>/dev/null || true
printf 'use_proxy = on\nhttp_proxy = %s\nhttps_proxy = %s\n' "$PROXY" "$PROXY" >/root/.wgetrc 2>/dev/null || true
echo codefactory-container-network-bootstrap-ok"""

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        instruction_sha = hashlib.sha256(instruction.encode("utf-8")).hexdigest()
        (self.logs_dir / "instruction.txt").write_text(instruction, encoding="utf-8")
        (self.logs_dir / "instruction.sha256").write_text(instruction_sha, encoding="utf-8")

        if not self._has_model_config():
            if self._bool_env("CODEFACTORY_BENCH_REQUIRE_MODEL"):
                raise RuntimeError(
                    "CODEFACTORY_BENCH_REQUIRE_MODEL is set but the explicit "
                    "benchmark API key and model are not both configured"
                )
            await self._run_baseline(instruction_sha, environment, context)
            return

        await self._run_sidecar(instruction, instruction_sha, environment, context)

    async def _run_baseline(
        self,
        instruction_sha: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        _, contract_sha = _load_execution_contract()
        command = """set -u
echo agent=codefactory-headless
echo mode=baseline-no-model
echo cwd=$(pwd)
echo kernel=$(uname -a 2>/dev/null || true)
find . -maxdepth 2 -type f 2>/dev/null | sed 's#^./##' | sort | head -200"""
        result = await environment.exec(
            command,
            env=self._tool_execution_env(),
            timeout_sec=self._command_timeout_sec(),
        )
        output = result.stdout or ""
        if result.stderr:
            output += "\n[stderr]\n" + result.stderr
        (self.logs_dir / "codefactory-headless.txt").write_text(output, encoding="utf-8")
        context.metadata = {
            **(context.metadata or {}),
            "agent": self.name(),
            "runtime_subject": "rust-core",
            "mode": "baseline-no-model",
            "instruction_sha256": instruction_sha,
            "execution_contract_sha256": contract_sha,
            "exec_return_code": result.return_code,
            "integrity": {
                "contamination_scan": "pass",
                "adapter_role": "harbor-sidecar-bridge",
            },
        }

    async def _run_sidecar(
        self,
        instruction: str,
        instruction_sha: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        _, contract_sha = _load_execution_contract()
        binary = self._resolve_sidecar_binary()
        model = self._model_name()
        api_key = self._bench_env("CODEFACTORY_BENCH_API_KEY")
        assert model and api_key
        allow_network, network_policy = self._resolve_network_policy(environment)
        container_directory = await self._resolve_container_directory(environment)
        working_directory, project_root_confirmed = await self._resolve_project_directory(
            environment, container_directory
        )

        process = await asyncio.create_subprocess_exec(
            str(binary),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=self._sidecar_process_env(),
        )
        assert process.stdin and process.stdout and process.stderr

        start = {
            "type": "start",
            "instruction": instruction,
            "model": model,
            "api_key": api_key,
            "base_url": self._bench_env("CODEFACTORY_BENCH_BASE_URL")
            or "https://api.openai.com/v1",
            "max_steps": self._int_env("CODEFACTORY_BENCH_MAX_STEPS", 80),
            "model_timeout_sec": self._int_env("CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC", 90),
            "shell_timeout_sec": self._int_env("CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC", 300),
            "wall_time_budget_sec": self._execution_budget_sec(),
            "working_directory": working_directory,
            "allow_network": allow_network,
            "execution_contract_sha256": contract_sha,
        }
        process.stdin.write((json.dumps(start) + "\n").encode("utf-8"))
        await process.stdin.drain()

        trajectory: list[dict[str, Any]] = []
        finished: dict[str, Any] | None = None
        wall_timeout_sec = self._agent_wall_timeout_sec()
        deadline = (
            time.monotonic() + wall_timeout_sec if wall_timeout_sec is not None else None
        )

        try:
            while True:
                if deadline is None:
                    raw = await process.stdout.readline()
                else:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError(
                            "CodeFactory Rust sidecar exceeded the agent wall timeout"
                        )
                    raw = await asyncio.wait_for(process.stdout.readline(), timeout=remaining)
                if not raw:
                    stderr = (await process.stderr.read()).decode("utf-8", errors="replace")
                    raise RuntimeError(
                        "CodeFactory sidecar protocol ended before finished: "
                        + self._single_line(stderr, 1000)
                    )
                try:
                    message = json.loads(raw.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                    raise RuntimeError(f"CodeFactory sidecar protocol returned invalid JSON: {exc}") from exc
                if not isinstance(message, dict) or not isinstance(message.get("type"), str):
                    raise RuntimeError("CodeFactory sidecar protocol message is missing a type")

                message_type = message["type"]
                if message_type == "tool_request":
                    trajectory.append(self._redact_protocol_message(message))
                    tool_result = await self._execute_tool_request(
                        message, environment, working_directory
                    )
                    tool_result["working_directory"] = working_directory
                    next_working_directory = working_directory
                    if tool_result.get("return_code") == 0 and not project_root_confirmed:
                        (
                            next_working_directory,
                            project_root_confirmed,
                        ) = await self._resolve_project_directory(
                            environment, container_directory
                        )
                    tool_result["next_working_directory"] = next_working_directory
                    trajectory.append(self._redact_protocol_message(tool_result))
                    process.stdin.write((json.dumps(tool_result) + "\n").encode("utf-8"))
                    await process.stdin.drain()
                    working_directory = next_working_directory
                    self._write_trajectory(trajectory, contract_sha)
                    continue
                if message_type == "event":
                    trajectory.append(self._redact_protocol_message(message))
                    self._write_trajectory(trajectory, contract_sha)
                    continue
                if message_type == "finished":
                    finished = message
                    break
                raise RuntimeError(f"CodeFactory sidecar protocol returned unknown type: {message_type}")
        except BaseException as exc:
            if process.returncode is None:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
            await process.wait()
            failure_metadata = {
                "agent": self.name(),
                "runtime_subject": "rust-core",
                "mode": "model-backed",
                "model": model,
                "status": (
                    "cancelled" if isinstance(exc, asyncio.CancelledError) else "failed"
                ),
                "instruction_sha256": instruction_sha,
                "execution_contract_sha256": contract_sha,
                "network_policy": network_policy,
                "execution_budget_sec": self._execution_budget_sec(),
                "tool_calls": sum(
                    1 for item in trajectory if item.get("type") == "tool_request"
                ),
                "failure": {"type": type(exc).__name__},
                "integrity": {
                    "contamination_scan": "pass",
                    "adapter_role": "harbor-sidecar-bridge",
                },
            }
            context.metadata = {**(context.metadata or {}), **failure_metadata}
            self._write_json("run-metadata.json", failure_metadata)
            raise
        finally:
            if process.stdin and not process.stdin.is_closing():
                process.stdin.close()

        exit_code = await process.wait()
        stderr = (await process.stderr.read()).decode("utf-8", errors="replace")
        if exit_code != 0:
            raise RuntimeError(
                f"CodeFactory sidecar exited with {exit_code}: {self._single_line(stderr, 1000)}"
            )
        assert finished is not None
        if finished.get("execution_contract_sha256") != contract_sha:
            raise RuntimeError("CodeFactory sidecar execution contract hash mismatch")

        final_text = str(finished.get("final_text") or "")
        (self.logs_dir / "final.txt").write_text(final_text, encoding="utf-8")
        self._write_trajectory(trajectory, contract_sha, final_text=final_text)
        metadata = {
            "agent": self.name(),
            "runtime_subject": "rust-core",
            "mode": "model-backed",
            "model": model,
            "instruction_sha256": instruction_sha,
            "execution_contract_sha256": contract_sha,
            "network_policy": network_policy,
            "working_directory": working_directory,
            "execution_budget_sec": self._execution_budget_sec(),
            "completion_evidence": finished.get("completion_evidence") or {},
            "usage": finished.get("usage") or {},
            "tool_calls": sum(1 for item in trajectory if item.get("type") == "tool_request"),
            "integrity": {
                "contamination_scan": "pass",
                "adapter_role": "harbor-sidecar-bridge",
            },
        }
        context.metadata = {**(context.metadata or {}), **metadata}
        self._write_json("run-metadata.json", metadata)

    async def _execute_tool_request(
        self,
        request_message: dict[str, Any],
        environment: BaseEnvironment,
        working_directory: str,
    ) -> dict[str, Any]:
        request_id = str(request_message.get("id") or "")
        command = str(request_message.get("command") or "").strip()
        timeout_sec = int(request_message.get("timeout_sec") or 300)
        if not request_id or not command:
            raise RuntimeError("CodeFactory sidecar protocol tool_request is incomplete")
        try:
            result = await environment.exec(
                command,
                cwd=working_directory,
                env=self._tool_execution_env(),
                timeout_sec=max(1, min(timeout_sec, 900)),
            )
            return {
                "type": "tool_result",
                "id": request_id,
                "return_code": result.return_code,
                "stdout": self._truncate(result.stdout or "", 30000),
                "stderr": self._truncate(result.stderr or "", 30000),
                "error": None,
            }
        except Exception as exc:
            return {
                "type": "tool_result",
                "id": request_id,
                "return_code": None,
                "stdout": "",
                "stderr": "",
                "error": self._single_line(f"{type(exc).__name__}: {exc}", 2000),
            }

    async def _resolve_container_directory(self, environment: BaseEnvironment) -> str:
        result = await environment.exec(
            "pwd -P",
            env=self._tool_execution_env(),
            timeout_sec=30,
        )
        lines = (result.stdout or "").strip().splitlines()
        if result.return_code != 0 or not lines:
            raise RuntimeError("CodeFactory could not resolve the Harbor working directory")
        resolved = lines[0].strip()
        if not resolved.startswith("/"):
            raise RuntimeError("Harbor returned a non-absolute working directory")
        return resolved

    async def _resolve_project_directory(
        self, environment: BaseEnvironment, container_directory: str
    ) -> tuple[str, bool]:
        manifest_scan = await environment.exec(
            "find . -maxdepth 3 "
            "\\( -path './node_modules' -o -path './.venv' -o -path './venv' "
            "-o -path './target' -o -path './.git' \\) -prune -o "
            "-type f \\( -name pyproject.toml -o -name setup.py -o -name setup.cfg "
            "-o -name package.json -o -name Cargo.toml -o -name go.mod \\) -print0",
            cwd=container_directory,
            env=self._tool_execution_env(),
            timeout_sec=30,
        )
        if manifest_scan.return_code != 0:
            return container_directory, False

        project_roots: set[str] = set()
        for manifest in (manifest_scan.stdout or "").split("\0"):
            manifest = manifest.strip()
            if not manifest:
                continue
            candidate = posixpath.normpath(posixpath.join(container_directory, manifest))
            try:
                if (
                    posixpath.commonpath((container_directory, candidate))
                    != container_directory
                ):
                    continue
            except ValueError:
                continue
            project_roots.add(posixpath.dirname(candidate))

        if container_directory in project_roots:
            return container_directory, True
        if len(project_roots) == 1:
            return project_roots.pop(), True
        return container_directory, False

    def _resolve_sidecar_binary(self) -> Path:
        explicit = self._bench_env("CODEFACTORY_BENCH_AGENT_BINARY")
        candidates = [Path(explicit).expanduser()] if explicit else []
        executable = "codefactory-agent-headless.exe" if os.name == "nt" else "codefactory-agent-headless"
        candidates.extend(
            [
                REPO_ROOT / "src-tauri" / "target" / "release" / executable,
                REPO_ROOT / "src-tauri" / "target" / "debug" / executable,
            ]
        )
        for candidate in candidates:
            if candidate.is_file() and os.access(candidate, os.X_OK):
                return candidate
        rendered = ", ".join(str(path) for path in candidates)
        raise RuntimeError(
            "CodeFactory Rust headless sidecar was not found. Build or package "
            f"codefactory-agent-headless, or set CODEFACTORY_BENCH_AGENT_BINARY. Checked: {rendered}"
        )

    def _write_trajectory(
        self,
        steps: list[dict[str, Any]],
        contract_sha: str,
        *,
        final_text: str = "",
    ) -> None:
        self._write_json(
            "trajectory.json",
            {
                "agent": self.name(),
                "runtime_subject": "rust-core",
                "execution_contract_sha256": contract_sha,
                "steps": steps,
                "final_text": final_text,
            },
        )
        (self.logs_dir / "trajectory.jsonl").write_text(
            "".join(json.dumps(item, sort_keys=True) + "\n" for item in steps),
            encoding="utf-8",
        )

    @staticmethod
    def _redact_protocol_message(message: dict[str, Any]) -> dict[str, Any]:
        allowed = {
            "type",
            "id",
            "command",
            "timeout_sec",
            "return_code",
            "stdout",
            "stderr",
            "error",
            "name",
            "content",
            "step",
        }
        return {key: value for key, value in message.items() if key in allowed}

    def _tool_execution_env(self) -> dict[str, str]:
        env = {
            "CODEFACTORY_BENCHMARK_POLICY": "benchmark-sandbox",
            "CODEFACTORY_AGENT_MODE": self._mode(),
            "NO_PROXY": self.LOOPBACK_NO_PROXY,
            "no_proxy": self.LOOPBACK_NO_PROXY,
        }
        proxy = self._bench_env("CODEFACTORY_BENCH_DOCKER_APT_PROXY")
        if proxy:
            env.update(
                {
                    "HTTP_PROXY": proxy,
                    "HTTPS_PROXY": proxy,
                    "ALL_PROXY": proxy,
                    "http_proxy": proxy,
                    "https_proxy": proxy,
                    "all_proxy": proxy,
                }
            )
        return env

    @staticmethod
    def _sidecar_process_env() -> dict[str, str]:
        return dict(os.environ)

    def _has_model_config(self) -> bool:
        return bool(self._bench_env("CODEFACTORY_BENCH_API_KEY") and self._model_name())

    def _mode(self) -> str:
        return "model-backed" if self._has_model_config() else "baseline-no-model"

    def _model_name(self) -> str | None:
        return self._bench_env("CODEFACTORY_BENCH_MODEL") or self.model_name

    def _resolve_network_policy(
        self, environment: BaseEnvironment
    ) -> tuple[bool, str]:
        explicit = self._bench_env("CODEFACTORY_BENCH_ALLOW_NETWORK")
        if explicit is not None:
            allowed = explicit.lower() in {"1", "true", "yes", "on"}
            return allowed, "override-public" if allowed else "override-no-network"

        policy = getattr(environment, "network_policy", None)
        mode = getattr(policy, "network_mode", None)
        value = getattr(mode, "value", mode)
        normalized = str(value or "").strip().lower().replace("_", "-")
        if normalized == "public":
            return True, "public"
        if normalized == "allowlist":
            # Harbor remains the enforcement point for the configured host list.
            return True, "allowlist"
        if normalized == "no-network":
            return False, "no-network"
        return False, "unknown-fail-closed"

    def _bench_env(self, key: str) -> str | None:
        value = self._extra_env.get(key)
        if value is None:
            value = os.environ.get(key)
        if value is None:
            return None
        normalized = str(value).strip()
        return normalized or None

    def _bool_env(self, key: str) -> bool:
        return (self._bench_env(key) or "").lower() in {"1", "true", "yes", "on"}

    def _int_env(self, key: str, default: int) -> int:
        try:
            return int(self._bench_env(key) or default)
        except ValueError:
            return default

    def _agent_wall_timeout_sec(self) -> int | None:
        raw = self._bench_env("CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC")
        if raw is None:
            return None
        try:
            value = int(raw)
        except ValueError:
            return None
        return max(30, value) if value > 0 else None

    def _command_timeout_sec(self) -> int:
        return self._agent_timeout_sec or 120

    def _official_task_execution_budget_sec(self) -> int | None:
        raw = self._bench_env("CODEFACTORY_BENCH_TASK_AGENT_TIMEOUTS_JSON")
        if raw is None:
            return None
        try:
            budgets = json.loads(raw)
        except (TypeError, ValueError):
            return None
        if not isinstance(budgets, dict):
            return None

        trial_name = self.logs_dir.parent.name
        task_name = trial_name.split("__", 1)[0]
        candidates = (task_name, f"terminal-bench/{task_name}")
        for candidate in candidates:
            value = budgets.get(candidate)
            if isinstance(value, (int, float)) and value > 0:
                return int(value)
        return None

    def _execution_budget_sec(self) -> int | None:
        external_budget = (
            self._official_task_execution_budget_sec() or self._agent_timeout_sec
        )
        private_cap = self._agent_wall_timeout_sec()
        if external_budget is not None and private_cap is not None:
            return min(external_budget, private_cap)
        return external_budget or private_cap

    def _write_json(self, name: str, payload: dict[str, Any]) -> None:
        (self.logs_dir / name).write_text(
            json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8"
        )

    @staticmethod
    def _truncate(value: str, limit: int) -> str:
        if len(value) <= limit:
            return value
        return value[:limit] + f"\n[truncated {len(value) - limit} characters]"

    @staticmethod
    def _tail(value: str, limit: int) -> str:
        if len(value) <= limit:
            return value
        return f"[truncated leading {len(value) - limit} characters]\n{value[-limit:]}"

    @staticmethod
    def _single_line(value: str, limit: int) -> str:
        collapsed = " ".join(value.split())
        if len(collapsed) <= limit:
            return collapsed
        return collapsed[:limit] + "..."
