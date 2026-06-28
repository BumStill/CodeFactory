from __future__ import annotations

import hashlib
import os
import json
import re
import shlex
import time
from http import client as http_client
from pathlib import Path
from typing import Any
from urllib import error, request

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class CodeFactoryAgent(BaseAgent):
    """Harbor adapter for CodeFactory Terminal-Bench evaluation loops.

    The model-backed path only uses explicit CODEFACTORY_BENCH_* environment
    variables. It does not read CodeFactory desktop settings, keychain entries,
    generic provider env vars, or user credentials.
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
        return "codefactory-headless"

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
                    "mode": self._mode(),
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

        if self._has_model_config():
            await self._run_model_backed(instruction, instruction_hash, environment, context)
            return

        if self._bool_env("CODEFACTORY_BENCH_REQUIRE_MODEL"):
            raise RuntimeError(
                "CODEFACTORY_BENCH_REQUIRE_MODEL is set but "
                "CODEFACTORY_BENCH_API_KEY and model are not both configured"
            )

        await self._run_baseline(instruction_hash, environment, context)

    async def _run_baseline(
        self,
        instruction_hash: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        diagnostic_command = r"""
set -u
echo "agent=codefactory-headless"
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

    async def _run_model_backed(
        self,
        instruction: str,
        instruction_hash: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        model = self._model_name()
        assert model is not None

        max_steps = self._int_env("CODEFACTORY_BENCH_MAX_STEPS", 20)
        shell_timeout = self._int_env("CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC", 120)
        wall_timeout = self._int_env("CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC", 780)
        deadline = time.monotonic() + max(30, wall_timeout)
        trajectory: list[dict[str, Any]] = []
        artifact_hint = self._artifact_hint_from_instruction(instruction)
        verification_hint = self._verification_hint_from_instruction(instruction)

        messages: list[dict[str, Any]] = [
            {
                "role": "system",
                "content": (
                    "You are CodeFactory headless running inside Terminal-Bench. "
                    "Complete the task by inspecting and editing files through the "
                    "run_shell tool. Do not ask for confirmation. Stay inside the "
                    "task workspace. Do not access Harbor solution, verifier, host, "
                    "credential, or network-exfiltration paths. Run a relevant "
                    "verification command before you finish when feasible. If a "
                    "tool call is denied, choose a safe equivalent command instead "
                    "of repeating the denied command. Before finishing, check that "
                    "the task's expected output artifacts exist. Avoid installing "
                    "packages unless the task cannot be solved with existing tools; "
                    "package installation consumes benchmark time. For artifact "
                    "generation tasks, use at most two inspection rounds before "
                    "creating a candidate artifact; a wrong candidate plus a fast "
                    "self-check is better than extended inspection."
                ),
            },
            {"role": "user", "content": instruction},
        ]
        if artifact_hint:
            messages.append(
                {
                    "role": "user",
                    "content": (
                        "Output artifact hint: the instruction appears to require "
                        f"creating `{artifact_hint}`. Make sure that exact artifact "
                        "exists in the task workspace before you finish."
                    ),
                }
            )
        if verification_hint:
            messages.append({"role": "user", "content": verification_hint})

        final_text = ""
        total_tool_calls = 0
        command_counts: dict[str, int] = {}
        for step in range(max_steps):
            if deadline - time.monotonic() <= 15:
                final_text = final_text or "Stopped before the benchmark agent timeout."
                trajectory.append(
                    {
                        "step": step,
                        "role": "system-reminder",
                        "content": (
                            "Stopping now to leave time for Harbor cleanup and verifier execution."
                        ),
                    }
                )
                self._write_model_backed_logs(trajectory, final_text, model, instruction_hash)
                break

            reminders = [
                self._phase_progress_reminder(step, max_steps, artifact_hint),
                self._remaining_budget_reminder(step, max_steps, artifact_hint),
            ]
            for reminder in [item for item in reminders if item]:
                messages.append({"role": "user", "content": reminder})
                trajectory.append(
                    {
                        "step": step,
                        "role": "system-reminder",
                        "content": reminder,
                    }
                )
                self._write_model_backed_logs(trajectory, final_text, model, instruction_hash)
            try:
                assistant_message = self._chat_completion(
                    self._chat_messages_for_model(messages, trajectory),
                    model,
                    self._bounded_model_timeout(deadline - time.monotonic() - 10),
                )
            except TimeoutError as exc:
                final_text = final_text or "Stopped after the model request timed out."
                trajectory.append(
                    {
                        "step": step,
                        "role": "model-error",
                        "content": f"model request timed out: {exc}",
                    }
                )
                self._write_model_backed_logs(trajectory, final_text, model, instruction_hash)
                break
            tool_calls = assistant_message.get("tool_calls") or []
            content = assistant_message.get("content") or ""
            final_text = content or final_text
            messages.append(
                {
                    "role": "assistant",
                    "content": content,
                    **({"tool_calls": tool_calls} if tool_calls else {}),
                }
            )
            trajectory.append(
                {
                    "step": step,
                    "role": "assistant",
                    "content": content,
                    "tool_calls": self._redact_tool_calls(tool_calls),
                }
            )
            self._write_model_backed_logs(trajectory, final_text, model, instruction_hash)

            if not tool_calls:
                break

            for tool_call in tool_calls:
                remaining_sec = deadline - time.monotonic()
                if remaining_sec <= 15:
                    final_text = final_text or "Stopped before the benchmark agent timeout."
                    trajectory.append(
                        {
                            "step": step,
                            "role": "system-reminder",
                            "content": (
                                "Skipping remaining tool calls to leave time for Harbor cleanup "
                                "and verifier execution."
                            ),
                        }
                    )
                    self._write_model_backed_logs(
                        trajectory, final_text, model, instruction_hash
                    )
                    break

                total_tool_calls += 1
                tool_result = await self._handle_tool_call(
                    tool_call,
                    environment,
                    min(shell_timeout, max(5, int(remaining_sec) - 5)),
                    command_counts,
                )
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": tool_call.get("id", f"call_{total_tool_calls}"),
                        "content": tool_result["content"],
                    }
                )
                trajectory.append(tool_result["trajectory"])
                repair_hint = self._repair_hint_from_tool_result(
                    tool_result["trajectory"], artifact_hint
                )
                if repair_hint:
                    messages.append({"role": "user", "content": repair_hint})
                    trajectory.append(
                        {
                            "step": step,
                            "role": "system-reminder",
                            "content": repair_hint,
                        }
                    )
                self._write_model_backed_logs(trajectory, final_text, model, instruction_hash)

        self._write_model_backed_logs(trajectory, final_text, model, instruction_hash)

        context.metadata = {
            **(context.metadata or {}),
            "agent": self.name(),
            "mode": "model-backed",
            "model": model,
            "instruction_sha256": instruction_hash,
            "tool_calls": total_tool_calls,
            "max_steps": max_steps,
        }

    def _write_model_backed_logs(
        self,
        trajectory: list[dict[str, Any]],
        final_text: str,
        model: str,
        instruction_hash: str,
    ) -> None:
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        (self.logs_dir / "final.txt").write_text(final_text)
        (self.logs_dir / "trajectory.json").write_text(
            json.dumps(
                {
                    "agent": self.name(),
                    "mode": "model-backed",
                    "model": model,
                    "instruction_sha256": instruction_hash,
                    "steps": trajectory,
                },
                indent=2,
                sort_keys=True,
            )
        )
        (self.logs_dir / "trajectory.jsonl").write_text(
            "\n".join(json.dumps(item, sort_keys=True) for item in trajectory) + "\n"
        )

    def _chat_messages_for_model(
        self,
        messages: list[dict[str, Any]],
        trajectory: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        compacted: list[dict[str, Any]] = []
        kept_user_messages = 0
        for message in messages:
            role = message.get("role")
            content = str(message.get("content") or "")
            if role == "system":
                compacted.append({"role": "system", "content": content})
                continue
            if role != "user":
                continue
            if kept_user_messages < 2 or content.startswith(
                ("Inspection phase", "Repair focus:", "Verification hint:", "You have ")
            ):
                compacted.append({"role": "user", "content": content})
                kept_user_messages += 1

        summary = self._trajectory_summary_for_model(trajectory)
        if summary:
            compacted.append({"role": "user", "content": summary})
        return compacted

    def _trajectory_summary_for_model(self, trajectory: list[dict[str, Any]]) -> str | None:
        if not trajectory:
            return None

        lines = [
            "Execution summary from previous tool calls. Continue from the latest "
            "failure; inspect files again if needed instead of relying on omitted "
            "large command bodies."
        ]
        for item in trajectory[-8:]:
            role = item.get("role")
            if role == "tool":
                command = self._single_line(
                    self._strip_heredoc_bodies(str(item.get("command") or ""))
                )
                content = self._tail(str(item.get("content") or ""), 1200)
                lines.append(
                    f"- tool {item.get('status', 'unknown')}: `{command}`\n{content}"
                )
            elif role == "system-reminder":
                lines.append(f"- reminder: {self._single_line(str(item.get('content') or ''))}")
            elif role == "model-error":
                lines.append(f"- model-error: {self._single_line(str(item.get('content') or ''))}")

        return self._truncate(
            "\n".join(lines),
            self._int_env("CODEFACTORY_BENCH_CONTEXT_SUMMARY_LIMIT", 12000),
        )

    async def _handle_tool_call(
        self,
        tool_call: dict[str, Any],
        environment: BaseEnvironment,
        timeout_sec: int,
        command_counts: dict[str, int],
    ) -> dict[str, Any]:
        call_id = tool_call.get("id", "unknown")
        function = tool_call.get("function") or {}
        name = function.get("name")
        raw_arguments = function.get("arguments") or "{}"
        try:
            arguments = json.loads(raw_arguments)
        except json.JSONDecodeError as exc:
            content = f"Tool arguments were not valid JSON: {exc}"
            return {
                "content": content,
                "trajectory": {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "tool": name,
                    "status": "error",
                    "content": content,
                },
            }

        if name != "run_shell":
            content = f"Unknown tool: {name}"
            return {
                "content": content,
                "trajectory": {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "tool": name,
                    "status": "error",
                    "content": content,
                },
            }

        command = str(arguments.get("command") or "").strip()
        decision = self._classify_shell_command(command)
        if decision["action"] == "deny":
            content = f"DENIED by benchmark-sandbox: {decision['reason']}"
            return {
                "content": content,
                "trajectory": {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "tool": name,
                    "command": command,
                    "policy": decision,
                    "status": "denied",
                    "content": content,
                },
            }

        command_key = self._repeat_command_key(command)
        command_counts[command_key] = command_counts.get(command_key, 0) + 1
        repeat_limit = self._int_env("CODEFACTORY_BENCH_REPEAT_COMMAND_LIMIT", 2)
        if (
            command_counts[command_key] > repeat_limit
            and self._is_repeat_suppression_candidate(command)
        ):
            content = (
                "REPEATED COMMAND SUPPRESSED: this inspection command already ran "
                f"{command_counts[command_key] - 1} time(s). Use the existing output "
                "from the execution summary, move to implementation or verification, "
                "or run a materially different command."
            )
            return {
                "content": content,
                "trajectory": {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "tool": name,
                    "command": command,
                    "policy": {
                        "action": "suppress",
                        "reason": "repeated low-value inspection command",
                    },
                    "status": "suppressed",
                    "content": content,
                },
            }

        result = await environment.exec(
            command,
            env={
                "CODEFACTORY_BENCHMARK_POLICY": "benchmark-sandbox",
                "CODEFACTORY_AGENT_MODE": "model-backed",
            },
            timeout_sec=timeout_sec,
        )
        content = self._format_exec_result(
            result.return_code,
            result.stdout,
            result.stderr,
            self._int_env("CODEFACTORY_BENCH_TOOL_OUTPUT_LIMIT", 20000),
        )
        return {
            "content": content,
            "trajectory": {
                "role": "tool",
                "tool_call_id": call_id,
                "tool": name,
                "command": command,
                "policy": decision,
                "status": "ok" if result.return_code == 0 else "nonzero",
                "return_code": result.return_code,
                "stdout_bytes": len(result.stdout or ""),
                "stderr_bytes": len(result.stderr or ""),
                "content": content,
            },
        }

    def _chat_completion(
        self,
        messages: list[dict[str, Any]],
        model: str,
        timeout_sec: int | None = None,
    ) -> dict[str, Any]:
        api_key = self._bench_env("CODEFACTORY_BENCH_API_KEY")
        if not api_key:
            raise RuntimeError("CODEFACTORY_BENCH_API_KEY is required for model-backed mode")
        base_url = (
            self._bench_env("CODEFACTORY_BENCH_BASE_URL") or "https://api.openai.com/v1"
        ).rstrip("/")
        timeout_sec = timeout_sec or self._int_env("CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC", 60)
        payload = {
            "model": model,
            "messages": messages,
            "temperature": 0,
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "run_shell",
                        "description": "Run a shell command inside the Terminal-Bench task container.",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "command": {
                                    "type": "string",
                                    "description": "Shell command to run in the task workspace.",
                                }
                            },
                            "required": ["command"],
                            "additionalProperties": False,
                        },
                    },
                }
            ],
            "tool_choice": "auto",
        }
        req = request.Request(
            f"{base_url}/chat/completions",
            data=json.dumps(payload).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with request.urlopen(req, timeout=timeout_sec) as response:
                data = json.loads(response.read().decode("utf-8"))
        except error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"model request failed: HTTP {exc.code}: {body[:1000]}") from exc
        except (TimeoutError, http_client.IncompleteRead, http_client.RemoteDisconnected) as exc:
            raise TimeoutError(f"model request did not complete: {exc}") from exc

        choices = data.get("choices") or []
        if not choices:
            raise RuntimeError("model response did not include choices")
        message = choices[0].get("message") or {}
        if not isinstance(message, dict):
            raise RuntimeError("model response message was not an object")
        return message

    def _bounded_model_timeout(self, remaining_sec: float) -> int:
        configured = self._int_env("CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC", 60)
        if remaining_sec <= 0:
            return 1
        return max(1, min(configured, int(remaining_sec)))

    def _classify_shell_command(self, command: str) -> dict[str, str]:
        if not command:
            return {"action": "deny", "reason": "empty command"}

        normalized = re.sub(r"\s+", " ", command.strip().lower())
        permanent_denies = [
            ("rm -rf /", "destructive root delete"),
            ("mkfs", "filesystem formatting"),
            ("shutdown", "system shutdown"),
            ("reboot", "system reboot"),
            ("/var/run/docker.sock", "docker host socket access"),
            ("/solution", "Harbor solution path access"),
            ("/tests", "Harbor verifier test path access"),
            ("/logs/verifier", "Harbor verifier log access"),
            ("/root/.ssh", "credential path access"),
            ("~/.ssh", "credential path access"),
            (".ssh/", "credential path access"),
            ("id_rsa", "private key access"),
            ("id_ed25519", "private key access"),
            ("codefactory_bench_api_key", "benchmark API key access"),
            ("openai_api_key", "provider secret access"),
            ("anthropic_api_key", "provider secret access"),
            ("github_token", "provider secret access"),
        ]
        for needle, reason in permanent_denies:
            if needle in normalized:
                return {"action": "deny", "reason": reason}

        if not self._bool_env("CODEFACTORY_BENCH_ALLOW_NETWORK"):
            if self._contains_network_tool_invocation(command):
                return {"action": "deny", "reason": "network/exfiltration tool disabled"}

        return {"action": "allow", "reason": "benchmark task container command"}

    @staticmethod
    def _artifact_hint_from_instruction(instruction: str) -> str | None:
        patterns = [
            r"\b(?:write|create|generate|produce|save)\s+(?:me\s+)?(?:a\s+|an\s+|the\s+)?"
            r"(?:file\s+)?(?:at\s+|to\s+|as\s+|named\s+)?"
            r"(`?/?[A-Za-z0-9_.\-/]+\.[A-Za-z0-9_.-]+`?)",
            r"\b(`/[A-Za-z0-9_.\-/]+\.[A-Za-z0-9_.-]+`?)",
        ]
        for pattern in patterns:
            match = re.search(pattern, instruction, re.IGNORECASE)
            if match:
                return match.group(1).strip("`.,;:")
        return None

    @staticmethod
    def _verification_hint_from_instruction(instruction: str) -> str | None:
        match = re.search(
            r"\brunning\s+(.+?)\s+gives exactly\s+(`?/?[A-Za-z0-9_.\-/]+`?)",
            instruction,
            re.IGNORECASE | re.DOTALL,
        )
        if not match:
            return None

        command = " ".join(match.group(1).strip("` \n\t").split())
        expected = match.group(2).strip("`.,;:")
        if not command or not expected:
            return None

        return (
            "Verification hint: the task gives an exact stdout comparison. After "
            "creating a candidate artifact, run "
            f"`{command} > /tmp/codefactory-bench-output && "
            f"cmp -s /tmp/codefactory-bench-output {expected} && echo verification-ok`. "
            "If it fails or crashes, repair the artifact or generator before finishing."
        )

    @staticmethod
    def _phase_progress_reminder(
        step: int, max_steps: int, artifact_hint: str | None
    ) -> str | None:
        checkpoints = {2, 5, max_steps // 2}
        if step not in checkpoints:
            return None

        artifact = f" `{artifact_hint}`" if artifact_hint else " the expected artifact"
        return (
            "Inspection phase should be over. Stop re-reading files you already saw. "
            f"Create or repair{artifact} now, then run the exact or closest self-check. "
            "Use later tool calls for implementation and verification, not broad exploration."
        )

    @staticmethod
    def _remaining_budget_reminder(
        step: int, max_steps: int, artifact_hint: str | None
    ) -> str | None:
        remaining = max_steps - step
        if remaining not in {3, 1}:
            return None

        artifact = (
            f" The expected artifact appears to be `{artifact_hint}`;"
            if artifact_hint
            else " If the task names an expected output artifact,"
        )
        urgency = "last tool-call round" if remaining == 1 else f"only {remaining} tool-call rounds"
        return (
            f"You have {urgency} remaining before the benchmark agent stops."
            f"{artifact} create it now, run the quickest feasible verification, "
            "and then return a concise final answer. Do not spend remaining calls "
            "on broad exploration."
        )

    @staticmethod
    def _repair_hint_from_tool_result(
        tool_result: dict[str, Any], artifact_hint: str | None
    ) -> str | None:
        content = str(tool_result.get("content") or "")
        normalized = content.lower()
        artifact = f" `{artifact_hint}`" if artifact_hint else " the expected artifact"

        if "segmentation fault" in normalized or "core dumped" in normalized:
            return (
                "Repair focus: the latest self-check crashed with a segmentation fault. "
                f"Do not finish yet. Fix the generated artifact/protocol for{artifact}, "
                "then run the smallest round-trip check that proves the task output is "
                "accepted before giving a final answer."
            )

        if artifact_hint and artifact_hint.lower() in normalized and (
            "no such file" in normalized or "not found" in normalized
        ):
            return (
                f"Repair focus: `{artifact_hint}` is still missing. Create that exact "
                "artifact path now, then run the quickest feasible existence and content "
                "check before finishing."
            )

        if "command not found" in normalized:
            return (
                "Repair focus: the latest command used a missing tool. Do not retry "
                "that tool; switch to available shell, coreutils, compiler, or a small "
                "helper program and continue from the known failure."
            )

        if "return_code=124" in normalized or "timed out" in normalized:
            return (
                "Repair focus: the latest command timed out. Replace broad exploration "
                "or long-running checks with a smaller deterministic check and continue "
                "from the known failure."
            )

        return None

    @staticmethod
    def _repeat_command_key(command: str) -> str:
        inspection_key = CodeFactoryAgent._inspection_target_key(command)
        if inspection_key:
            return inspection_key
        return " ".join(CodeFactoryAgent._strip_heredoc_bodies(command).split()).lower()

    @staticmethod
    def _inspection_target_key(command: str) -> str | None:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip()
        if not command_text:
            return None
        if any(token in command_text for token in [">", ">>", "&&", ";", "$("]):
            return None

        pipeline = [part.strip() for part in command_text.split("|")]
        first_key = CodeFactoryAgent._simple_inspection_target_key(pipeline[0])
        if not first_key:
            return None
        for filter_command in pipeline[1:]:
            try:
                filter_parts = shlex.split(filter_command)
            except ValueError:
                return None
            if not filter_parts:
                return None
            if filter_parts[0].split("/")[-1] not in {"grep", "head", "od", "sed", "tail", "wc"}:
                return None
        return first_key

    @staticmethod
    def _simple_inspection_target_key(command_text: str) -> str | None:
        try:
            parts = shlex.split(command_text)
        except ValueError:
            return None
        if not parts:
            return None

        command_name = parts[0].split("/")[-1]
        if command_name not in {"cat", "head", "od", "sed", "tail", "wc"}:
            return None

        for token in reversed(parts[1:]):
            if token.startswith("-"):
                continue
            return f"{command_name if command_name == 'wc' else 'read'}:{token}"
        return None

    @staticmethod
    def _is_repeat_suppression_candidate(command: str) -> bool:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip()
        if not command_text:
            return False
        if CodeFactoryAgent._inspection_target_key(command):
            return True
        if any(token in command_text for token in [">", ">>", "|", "&&", ";", "$("]):
            return False
        first = command_text.split(None, 1)[0].split("/")[-1]
        return first in {
            "cat",
            "find",
            "grep",
            "head",
            "ls",
            "od",
            "sed",
            "tail",
            "wc",
            "which",
        }

    @staticmethod
    def _contains_network_tool_invocation(command: str) -> bool:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).lower()
        network_tool_pattern = (
            r"(?:^|[;&|()]\s*|\b(?:then|do|else)\s+)"
            r"(?:sudo\s+|command\s+|env\s+)*"
            r"(curl|wget|nc|ncat|netcat|socat|ssh|scp|sftp|ftp|rsync)"
            r"(?:\s|$)"
        )
        return re.search(network_tool_pattern, command_text, re.MULTILINE) is not None

    @staticmethod
    def _strip_heredoc_bodies(command: str) -> str:
        lines = command.splitlines()
        stripped: list[str] = []
        delimiter: str | None = None
        heredoc_pattern = re.compile(r"<<-?\s*['\"]?([A-Za-z0-9_./-]+)['\"]?")

        for line in lines:
            if delimiter is not None:
                if line.strip() == delimiter:
                    delimiter = None
                continue

            stripped.append(line)
            match = heredoc_pattern.search(line)
            if match:
                delimiter = match.group(1)

        return "\n".join(stripped)

    def _has_model_config(self) -> bool:
        return bool(self._bench_env("CODEFACTORY_BENCH_API_KEY") and self._model_name())

    def _mode(self) -> str:
        return "model-backed" if self._has_model_config() else "baseline-no-model"

    def _model_name(self) -> str | None:
        return self._bench_env("CODEFACTORY_BENCH_MODEL") or self.model_name

    def _bench_env(self, key: str) -> str | None:
        value = self._extra_env.get(key) or os.environ.get(key)
        if value is None:
            return None
        value = str(value).strip()
        return value or None

    def _bool_env(self, key: str) -> bool:
        value = (self._bench_env(key) or "").lower()
        return value in {"1", "true", "yes", "on"}

    def _int_env(self, key: str, default: int) -> int:
        value = self._bench_env(key)
        if value is None:
            return default
        try:
            return int(value)
        except ValueError:
            return default

    @staticmethod
    def _format_exec_result(
        return_code: int,
        stdout: str | None,
        stderr: str | None,
        output_limit: int = 20000,
    ) -> str:
        return (
            f"return_code={return_code}\n"
            f"stdout:\n{CodeFactoryAgent._truncate(stdout or '', output_limit)}\n"
            f"stderr:\n{CodeFactoryAgent._truncate(stderr or '', output_limit)}"
        )

    @staticmethod
    def _truncate(value: str, limit: int = 6000) -> str:
        if len(value) <= limit:
            return value
        return value[:limit] + f"\n[truncated {len(value) - limit} bytes]"

    @staticmethod
    def _tail(value: str, limit: int) -> str:
        if len(value) <= limit:
            return value
        return f"[truncated leading {len(value) - limit} bytes]\n{value[-limit:]}"

    @staticmethod
    def _single_line(value: str, limit: int = 240) -> str:
        collapsed = " ".join(value.split())
        if len(collapsed) <= limit:
            return collapsed
        return collapsed[:limit] + f"... [truncated {len(collapsed) - limit} chars]"

    @staticmethod
    def _redact_tool_calls(tool_calls: list[dict[str, Any]]) -> list[dict[str, Any]]:
        redacted = []
        for call in tool_calls:
            function = call.get("function") or {}
            redacted.append(
                {
                    "id": call.get("id"),
                    "type": call.get("type"),
                    "function": {
                        "name": function.get("name"),
                        "arguments": function.get("arguments"),
                    },
                }
            )
        return redacted
