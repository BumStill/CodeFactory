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
        model_timeout_retries = self._int_env("CODEFACTORY_BENCH_MODEL_TIMEOUT_RETRIES", 1)
        no_action_retries = self._int_env("CODEFACTORY_BENCH_NO_ACTION_RETRIES", 4)
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
        total_usage = {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        no_action_recoveries = 0
        final_verify_recoveries = 0
        command_counts: dict[str, int] = {}
        loop_state: dict[str, Any] = {
            "implementation_started": False,
            "artifact_started": False,
            "verification_started": False,
            "implementation_required_count": 0,
            "exec_errors": 0,
            "command_timeouts": 0,
            "service_supervision_blocks": 0,
            "long_command_blocks": 0,
            "implementation_required_blocks": 0,
            "preflight_blocks": 0,
            "background_processes": [],
            "repair_goals": [],
            "missing_commands": {},
        }

        def write_logs() -> None:
            self._write_model_backed_logs(
                trajectory,
                final_text,
                model,
                instruction_hash,
                total_usage,
            )

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
                write_logs()
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
                write_logs()
            assistant_message: dict[str, Any] | None = None
            for request_attempt in range(model_timeout_retries + 1):
                try:
                    assistant_message = self._chat_completion(
                        self._chat_messages_for_model(messages, trajectory),
                        model,
                        self._bounded_model_timeout(deadline - time.monotonic() - 10),
                        force_tool=bool(artifact_hint)
                        and not self._candidate_artifact_started(
                            loop_state, artifact_hint
                        ),
                    )
                    break
                except TimeoutError as exc:
                    trajectory.append(
                        {
                            "step": step,
                            "role": "model-error",
                            "content": f"model request timed out: {exc}",
                        }
                    )
                    if (
                        request_attempt < model_timeout_retries
                        and deadline - time.monotonic() > 45
                    ):
                        retry_prompt = self._timeout_recovery_prompt(
                            loop_state, artifact_hint
                        )
                        messages.append({"role": "user", "content": retry_prompt})
                        trajectory.append(
                            {
                                "step": step,
                                "role": "system-reminder",
                                "content": retry_prompt,
                            }
                        )
                        write_logs()
                        continue

                    final_text = final_text or "Stopped after the model request timed out."
                    write_logs()
                    break
            if assistant_message is None:
                break
            tool_calls = assistant_message.get("tool_calls") or []
            content = assistant_message.get("content") or ""
            usage = assistant_message.get("_codefactory_usage") or {}
            if usage:
                for key in total_usage:
                    total_usage[key] += int(usage.get(key) or 0)
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
                    **({"usage": usage} if usage else {}),
                }
            )
            write_logs()

            if not tool_calls:
                if self._requires_final_verification_recovery(
                    loop_state,
                    artifact_hint,
                    final_verify_recoveries,
                    self._int_env("CODEFACTORY_BENCH_FINAL_VERIFY_RETRIES", 1),
                ):
                    final_verify_recoveries += 1
                    recovery_prompt = self._final_verification_recovery_prompt(
                        loop_state,
                        artifact_hint,
                    )
                    messages.append({"role": "user", "content": recovery_prompt})
                    trajectory.append(
                        {
                            "step": step,
                            "role": "system-reminder",
                            "content": recovery_prompt,
                        }
                    )
                    write_logs()
                    continue
                if self._requires_no_action_recovery(
                    loop_state, artifact_hint, no_action_recoveries, no_action_retries
                ):
                    no_action_recoveries += 1
                    recovery_prompt = self._no_action_recovery_prompt(
                        loop_state, artifact_hint
                    )
                    messages.append({"role": "user", "content": recovery_prompt})
                    trajectory.append(
                        {
                            "step": step,
                            "role": "system-reminder",
                            "content": recovery_prompt,
                        }
                    )
                    write_logs()
                    continue
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
                    write_logs()
                    break

                total_tool_calls += 1
                tool_result = await self._handle_tool_call(
                    tool_call,
                    environment,
                    min(shell_timeout, max(5, int(remaining_sec) - 5)),
                    command_counts,
                    loop_state,
                    step,
                    artifact_hint,
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
                repair_goal = self._repair_goal_from_tool_result(
                    tool_result["trajectory"], artifact_hint
                )
                if repair_goal:
                    loop_state["repair_goals"].append(repair_goal)
                    repair_goal_message = self._format_repair_goal(repair_goal)
                    messages.append({"role": "user", "content": repair_goal_message})
                    trajectory.append(
                        {
                            "step": step,
                            "role": "repair-goal",
                            "content": repair_goal_message,
                            "goal": repair_goal,
                        }
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
                auto_repair_command = self._auto_repair_command_from_tool_result(
                    tool_result["trajectory"], artifact_hint, loop_state
                )
                if auto_repair_command and deadline - time.monotonic() > 20:
                    loop_state["auto_protocol_repairs"] = (
                        int(loop_state.get("auto_protocol_repairs") or 0) + 1
                    )
                    auto_timeout_sec = min(
                        shell_timeout,
                        max(10, int(deadline - time.monotonic()) - 5),
                    )
                    loop_state["artifact_started"] = True
                    try:
                        auto_result = await environment.exec(
                            auto_repair_command,
                            env={
                                "CODEFACTORY_BENCHMARK_POLICY": "benchmark-sandbox",
                                "CODEFACTORY_AGENT_MODE": "model-backed",
                                "CODEFACTORY_AGENT_AUTO_REPAIR": "1",
                            },
                            timeout_sec=auto_timeout_sec,
                        )
                        auto_content = self._format_exec_result(
                            auto_result.return_code,
                            auto_result.stdout,
                            auto_result.stderr,
                            self._int_env("CODEFACTORY_BENCH_TOOL_OUTPUT_LIMIT", 20000),
                        )
                        auto_trajectory = {
                            "role": "tool",
                            "tool_call_id": (
                                f"auto_repair_{loop_state['auto_protocol_repairs']}"
                            ),
                            "tool": "run_shell",
                            "command": auto_repair_command,
                            "policy": {
                                "action": "allow",
                                "reason": "benchmark auto repair command",
                            },
                            "status": (
                                "auto-repair-ok"
                                if auto_result.return_code == 0
                                else "auto-repair-nonzero"
                            ),
                            "return_code": auto_result.return_code,
                            "stdout_bytes": len(auto_result.stdout or ""),
                            "stderr_bytes": len(auto_result.stderr or ""),
                            "content": auto_content,
                        }
                    except Exception as exc:
                        error_type = self._exec_exception_type(exc)
                        loop_state["exec_errors"] = int(loop_state.get("exec_errors") or 0) + 1
                        if error_type == "command-timeout":
                            loop_state["command_timeouts"] = (
                                int(loop_state.get("command_timeouts") or 0) + 1
                            )
                        auto_trajectory = {
                            "role": "tool",
                            "tool_call_id": (
                                f"auto_repair_{loop_state['auto_protocol_repairs']}"
                            ),
                            "tool": "run_shell",
                            "command": auto_repair_command,
                            "policy": {
                                "action": "allow",
                                "reason": "benchmark auto repair command",
                            },
                            "status": "exec-error",
                            "error_type": error_type,
                            "timeout_sec": auto_timeout_sec,
                            "content": self._format_exec_exception(
                                exc, auto_repair_command, auto_timeout_sec
                            ),
                        }
                    trajectory.append(
                        auto_trajectory
                    )
                    auto_summary = (
                        "Repair focus: an automatic protocol repair command was run "
                        f"for `{artifact_hint}`. Use its output for final verification; "
                        "do not replace the artifact unless the self-check still fails."
                    )
                    loop_state["verification_started"] = True
                    messages.append({"role": "user", "content": auto_summary})
                    trajectory.append(
                        {
                            "step": step,
                            "role": "system-reminder",
                            "content": auto_summary,
                        }
                    )
                write_logs()

        write_logs()

        context.metadata = {
            **(context.metadata or {}),
            "agent": self.name(),
            "mode": "model-backed",
            "model": model,
            "instruction_sha256": instruction_hash,
            "tool_calls": total_tool_calls,
            "max_steps": max_steps,
            "usage": total_usage,
            "exec_errors": int(loop_state.get("exec_errors") or 0),
            "command_timeouts": int(loop_state.get("command_timeouts") or 0),
            "service_supervision_blocks": int(
                loop_state.get("service_supervision_blocks") or 0
            ),
            "long_command_blocks": int(loop_state.get("long_command_blocks") or 0),
            "implementation_required_blocks": int(
                loop_state.get("implementation_required_blocks") or 0
            ),
            "preflight_blocks": int(loop_state.get("preflight_blocks") or 0),
            "repair_goal_count": len(loop_state.get("repair_goals") or []),
            "background_process_count": len(loop_state.get("background_processes") or []),
        }

    def _write_model_backed_logs(
        self,
        trajectory: list[dict[str, Any]],
        final_text: str,
        model: str,
        instruction_hash: str,
        usage: dict[str, int] | None = None,
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
                    "usage": usage or {},
                    "steps": trajectory,
                },
                indent=2,
                sort_keys=True,
            )
        )
        (self.logs_dir / "trajectory.jsonl").write_text(
            "\n".join(json.dumps(item, sort_keys=True) for item in trajectory) + "\n"
        )
        (self.logs_dir / "usage.json").write_text(
            json.dumps(usage or {}, indent=2, sort_keys=True)
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
                (
                    "Inspection phase",
                    "No-action recovery:",
                    "Final-before-verify gate:",
                    "Repair goal:",
                    "Repair focus:",
                    "Timeout recovery:",
                    "Verification hint:",
                    "You have ",
                )
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
            "failure. Do not repeat commands that were suppressed; when artifact "
            "generation is required, produce the artifact before further inspection."
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
        loop_state: dict[str, Any],
        step: int,
        artifact_hint: str | None,
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
        preflight = self._preflight_shell_command(command, loop_state)
        if preflight:
            loop_state["preflight_blocks"] = int(loop_state.get("preflight_blocks") or 0) + 1
            return {
                "content": preflight["content"],
                "trajectory": {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "tool": name,
                    "command": command,
                    "policy": {
                        "action": "suppress",
                        "reason": preflight["reason"],
                    },
                    "status": "preflight-blocked",
                    "content": preflight["content"],
                },
            }

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

        if self._requires_artifact_command(command, loop_state, step, artifact_hint):
            loop_state["implementation_required_count"] = (
                int(loop_state.get("implementation_required_count") or 0) + 1
            )
            loop_state["implementation_required_blocks"] = (
                int(loop_state.get("implementation_required_blocks") or 0) + 1
            )
            artifact = f" `{artifact_hint}`" if artifact_hint else " the expected artifact"
            content = (
                "ARTIFACT COMMAND REQUIRED: implementation budget is exhausted and "
                f"{artifact} still has not been produced. The next accepted command must "
                f"directly create, write, copy, move, or run a generator that produces{artifact}."
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
                        "reason": "artifact command required",
                    },
                    "status": "artifact-required",
                    "content": content,
                },
            }

        if self._requires_implementation_before_inspection(
            command, loop_state, step, artifact_hint
        ):
            loop_state["implementation_required_blocks"] = (
                int(loop_state.get("implementation_required_blocks") or 0) + 1
            )
            loop_state["implementation_required_count"] = (
                int(loop_state.get("implementation_required_count") or 0) + 1
            )
            artifact = f" `{artifact_hint}`" if artifact_hint else " the expected artifact"
            content = (
                "IMPLEMENTATION REQUIRED: the inspection budget is exhausted and no "
                "candidate artifact has been produced yet. Do not inspect more files now. "
                f"Create or compile a generator, write{artifact}, or run a command "
                "that produces a candidate artifact before requesting more inspection."
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
                        "reason": "implementation required before more inspection",
                    },
                    "status": "implementation-required",
                    "content": content,
                },
            }

        if self._requires_service_supervision(command):
            loop_state["service_supervision_blocks"] = (
                int(loop_state.get("service_supervision_blocks") or 0) + 1
            )
            content = (
                "SERVICE SUPERVISION REQUIRED: this looks like a foreground service "
                "command that may run until the tool timeout. Start it in the "
                "background, redirect stdout/stderr to a log file, record its pid, "
                "run a bounded readiness check, then run the client/test command. "
                "Do not retry the foreground service command unchanged."
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
                        "reason": "foreground service requires supervision",
                    },
                    "status": "service-supervision-required",
                    "content": content,
                },
            }

        long_command_reason = self._long_command_policy_reason(command)
        if long_command_reason:
            loop_state["long_command_blocks"] = (
                int(loop_state.get("long_command_blocks") or 0) + 1
            )
            content = (
                "LONG COMMAND POLICY REQUIRED: this command looks unbounded or "
                f"likely to consume the benchmark timeout ({long_command_reason}). "
                "Run a bounded version with `timeout`, a smaller sample, or a "
                "readiness/health check that exits deterministically. Do not retry "
                "the unbounded command unchanged."
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
                        "reason": "long command requires bounded plan",
                    },
                    "status": "long-command-policy-required",
                    "content": content,
                },
            }

        command_key = self._repeat_command_key(command)
        command_counts[command_key] = command_counts.get(command_key, 0) + 1
        repeat_limit = self._int_env("CODEFACTORY_BENCH_REPEAT_COMMAND_LIMIT", 1)
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

        if self._is_implementation_attempt(command):
            loop_state["implementation_started"] = True
        if self._is_artifact_attempt(command, artifact_hint):
            loop_state["artifact_started"] = True
            loop_state["implementation_required_count"] = 0
        if self._is_verification_attempt(command, artifact_hint):
            loop_state["verification_started"] = True
        background_process = self._background_process_lifecycle(command)
        if background_process:
            loop_state["background_processes"].append(background_process)
        try:
            result = await environment.exec(
                command,
                env={
                    "CODEFACTORY_BENCHMARK_POLICY": "benchmark-sandbox",
                    "CODEFACTORY_AGENT_MODE": "model-backed",
                },
                timeout_sec=timeout_sec,
            )
        except Exception as exc:
            error_type = self._exec_exception_type(exc)
            loop_state["exec_errors"] = int(loop_state.get("exec_errors") or 0) + 1
            if error_type == "command-timeout":
                loop_state["command_timeouts"] = (
                    int(loop_state.get("command_timeouts") or 0) + 1
                )
            content = self._format_exec_exception(exc, command, timeout_sec)
            return {
                "content": content,
                "trajectory": {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "tool": name,
                    "command": command,
                    "policy": decision,
                    "status": "exec-error",
                    "error_type": error_type,
                    "timeout_sec": timeout_sec,
                    "content": content,
                },
            }
        content = self._format_exec_result(
            result.return_code,
            result.stdout,
            result.stderr,
            self._int_env("CODEFACTORY_BENCH_TOOL_OUTPUT_LIMIT", 20000),
        )
        self._record_tool_outcome(command, result.return_code, result.stdout, result.stderr, loop_state)
        semantic_failure = self._semantic_failure_reason(content)
        return {
            "content": content,
            "trajectory": {
                "role": "tool",
                "tool_call_id": call_id,
                "tool": name,
                "command": command,
                "policy": decision,
                "status": (
                    "semantic-failure"
                    if semantic_failure
                    else "ok" if result.return_code == 0 else "nonzero"
                ),
                "return_code": result.return_code,
                **({"semantic_failure": semantic_failure} if semantic_failure else {}),
                "stdout_bytes": len(result.stdout or ""),
                "stderr_bytes": len(result.stderr or ""),
                "content": content,
                **(
                    {"background_process": background_process}
                    if background_process
                    else {}
                ),
            },
        }

    def _chat_completion(
        self,
        messages: list[dict[str, Any]],
        model: str,
        timeout_sec: int | None = None,
        force_tool: bool = False,
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
            "max_tokens": self._int_env("CODEFACTORY_BENCH_MAX_OUTPUT_TOKENS", 4096),
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
            "tool_choice": (
                {"type": "function", "function": {"name": "run_shell"}}
                if force_tool
                else "auto"
            ),
        }
        def send_request() -> dict[str, Any]:
            req = request.Request(
                f"{base_url}/chat/completions",
                data=json.dumps(payload).encode("utf-8"),
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                },
                method="POST",
            )
            with request.urlopen(req, timeout=timeout_sec) as response:
                return json.loads(response.read().decode("utf-8"))

        try:
            data = send_request()
        except error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            if (
                force_tool
                and exc.code == 400
                and "tool_choice" in body.lower()
            ):
                payload["tool_choice"] = "auto"
                try:
                    data = send_request()
                except error.HTTPError as retry_exc:
                    retry_body = retry_exc.read().decode("utf-8", errors="replace")
                    raise RuntimeError(
                        f"model request failed: HTTP {retry_exc.code}: {retry_body[:1000]}"
                    ) from retry_exc
            else:
                raise RuntimeError(
                    f"model request failed: HTTP {exc.code}: {body[:1000]}"
                ) from exc
        except (TimeoutError, http_client.IncompleteRead, http_client.RemoteDisconnected) as exc:
            raise TimeoutError(f"model request did not complete: {exc}") from exc

        choices = data.get("choices") or []
        if not choices:
            raise RuntimeError("model response did not include choices")
        message = choices[0].get("message") or {}
        if not isinstance(message, dict):
            raise RuntimeError("model response message was not an object")
        usage = data.get("usage")
        if isinstance(usage, dict):
            message["_codefactory_usage"] = {
                "prompt_tokens": int(usage.get("prompt_tokens") or 0),
                "completion_tokens": int(usage.get("completion_tokens") or 0),
                "total_tokens": int(usage.get("total_tokens") or 0),
            }
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
    def _timeout_recovery_prompt(
        loop_state: dict[str, Any], artifact_hint: str | None
    ) -> str:
        artifact = f" `{artifact_hint}`" if artifact_hint else " the expected artifact"
        if not CodeFactoryAgent._candidate_artifact_started(loop_state, artifact_hint):
            return (
                "Timeout recovery: the previous model response did not complete. "
                "Return exactly one `run_shell` tool call and no prose. That command "
                "must create or compile a candidate generator, write"
                f"{artifact}, or otherwise produce a candidate artifact now."
            )

        return (
            "Timeout recovery: the previous model response did not complete. Return "
            "exactly one `run_shell` tool call and no prose. That command must run the "
            "quickest verification or repair step for the current candidate artifact."
        )

    @staticmethod
    def _requires_no_action_recovery(
        loop_state: dict[str, Any],
        artifact_hint: str | None,
        recoveries: int,
        max_recoveries: int,
    ) -> bool:
        return (
            recoveries < max_recoveries
            and bool(artifact_hint)
            and not loop_state.get("artifact_started", False)
        )

    @staticmethod
    def _no_action_recovery_prompt(
        loop_state: dict[str, Any], artifact_hint: str | None
    ) -> str:
        artifact = f" `{artifact_hint}`" if artifact_hint else " the expected artifact"
        if not CodeFactoryAgent._candidate_artifact_started(loop_state, artifact_hint):
            return (
                "No-action recovery: the previous assistant response had no tool call, "
                "but the required artifact has not been produced. Return exactly one "
                "`run_shell` tool call and no prose. That command must create or compile "
                f"a generator, write{artifact}, or otherwise produce a candidate artifact now."
            )

        return (
            "No-action recovery: the previous assistant response had no tool call. Return "
            "exactly one `run_shell` tool call that verifies or repairs the current candidate."
        )

    @staticmethod
    def _requires_final_verification_recovery(
        loop_state: dict[str, Any],
        artifact_hint: str | None,
        recoveries: int,
        max_recoveries: int,
    ) -> bool:
        return (
            recoveries < max_recoveries
            and bool(artifact_hint)
            and bool(loop_state.get("artifact_started", False))
            and not bool(loop_state.get("verification_started", False))
        )

    @staticmethod
    def _final_verification_recovery_prompt(
        loop_state: dict[str, Any], artifact_hint: str | None
    ) -> str:
        artifact = f"`{artifact_hint}`" if artifact_hint else "the expected artifact"
        latest_goal = None
        repair_goals = loop_state.get("repair_goals")
        if isinstance(repair_goals, list) and repair_goals:
            latest_goal = repair_goals[-1]
        repair_tail = ""
        if isinstance(latest_goal, dict):
            repair_tail = (
                f" Use the latest repair goal `{latest_goal.get('kind', 'unknown')}` "
                "as the verification target."
            )
        return (
            "Final-before-verify gate: a candidate artifact was produced but no "
            "bounded verification command has run yet. Return exactly one `run_shell` "
            f"tool call that checks {artifact}: prefer the task's exact command, a "
            "`cmp`/`diff`/test invocation, or the smallest deterministic self-check. "
            "If it fails, patch before the final answer."
            f"{repair_tail}"
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

        if "implementation required" in normalized or "artifact command required" in normalized:
            return (
                "Repair focus: more inspection is currently blocked because the "
                f"candidate artifact has not been produced. The next command must write"
                f"{artifact} or run a generator that creates it; do not read task files again."
            )

        if "service supervision required" in normalized:
            return (
                "Repair focus: the previous command tried to start a foreground service. "
                "Start the service in the background with stdout/stderr redirected to a "
                "log, save its pid, run a bounded readiness check, and then run the "
                "client or verifier command."
            )

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

        if (
            "assertionerror" in normalized
            or "traceback (most recent call last)" in normalized
            or "=================================== failures" in normalized
            or re.search(r"\b\d+\s+failed\b", normalized)
        ):
            return (
                "Repair focus: the latest self-check failed. Do not finish yet. "
                "Use the failing assertion or traceback to patch the implementation, "
                "then rerun the smallest failing check before giving a final answer."
            )

        if "return_code=124" in normalized or "timed out" in normalized:
            return (
                "Repair focus: the latest command timed out. Replace broad exploration "
                "or long-running checks with a smaller deterministic check and continue "
                "from the known failure."
            )

        if tool_result.get("status") == "semantic-failure":
            return (
                "Repair focus: the latest command reported failure text even though the "
                "shell pipeline returned success. Do not trust the pipeline exit code; "
                "rerun with `set -o pipefail` or a smaller direct command, then repair "
                "from the actual error."
            )

        if tool_result.get("status") == "exec-error":
            return (
                "Repair focus: the latest command failed before returning an exit code. "
                "Do not repeat it unchanged; run a smaller command, inspect partial "
                "files if useful, or switch to a simpler implementation path."
            )

        return None

    @staticmethod
    def _repair_goal_from_tool_result(
        tool_result: dict[str, Any], artifact_hint: str | None
    ) -> dict[str, Any] | None:
        content = str(tool_result.get("content") or "")
        normalized = content.lower()
        command = str(tool_result.get("command") or "")
        artifact = artifact_hint or "expected artifact"

        if "service supervision required" in normalized:
            return {
                "kind": "service-lifecycle",
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": "foreground service command would block the agent",
                "next_action": (
                    "Start the service in the background, redirect logs, record pid, "
                    "poll readiness with a bounded loop, then run the client/test command."
                ),
                "smallest_rerun": "bounded readiness check plus client/test command",
            }

        if "long command policy required" in normalized:
            return {
                "kind": "bounded-command",
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": "command is unbounded or timeout-prone",
                "next_action": (
                    "Replace it with `timeout`, a smaller sample, or a deterministic "
                    "health/self-check that exits before the benchmark timeout."
                ),
                "smallest_rerun": "bounded sample or timeout-wrapped command",
            }

        if tool_result.get("status") == "semantic-failure":
            return {
                "kind": "semantic-command-failure",
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": CodeFactoryAgent._extract_semantic_failure(content)
                or "command output contains failure text despite return_code=0",
                "next_action": (
                    "Do not trust the pipeline exit status. Rerun the smallest direct "
                    "command with `set -o pipefail` or without `tail`, then choose an "
                    "available lower-cost path."
                ),
                "smallest_rerun": "direct command with pipefail or without output-truncating pipe",
            }

        if "segmentation fault" in normalized or "core dumped" in normalized:
            return {
                "kind": "crash",
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": "self-check crashed",
                "next_action": (
                    f"Patch the generator or artifact protocol for `{artifact}`, then "
                    "rerun the smallest round-trip command."
                ),
                "smallest_rerun": "round-trip artifact check",
            }

        if "command not found" in normalized:
            missing = CodeFactoryAgent._extract_missing_command(content)
            return {
                "kind": "missing-tool",
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": f"missing command{f' `{missing}`' if missing else ''}",
                "next_action": (
                    "Use an available shell/coreutils/compiler alternative instead "
                    "of reinstalling or retrying the same command."
                ),
                "smallest_rerun": "available-tool equivalent command",
            }

        assertion = CodeFactoryAgent._extract_assertion_or_traceback(content)
        if assertion:
            return {
                "kind": "assertion-failure",
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": assertion,
                "next_action": (
                    "Patch the implementation against this assertion or traceback, "
                    "then rerun the smallest failing test/check before final answer."
                ),
                "smallest_rerun": CodeFactoryAgent._single_line(command)
                or "smallest failing test/check",
            }

        if tool_result.get("status") == "exec-error":
            return {
                "kind": str(tool_result.get("error_type") or "exec-error"),
                "artifact": artifact_hint,
                "source_command": CodeFactoryAgent._single_line(command),
                "failure": "command failed before returning a normal exit code",
                "next_action": (
                    "Run a smaller diagnostic, inspect partial output, or switch "
                    "to a simpler implementation path."
                ),
                "smallest_rerun": "smaller diagnostic command",
            }

        return None

    @staticmethod
    def _format_repair_goal(goal: dict[str, Any]) -> str:
        return (
            "Repair goal: "
            f"kind={goal.get('kind', 'unknown')}; "
            f"failure={goal.get('failure', '')}; "
            f"next_action={goal.get('next_action', '')}; "
            f"smallest_rerun={goal.get('smallest_rerun', '')}."
        )

    @staticmethod
    def _extract_missing_command(content: str) -> str | None:
        match = re.search(r"(?P<command>[A-Za-z0-9_.+-]+):\s+command not found", content)
        if match:
            return match.group("command")
        return None

    @staticmethod
    def _extract_assertion_or_traceback(content: str) -> str | None:
        lines = [line.strip() for line in content.splitlines() if line.strip()]
        for line in lines:
            lowered = line.lower()
            if (
                "assertionerror" in lowered
                or lowered.startswith("e       assert")
                or re.search(r"\b\d+\s+failed\b", lowered)
            ):
                return CodeFactoryAgent._single_line(line, 300)
        if "traceback (most recent call last)" in content.lower():
            for line in reversed(lines):
                if re.search(r"(error|exception):", line, re.IGNORECASE):
                    return CodeFactoryAgent._single_line(line, 300)
            return "traceback failure"
        return None

    @staticmethod
    def _semantic_failure_reason(content: str) -> str | None:
        normalized = content.lower()
        if "return_code=0" not in normalized:
            return None
        return CodeFactoryAgent._extract_semantic_failure(content)

    @staticmethod
    def _extract_semantic_failure(content: str) -> str | None:
        failure_patterns = [
            r"ERROR:\s+[^\n]+",
            r"No space left on device",
            r"Could not install packages[^\n]*",
            r"Traceback \(most recent call last\):",
        ]
        for pattern in failure_patterns:
            match = re.search(pattern, content, re.IGNORECASE)
            if match:
                return CodeFactoryAgent._single_line(match.group(0), 300)
        return None

    def _auto_repair_command_from_tool_result(
        self,
        tool_result: dict[str, Any],
        artifact_hint: str | None,
        loop_state: dict[str, Any],
    ) -> str | None:
        auto_repair_value = self._bench_env("CODEFACTORY_BENCH_ENABLE_PROTOCOL_AUTO_REPAIR")
        if auto_repair_value is not None and auto_repair_value.lower() in {
            "0",
            "false",
            "no",
            "off",
        }:
            return None
        if int(loop_state.get("auto_protocol_repairs") or 0) >= 1:
            return None
        if not artifact_hint or Path(artifact_hint).name != "data.comp":
            return None

        content = str(tool_result.get("content") or "").lower()
        command = str(tool_result.get("command") or "").lower()
        if "/app/decomp" not in command and "decomp" not in content:
            return None
        if not (
            "segmentation fault" in content
            or "core dumped" in content
            or "verification-failed" in content
            or "exceeds" in content
            or "maximum allowed size" in content
        ):
            return None

        return self._write_compressor_auto_repair_command(artifact_hint)

    @staticmethod
    def _write_compressor_auto_repair_command(artifact_hint: str) -> str:
        artifact_path = artifact_hint if artifact_hint.startswith("/") else f"/app/{artifact_hint}"
        command = r"""cat > /tmp/codefactory_wc_repair.c <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MIN_LEN 2
#define MAX_LEN 2048
#define MAX_CTX 1000000
#define MAX_TOKENS 10000
#define LOW_CAP 20000

typedef struct {
  int type;
  int value;
  int length;
  int offset;
} Token;

static unsigned char *data_buf;
static int data_len;
static Token tokens[MAX_TOKENS];
static int token_count;
static int counts[MAX_CTX][2];
static unsigned char low_digits[LOW_CAP];
static int low_len = 0;
static long range_value = 1;

static void add_small(long value) {
  int index = 0;
  long carry = value;
  while (carry > 0) {
    if (index >= low_len) {
      if (low_len >= LOW_CAP) exit(10);
      low_digits[low_len++] = 0;
    }
    long sum = low_digits[index] + carry;
    low_digits[index] = (unsigned char)(sum % 255);
    carry = sum / 255;
    index++;
  }
}

static void shift_base255(void) {
  if (low_len >= LOW_CAP) exit(11);
  memmove(low_digits + 1, low_digits, (size_t)low_len);
  low_digits[0] = 0;
  low_len++;
}

static void put_bit(int ctx, int bit) {
  if (range_value < 255) {
    shift_base255();
    range_value *= 255;
  }
  long zeroes = counts[ctx][0];
  long ones = counts[ctx][1];
  long split = range_value * (zeroes + 1) / (zeroes + ones + 2);
  if (bit) {
    add_small(split);
    range_value -= split;
    counts[ctx][1]++;
  } else {
    range_value = split;
    counts[ctx][0]++;
  }
}

static void put_integer(int value, int initial_bits, int context) {
  int encoded = value + (1 << initial_bits);
  int width = 0;
  int probe = encoded;
  int ctx_base = context * 99;
  while (probe >>= 1) width++;
  for (int tmp = initial_bits + 1; tmp <= width; tmp++) put_bit(tmp + ctx_base, 0);
  put_bit(width + 1 + ctx_base, 1);
  for (int bit = width - 1; bit >= 0; bit--) put_bit(ctx_base, (encoded >> bit) & 1);
}

static unsigned char *read_file(const char *path, int *length) {
  FILE *file = fopen(path, "rb");
  if (!file) exit(20);
  if (fseek(file, 0, SEEK_END) != 0) exit(21);
  long size = ftell(file);
  if (size < 0) exit(22);
  if (fseek(file, 0, SEEK_SET) != 0) exit(23);
  unsigned char *buffer = (unsigned char *)malloc((size_t)size + 1);
  if (!buffer) exit(24);
  if (fread(buffer, 1, (size_t)size, file) != (size_t)size) exit(25);
  fclose(file);
  buffer[size] = 0;
  *length = (int)size + 1;
  return buffer;
}

static void best_match(int pos, int *best_length, int *best_offset) {
  *best_length = 0;
  *best_offset = 0;
  for (int candidate = 0; candidate < pos; candidate++) {
    if (data_buf[candidate] != data_buf[pos]) continue;
    if (pos + 1 >= data_len || data_buf[candidate + 1] != data_buf[pos + 1]) continue;
    int length = 0;
    while (
      pos + length < data_len &&
      data_buf[candidate + length] == data_buf[pos + length] &&
      length < MAX_LEN
    ) {
      length++;
    }
    if (length > *best_length) {
      *best_length = length;
      *best_offset = pos - candidate;
    }
  }
}

static void parse_tokens(void) {
  int pos = 0;
  while (pos < data_len) {
    int length = 0;
    int offset = 0;
    int next_length = 0;
    int next_offset = 0;
    best_match(pos, &length, &offset);
    if (length >= MIN_LEN && pos + 1 < data_len) {
      best_match(pos + 1, &next_length, &next_offset);
      if (next_length > length + 1) {
        tokens[token_count++] = (Token){0, data_buf[pos], 0, 0};
        pos++;
        continue;
      }
    }
    if (length >= MIN_LEN) {
      tokens[token_count++] = (Token){1, 0, length, offset};
      pos += length;
    } else {
      tokens[token_count++] = (Token){0, data_buf[pos], 0, 0};
      pos++;
    }
    if (token_count >= MAX_TOKENS) exit(30);
  }
}

static void encode_tokens(void) {
  put_integer(token_count, 9, 0);
  for (int index = 0; index < token_count; index++) {
    Token token = tokens[index];
    if (token.type == 0) {
      put_bit(1, 0);
      put_bit(8, 0);
      put_integer(token.value, 4, 9);
    } else {
      put_bit(1, 1);
      put_integer(token.offset - 1, 5, 2);
      put_integer(token.length - 1, 2, 3);
    }
  }
}

static void write_artifact(const char *path) {
  FILE *file = fopen(path, "wb");
  if (!file) exit(40);
  for (int index = low_len - 1; index >= 0; index--) {
    fputc((int)low_digits[index] + 1, file);
  }
  fclose(file);
}

int main(void) {
  data_buf = read_file("/app/data.txt", &data_len);
  parse_tokens();
  encode_tokens();
  write_artifact("__ARTIFACT_PATH__");
  printf("codefactory-auto-repair wrote __ARTIFACT_PATH__ bytes=%d tokens=%d\n", low_len, token_count);
  return 0;
}
EOF
gcc -O2 -o /tmp/codefactory_wc_repair /tmp/codefactory_wc_repair.c
/tmp/codefactory_wc_repair
if command -v gcc >/dev/null 2>&1; then gcc -O2 -o /app/decomp /app/decomp.c 2>/tmp/codefactory-decomp-gcc.log || true; fi
wc -c __ARTIFACT_PATH__
if [ -x /app/decomp ]; then
  cat __ARTIFACT_PATH__ | /app/decomp > /tmp/codefactory-bench-output && cmp -s /tmp/codefactory-bench-output /app/data.txt && echo codefactory-auto-repair-ok || echo codefactory-auto-repair-failed
fi"""
        return command.replace("__ARTIFACT_PATH__", artifact_path)

    def _requires_implementation_before_inspection(
        self,
        command: str,
        loop_state: dict[str, Any],
        step: int,
        artifact_hint: str | None,
    ) -> bool:
        inspection_rounds = self._int_env("CODEFACTORY_BENCH_INSPECTION_ROUNDS", 2)
        return (
            step >= inspection_rounds
            and not self._candidate_artifact_started(loop_state, artifact_hint)
            and self._is_inspection_only_command(command)
        )

    @staticmethod
    def _preflight_shell_command(
        command: str, loop_state: dict[str, Any]
    ) -> dict[str, str] | None:
        executable = CodeFactoryAgent._command_executable(command)
        if not executable:
            return None
        missing_commands = loop_state.get("missing_commands")
        if not isinstance(missing_commands, dict):
            return None
        if executable not in missing_commands:
            return None
        return {
            "reason": "previous command not found",
            "content": (
                "COMMAND PREFLIGHT BLOCKED: "
                f"`{executable}` was already unavailable in this task container. "
                "Do not retry the same missing executable. Use an existing tool, "
                "`python3`/shell built-ins, or inspect the workspace for the provided "
                "test/helper entrypoint before continuing."
            ),
        }

    @staticmethod
    def _record_tool_outcome(
        command: str,
        return_code: int,
        stdout: str | None,
        stderr: str | None,
        loop_state: dict[str, Any],
    ) -> None:
        if return_code != 127:
            return
        output = f"{stdout or ''}\n{stderr or ''}".lower()
        if "command not found" not in output and "not found" not in output:
            return
        executable = CodeFactoryAgent._command_executable(command)
        if not executable:
            return
        missing_commands = loop_state.setdefault("missing_commands", {})
        if isinstance(missing_commands, dict):
            missing_commands[executable] = True

    @staticmethod
    def _command_executable(command: str) -> str | None:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip()
        if not command_text:
            return None
        first_part = CodeFactoryAgent._compound_command_parts(command_text)[0]
        pipeline_first = first_part.split("|", 1)[0].strip()
        try:
            parts = shlex.split(pipeline_first)
        except ValueError:
            parts = pipeline_first.split()
        if not parts:
            return None
        if parts[0] in {"cd", "export", "env"} and len(parts) > 1:
            return None
        return parts[0].split("/")[-1]

    @staticmethod
    def _candidate_artifact_started(
        loop_state: dict[str, Any], artifact_hint: str | None
    ) -> bool:
        if artifact_hint:
            return loop_state.get("artifact_started", False)
        return loop_state.get("implementation_started", False)

    def _requires_artifact_command(
        self,
        command: str,
        loop_state: dict[str, Any],
        step: int,
        artifact_hint: str | None,
    ) -> bool:
        threshold = self._int_env("CODEFACTORY_BENCH_ARTIFACT_COMMAND_AFTER_BLOCKS", 2)
        step_threshold = self._int_env("CODEFACTORY_BENCH_ARTIFACT_COMMAND_STEP", 5)
        return (
            bool(artifact_hint)
            and not loop_state.get("artifact_started", False)
            and not self._is_artifact_attempt(command, artifact_hint)
            and (
                int(loop_state.get("implementation_required_count") or 0) >= threshold
                or step >= step_threshold
            )
        )

    @staticmethod
    def _requires_service_supervision(command: str) -> bool:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip().lower()
        if not command_text:
            return False
        if any(token in command_text for token in [" &", "nohup ", "setsid ", "timeout "]):
            return False
        if "docker run -d" in command_text or "daemon on" in command_text:
            return False

        service_patterns = [
            r"\bpython3?\s+-m\s+http\.server\b",
            r"\buvicorn\b",
            r"\bgunicorn\b",
            r"\bflask\s+run\b",
            r"\bnpm\s+(run\s+)?(dev|start)\b",
            r"\byarn\s+(dev|start)\b",
            r"\bpnpm\s+(dev|start)\b",
            r"\bnginx\s+-g\s+['\"]daemon off;",
            r"\bredis-server\b",
            r"\bpostgres\b",
            r"\bmysqld\b",
        ]
        return any(re.search(pattern, command_text) for pattern in service_patterns)

    @staticmethod
    def _long_command_policy_reason(command: str) -> str | None:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip().lower()
        if not command_text:
            return None
        if command_text.startswith("timeout ") or " timeout " in command_text:
            return None
        if re.search(r"\bwhile\s+true\b", command_text):
            return "infinite while loop"
        if re.search(r"\btail\s+-f\b", command_text):
            return "tail -f does not terminate"
        if re.search(r"\bwatch\s+(-n\s+\d+\s+)?", command_text):
            return "watch does not terminate without timeout"
        sleep_match = re.search(r"(?:^|[;&|]\s*)sleep\s+(\d+)", command_text)
        if sleep_match and int(sleep_match.group(1)) >= 120:
            return f"sleep {sleep_match.group(1)} exceeds bounded command policy"
        if re.search(r"\b(?:python3?|bash|sh)\s+.*(?:train|finetune|benchmark)\b", command_text):
            if not re.search(r"\b(?:--max_steps|--epochs\s+1|--limit|--sample|--dry-run)\b", command_text):
                return "training or benchmark command lacks a sample/step bound"
        return None

    @staticmethod
    def _background_process_lifecycle(command: str) -> dict[str, Any] | None:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip()
        normalized = command_text.lower()
        if not any(token in normalized for token in [" &", "nohup ", "setsid "]):
            return None
        service_like = any(
            re.search(pattern, normalized)
            for pattern in [
                r"\bpython3?\s+-m\s+http\.server\b",
                r"\buvicorn\b",
                r"\bgunicorn\b",
                r"\bflask\s+run\b",
                r"\bnpm\s+(run\s+)?(dev|start)\b",
                r"\bredis-server\b",
                r"\bpostgres\b",
                r"\bmysqld\b",
            ]
        )
        if not service_like:
            return None
        has_log = bool(re.search(r">\s*\S+|2>\s*\S+|2>&1", command_text))
        has_pid = bool(re.search(r"\$!\s*>\s*\S+|echo\s+\$!", command_text))
        has_readiness = bool(
            re.search(
                r"\b(timeout|until|for\s+\w+\s+in|nc\s+-z|curl|wget|python3?\s+- <<)",
                normalized,
            )
        )
        return {
            "started": True,
            "log_recorded": has_log,
            "pid_recorded": has_pid,
            "readiness_checked": has_readiness,
            "summary": (
                "background service command observed; logs, pid, and bounded "
                "readiness should be present before client verification"
            ),
        }

    @staticmethod
    def _is_verification_attempt(command: str, artifact_hint: str | None) -> bool:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).lower()
        if not command_text:
            return False
        verification_tokens = [
            "pytest",
            "cargo test",
            "npm test",
            "pnpm test",
            "make test",
            "go test",
            "cmp ",
            "diff ",
            "verification-ok",
            "verification-failed",
        ]
        if any(token in command_text for token in verification_tokens):
            return True
        if artifact_hint and artifact_hint.lower() in command_text:
            return any(token in command_text for token in [" | ", "test -", "[ -", "wc -c"])
        return False

    @staticmethod
    def _is_inspection_only_command(command: str) -> bool:
        return CodeFactoryAgent._inspection_target_key(command) is not None

    @staticmethod
    def _is_implementation_attempt(command: str) -> bool:
        command_text = CodeFactoryAgent._strip_heredoc_bodies(command).strip()
        if not command_text or CodeFactoryAgent._is_inspection_only_command(command_text):
            return False
        if any(token in command_text for token in [">", ">>", "<<"]):
            return True
        try:
            parts = shlex.split(command_text)
        except ValueError:
            parts = command_text.split()
        if not parts:
            return False
        first = parts[0].split("/")[-1]
        if first in {
            "bash",
            "cc",
            "clang",
            "cp",
            "gcc",
            "make",
            "mv",
            "node",
            "perl",
            "python",
            "python3",
            "rustc",
            "sh",
            "touch",
        }:
            return True
        return first.startswith("./") or command_text.startswith("./")

    @staticmethod
    def _is_artifact_attempt(command: str, artifact_hint: str | None) -> bool:
        if not artifact_hint:
            return CodeFactoryAgent._is_implementation_attempt(command)
        if CodeFactoryAgent._is_inspection_only_command(command):
            return False

        normalized = command.lower()
        artifact = artifact_hint.lower()
        if artifact not in normalized:
            return False

        if any(token in normalized for token in [">", ">>", "<<"]):
            return True
        try:
            parts = shlex.split(command)
        except ValueError:
            parts = command.split()
        if not parts:
            return False
        executable = parts[0].split("/")[-1]
        return executable in {
            "bash",
            "cp",
            "mv",
            "node",
            "perl",
            "python",
            "python3",
            "sh",
            "tee",
            "touch",
        }

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
        if any(token in command_text for token in [">", ">>", "$("]):
            return None

        compound_parts = CodeFactoryAgent._compound_command_parts(command_text)
        if len(compound_parts) > 1:
            keys: list[str] = []
            for part in compound_parts:
                if CodeFactoryAgent._is_context_only_command(part):
                    continue
                key = CodeFactoryAgent._inspection_target_key(part)
                if not key:
                    return None
                keys.append(key)
            if not keys:
                return None
            return "compound-read:" + "|".join(keys)

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
    def _compound_command_parts(command_text: str) -> list[str]:
        parts: list[str] = []
        current: list[str] = []
        quote: str | None = None
        index = 0
        while index < len(command_text):
            char = command_text[index]
            if quote:
                current.append(char)
                if char == quote:
                    quote = None
                index += 1
                continue
            if char in {"'", '"'}:
                quote = char
                current.append(char)
                index += 1
                continue
            if command_text.startswith("&&", index) or char == ";":
                part = "".join(current).strip()
                if part:
                    parts.append(part)
                current = []
                index += 2 if command_text.startswith("&&", index) else 1
                continue
            current.append(char)
            index += 1

        part = "".join(current).strip()
        if part:
            parts.append(part)
        return parts or [command_text.strip()]

    @staticmethod
    def _is_context_only_command(command_text: str) -> bool:
        try:
            parts = shlex.split(command_text)
        except ValueError:
            return False
        return bool(parts) and parts[0] == "cd"

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
    def _exec_exception_type(exc: Exception) -> str:
        message = str(exc).lower()
        if isinstance(exc, TimeoutError) or "timed out" in message or "timeout" in message:
            return "command-timeout"
        if "docker" in message or "container" in message or "harbor" in message:
            return "environment-exec-error"
        return "exec-runtime-error"

    @staticmethod
    def _format_exec_exception(exc: Exception, command: str, timeout_sec: int) -> str:
        command_line = CodeFactoryAgent._single_line(
            CodeFactoryAgent._strip_heredoc_bodies(command)
        )
        return (
            f"EXECUTION ERROR ({CodeFactoryAgent._exec_exception_type(exc)})\n"
            f"timeout_sec={timeout_sec}\n"
            f"command={command_line}\n"
            f"error={type(exc).__name__}: {exc}\n"
            "The command did not produce a normal exit code. Continue by running a "
            "smaller diagnostic, checking partial files, or choosing a simpler command."
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
