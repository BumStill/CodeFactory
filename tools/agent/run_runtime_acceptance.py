#!/usr/bin/env python3
"""Run the CodeFactory Agent runtime without depending on a visible desktop.

This is a product acceptance driver, not a benchmark adapter. It resolves the
current CodeFactory endpoint/model, launches the shared Rust runtime, executes
requested commands in one selected working directory, and writes redacted
evidence with an explicit non-GUI proof tier.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, TextIO


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONTRACT_PATH = REPO_ROOT / "agent_contracts" / "execution_completion.md"
KEYCHAIN_SERVICE = "com.codefactory.app"
MAX_CAPTURE_CHARS = 30_000
REDACTED = "[REDACTED]"
SENSITIVE_ASSIGNMENT = re.compile(
    r"(?i)(api[_-]?key|access[_-]?token|auth[_-]?token|password|secret)"
    r"(\s*[=:]\s*[\"']?)([^\s,;\"']+)"
)
TOOL_ENV_ALLOWLIST = {
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "USER",
    "LOGNAME",
    "SHELL",
    "SDKROOT",
    "DEVELOPER_DIR",
    "RUSTUP_HOME",
    "CARGO_HOME",
    "PNPM_HOME",
    "VOLTA_HOME",
    "NVM_DIR",
}


@dataclass(frozen=True)
class RuntimeConfig:
    endpoint_id: str
    base_url: str
    model: str
    key_ref: str


def default_settings_path() -> Path:
    if sys.platform == "darwin":
        return (
            Path.home()
            / "Library"
            / "Application Support"
            / "com.codefactory.app"
            / "settings.json"
        )
    if os.name == "nt":
        appdata = os.environ.get("APPDATA")
        if not appdata:
            raise RuntimeError("APPDATA is unavailable")
        return Path(appdata) / "com.codefactory.app" / "settings.json"
    return Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / (
        "com.codefactory.app/settings.json"
    )


def _normalize_direct_model(endpoint_id: str, base_url: str, model: str) -> str:
    model = model.strip()
    if not model or "/" not in model:
        return model
    identity = f"{endpoint_id} {base_url}".lower()
    if "openrouter" in identity:
        return model
    provider, direct_model = model.split("/", 1)
    provider_aliases = {
        "deepseek": ("deepseek",),
        "anthropic": ("anthropic", "claude"),
        "openai": ("openai", "chatgpt"),
        "google": ("google", "gemini"),
    }
    aliases = provider_aliases.get(provider.lower(), (provider.lower(),))
    return direct_model if any(alias in identity for alias in aliases) else model


def load_runtime_config(
    settings_path: Path,
    *,
    endpoint_override: str | None = None,
    model_override: str | None = None,
) -> RuntimeConfig:
    try:
        settings = json.loads(settings_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise RuntimeError(f"CodeFactory settings not found: {settings_path}") from exc
    except (OSError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"Could not read CodeFactory settings: {exc}") from exc

    endpoint_id = (endpoint_override or settings.get("default_endpoint") or "").strip()
    endpoints = settings.get("endpoints")
    endpoint = endpoints.get(endpoint_id) if isinstance(endpoints, dict) else None
    if not endpoint_id or not isinstance(endpoint, dict):
        raise RuntimeError(f"CodeFactory endpoint is not configured: {endpoint_id or '<empty>'}")
    if str(endpoint.get("api_style") or "openai").lower() == "chatgpt":
        raise RuntimeError(
            "ChatGPT subscription transport is not supported by the no-GUI runtime yet"
        )

    base_url = str(endpoint.get("base_url") or "").strip()
    model = str(
        model_override
        or endpoint.get("active_model")
        or settings.get("default_model")
        or ""
    ).strip()
    key_ref = str(
        endpoint.get("key_ref") or f"codefactory.endpoint.{endpoint_id}"
    ).strip()
    if not base_url:
        raise RuntimeError(f"CodeFactory endpoint '{endpoint_id}' has no base_url")
    if not model:
        raise RuntimeError(f"CodeFactory endpoint '{endpoint_id}' has no active model")
    if not key_ref:
        raise RuntimeError(f"CodeFactory endpoint '{endpoint_id}' has no key_ref")

    return RuntimeConfig(
        endpoint_id=endpoint_id,
        base_url=base_url,
        model=_normalize_direct_model(endpoint_id, base_url, model),
        key_ref=key_ref,
    )


def lookup_api_key(key_ref: str, *, timeout_sec: int = 8) -> str:
    explicit = os.environ.get("CODEFACTORY_AGENT_API_KEY", "").strip()
    if explicit:
        return explicit
    if sys.platform != "darwin":
        raise RuntimeError(
            "Set CODEFACTORY_AGENT_API_KEY in memory for this runtime acceptance run"
        )
    try:
        completed = subprocess.run(
            [
                "security",
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                key_ref,
                "-w",
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=max(1, timeout_sec),
        )
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError(
            "OS credential lookup timed out; unlock or pre-authorize the credential once"
        ) from exc
    if completed.returncode != 0:
        raise RuntimeError(f"OS credential is unavailable for key_ref '{key_ref}'")
    secret = completed.stdout.strip()
    if not secret:
        raise RuntimeError(f"OS credential is empty for key_ref '{key_ref}'")
    return secret


def detect_macos_screen_locked() -> bool:
    if sys.platform != "darwin":
        return False
    completed = subprocess.run(
        ["ioreg", "-n", "Root", "-d1"],
        check=False,
        capture_output=True,
        text=True,
        timeout=5,
    )
    return '"CGSSessionScreenIsLocked"=Yes' in completed.stdout


def locate_sidecar(explicit: Path | None = None) -> Path:
    executable = "codefactory-agent-headless.exe" if os.name == "nt" else "codefactory-agent-headless"
    candidates = [explicit] if explicit else []
    candidates.extend(
        [
            REPO_ROOT / "src-tauri" / "target" / "release" / executable,
            REPO_ROOT / "src-tauri" / "target" / "debug" / executable,
        ]
    )
    for candidate in candidates:
        if candidate and candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate.resolve()
    checked = ", ".join(str(path) for path in candidates if path)
    raise RuntimeError(
        "CodeFactory Agent runtime binary not found. Run "
        f"`cargo build -p codefactory-agent-headless` first. Checked: {checked}"
    )


def _as_text(value: str | bytes) -> str:
    return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else value


def _truncate(value: str | bytes, limit: int = MAX_CAPTURE_CHARS) -> str:
    value = _as_text(value)
    if len(value) <= limit:
        return value
    half = max(1, (limit - 80) // 2)
    omitted = len(value) - half * 2
    return f"{value[:half]}\n...[{omitted} chars omitted]...\n{value[-half:]}"


def _redact_text(value: str | bytes, secrets: tuple[str, ...]) -> str:
    redacted = _as_text(value)
    for secret in secrets:
        if secret:
            redacted = redacted.replace(secret, REDACTED)
    return SENSITIVE_ASSIGNMENT.sub(
        lambda match: f"{match.group(1)}{match.group(2)}{REDACTED}", redacted
    )


def _redact_value(value: Any, secrets: tuple[str, ...]) -> Any:
    if isinstance(value, str):
        return _redact_text(value, secrets)
    if isinstance(value, list):
        return [_redact_value(item, secrets) for item in value]
    if isinstance(value, dict):
        return {key: _redact_value(item, secrets) for key, item in value.items()}
    return value


def _minimal_runtime_env(runtime_home: Path) -> dict[str, str]:
    runtime_home.mkdir(parents=True, exist_ok=True)
    runtime_tmp = runtime_home / "tmp"
    runtime_tmp.mkdir(parents=True, exist_ok=True)
    env = {
        key: value
        for key, value in os.environ.items()
        if key in TOOL_ENV_ALLOWLIST and value
    }
    env.update(
        {
            "HOME": str(runtime_home),
            "TMPDIR": str(runtime_tmp),
            "CODEFACTORY_AGENT_MODE": "product-acceptance",
        }
    )
    return env


def _write_macos_workspace_profile(
    *, cwd: Path, runtime_root: Path, allow_network: bool
) -> Path:
    if sys.platform != "darwin" or not Path("/usr/bin/sandbox-exec").is_file():
        raise RuntimeError(
            "workspace write isolation is currently available only on macOS"
        )
    quoted_cwd = json.dumps(str(cwd.resolve()))
    quoted_runtime = json.dumps(str(runtime_root.resolve()))
    rules = [
        "(version 1)",
        "(deny default)",
        "(allow process*)",
        "(allow signal)",
        "(allow sysctl-read)",
        "(allow mach-lookup)",
        "(allow file-read*)",
        (
            "(allow file-write* "
            f"(subpath {quoted_cwd}) "
            f"(subpath {quoted_runtime}) "
            '(literal "/dev/null"))'
        ),
    ]
    if allow_network:
        rules.append("(allow network*)")
    profile_path = runtime_root / "workspace.sb"
    profile_path.write_text("\n".join(rules) + "\n", encoding="utf-8")
    return profile_path


def _command_argv(command: str, sandbox_profile: Path) -> list[str]:
    if os.name == "nt":
        raise RuntimeError("workspace write isolation is not implemented on Windows")
    return [
        "/usr/bin/sandbox-exec",
        "-f",
        str(sandbox_profile),
        "/bin/bash",
        "--noprofile",
        "--norc",
        "-c",
        command,
    ]


def _execute_command(
    command: str,
    cwd: Path,
    timeout_sec: int,
    *,
    sandbox_profile: Path,
    tool_env: dict[str, str],
    secrets: tuple[str, ...],
) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            _command_argv(command, sandbox_profile),
            cwd=cwd,
            env=tool_env,
            check=False,
            capture_output=True,
            text=True,
            timeout=max(1, min(timeout_sec, 900)),
        )
        return {
            "return_code": completed.returncode,
            "stdout": _redact_text(_truncate(completed.stdout), secrets),
            "stderr": _redact_text(_truncate(completed.stderr), secrets),
            "error": None,
        }
    except subprocess.TimeoutExpired as exc:
        return {
            "return_code": None,
            "stdout": _redact_text(_truncate(exc.stdout or ""), secrets),
            "stderr": _redact_text(_truncate(exc.stderr or ""), secrets),
            "error": f"command timed out after {timeout_sec}s",
        }
    except OSError as exc:
        return {
            "return_code": None,
            "stdout": "",
            "stderr": "",
            "error": f"command execution failed: {exc}",
        }


def _line_reader(stream: TextIO, output: queue.Queue[str | None]) -> None:
    try:
        for line in stream:
            output.put(line)
    finally:
        output.put(None)


def _stderr_reader(stream: TextIO, chunks: list[str]) -> None:
    for chunk in iter(lambda: stream.read(4096), ""):
        chunks.append(chunk)


def _write_evidence(
    evidence_dir: Path,
    trajectory: list[dict[str, Any]],
    result: dict[str, Any],
    *,
    secrets: tuple[str, ...],
) -> None:
    evidence_dir.mkdir(parents=True, exist_ok=True)
    trajectory = _redact_value(trajectory, secrets)
    result = _redact_value(result, secrets)
    (evidence_dir / "trajectory.jsonl").write_text(
        "".join(json.dumps(item, ensure_ascii=False, sort_keys=True) + "\n" for item in trajectory),
        encoding="utf-8",
    )
    (evidence_dir / "result.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def run_runtime_acceptance(
    *,
    instruction: str,
    cwd: Path,
    evidence_dir: Path,
    sidecar_path: Path,
    config: RuntimeConfig,
    api_key: str,
    contract_path: Path = DEFAULT_CONTRACT_PATH,
    screen_locked: bool | None = None,
    max_steps: int = 80,
    model_timeout_sec: int = 90,
    shell_timeout_sec: int = 300,
    wall_time_budget_sec: int = 1800,
    allow_network: bool = False,
) -> dict[str, Any]:
    cwd = cwd.resolve()
    if not cwd.is_dir():
        raise ValueError(f"working directory is not a directory: {cwd}")
    if not instruction.strip():
        raise ValueError("instruction is empty")
    if not sidecar_path.is_file():
        raise ValueError(f"runtime sidecar does not exist: {sidecar_path}")
    contract = contract_path.read_bytes()
    contract_sha = hashlib.sha256(contract).hexdigest()
    screen_locked = detect_macos_screen_locked() if screen_locked is None else screen_locked
    started_at = datetime.now(timezone.utc).isoformat()
    started_monotonic = time.monotonic()
    start_message = {
        "type": "start",
        "instruction": instruction,
        "model": config.model,
        "api_key": api_key,
        "base_url": config.base_url,
        "max_steps": max(1, max_steps),
        "model_timeout_sec": max(1, model_timeout_sec),
        "shell_timeout_sec": max(1, shell_timeout_sec),
        "wall_time_budget_sec": max(1, wall_time_budget_sec),
        "working_directory": str(cwd),
        "allow_network": allow_network,
        "policy_profile": "product",
        "execution_contract_sha256": contract_sha,
    }
    runtime_root = Path(tempfile.mkdtemp(prefix="codefactory-agent-runtime-")).resolve()
    try:
        runtime_home = runtime_root / "home"
        tool_env = _minimal_runtime_env(runtime_home)
        sandbox_profile = _write_macos_workspace_profile(
            cwd=cwd, runtime_root=runtime_root, allow_network=allow_network
        )
        process = subprocess.Popen(
            [str(sidecar_path)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=tool_env,
        )
    except BaseException:
        shutil.rmtree(runtime_root, ignore_errors=True)
        raise
    assert process.stdin and process.stdout and process.stderr
    stdout_lines: queue.Queue[str | None] = queue.Queue()
    stderr_chunks: list[str] = []
    threading.Thread(
        target=_line_reader, args=(process.stdout, stdout_lines), daemon=True
    ).start()
    threading.Thread(
        target=_stderr_reader, args=(process.stderr, stderr_chunks), daemon=True
    ).start()
    trajectory: list[dict[str, Any]] = []
    finished: dict[str, Any] | None = None
    deadline = started_monotonic + max(1, wall_time_budget_sec)
    try:
        process.stdin.write(json.dumps(start_message) + "\n")
        process.stdin.flush()
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("Agent runtime exceeded the wall-time budget")
            try:
                raw = stdout_lines.get(timeout=remaining)
            except queue.Empty as exc:
                raise RuntimeError("Agent runtime exceeded the wall-time budget") from exc
            if raw is None:
                detail = _truncate("".join(stderr_chunks), 2000).strip()
                raise RuntimeError(
                    "Agent runtime ended before a finished message"
                    + (f": {detail}" if detail else "")
                )
            try:
                message = json.loads(raw)
            except json.JSONDecodeError as exc:
                raise RuntimeError(f"Agent runtime returned invalid JSON: {exc}") from exc
            message_type = message.get("type")
            if message_type == "tool_request":
                request_id = str(message.get("id") or "")
                command = str(message.get("command") or "").strip()
                if not request_id or not command:
                    raise RuntimeError("Agent runtime returned an incomplete tool request")
                trajectory.append(
                    {
                        "type": "tool_request",
                        "id": request_id,
                        "command": command,
                        "timeout_sec": int(message.get("timeout_sec") or shell_timeout_sec),
                    }
                )
                outcome = _execute_command(
                    command,
                    cwd,
                    int(message.get("timeout_sec") or shell_timeout_sec),
                    sandbox_profile=sandbox_profile,
                    tool_env=tool_env,
                    secrets=(api_key,),
                )
                tool_result = {"type": "tool_result", "id": request_id, **outcome}
                trajectory.append(tool_result)
                process.stdin.write(json.dumps(tool_result) + "\n")
                process.stdin.flush()
                continue
            if message_type == "finished":
                finished = message
                break
            if message_type == "event":
                trajectory.append(message)
                continue
            raise RuntimeError(f"Agent runtime returned unknown message type: {message_type}")
    finally:
        if process.stdin and not process.stdin.closed:
            process.stdin.close()
        if finished is None and process.poll() is None:
            process.kill()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        process.stdout.close()
        process.stderr.close()
        shutil.rmtree(runtime_root, ignore_errors=True)

    if process.returncode != 0:
        detail = _truncate("".join(stderr_chunks), 2000).strip()
        raise RuntimeError(f"Agent runtime exited with {process.returncode}: {detail}")
    assert finished is not None
    if finished.get("execution_contract_sha256") != contract_sha:
        raise RuntimeError("Agent runtime execution contract hash mismatch")
    completion_evidence = finished.get("completion_evidence") or {}
    passed = bool(completion_evidence.get("completed"))
    screen_locked_at_end = detect_macos_screen_locked()
    result = {
        "status": "passed" if passed else "failed",
        "proof_tier": "agent-runtime-no-gui",
        "screen_locked": screen_locked,
        "screen_locked_at_start": screen_locked,
        "screen_locked_at_end": screen_locked_at_end,
        "provider": config.endpoint_id,
        "model": config.model,
        "working_directory": str(cwd),
        "instruction_sha256": hashlib.sha256(instruction.encode("utf-8")).hexdigest(),
        "execution_contract_sha256": contract_sha,
        "completion_evidence": completion_evidence,
        "final_text": _redact_text(str(finished.get("final_text") or ""), (api_key,)),
        "usage": finished.get("usage") or {},
        "tool_calls": sum(item.get("type") == "tool_request" for item in trajectory),
        "started_at": started_at,
        "duration_ms": int((time.monotonic() - started_monotonic) * 1000),
        # GUI proof is produced by the separate remote macOS harness. A local
        # lock is recorded above but never becomes a delivery wait state.
        "gui_status": "not_evaluated",
        "workspace_write_isolation": "macos-sandbox-exec",
        "host": {"os": platform.system(), "arch": platform.machine()},
    }
    _write_evidence(evidence_dir, trajectory, result, secrets=(api_key,))
    return result


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    instruction = parser.add_mutually_exclusive_group(required=True)
    instruction.add_argument("--instruction")
    instruction.add_argument("--instruction-file", type=Path)
    parser.add_argument("--cwd", required=True, type=Path)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    parser.add_argument("--settings", type=Path, default=default_settings_path())
    parser.add_argument("--endpoint")
    parser.add_argument("--model")
    parser.add_argument("--sidecar", type=Path)
    parser.add_argument("--max-steps", type=int, default=80)
    parser.add_argument("--model-timeout-sec", type=int, default=90)
    parser.add_argument("--shell-timeout-sec", type=int, default=300)
    parser.add_argument("--wall-time-budget-sec", type=int, default=1800)
    parser.add_argument("--allow-network", action="store_true")
    parser.add_argument("--credential-timeout-sec", type=int, default=8)
    return parser


def main() -> int:
    args = _parser().parse_args()
    try:
        instruction = (
            args.instruction
            if args.instruction is not None
            else args.instruction_file.read_text(encoding="utf-8")
        )
        config = load_runtime_config(
            args.settings,
            endpoint_override=args.endpoint,
            model_override=args.model,
        )
        api_key = lookup_api_key(config.key_ref, timeout_sec=args.credential_timeout_sec)
        result = run_runtime_acceptance(
            instruction=instruction,
            cwd=args.cwd,
            evidence_dir=args.evidence_dir,
            sidecar_path=locate_sidecar(args.sidecar),
            config=config,
            api_key=api_key,
            max_steps=args.max_steps,
            model_timeout_sec=args.model_timeout_sec,
            shell_timeout_sec=args.shell_timeout_sec,
            wall_time_budget_sec=args.wall_time_budget_sec,
            allow_network=args.allow_network,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"CodeFactory Agent runtime acceptance blocked: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(result, ensure_ascii=False, sort_keys=True))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
