#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SUBSET_PATH = REPO_ROOT / "docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json"
EVIDENCE_DIR = REPO_ROOT / "docs/evidence-packs"
LOOPBACK_NO_PROXY = "localhost,127.0.0.1,127.0.0.0/8,::1,0.0.0.0"
TEST_NAME = "benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings"
DEFAULT_TRIAL_HARD_TIMEOUT_SEC = 0
DEFAULT_HEAVY_VERIFIER_HARD_TIMEOUT_SEC = 0
DEFAULT_HEAVY_VERIFIER_TIMEOUT_MULTIPLIER = 1.0
HOST_DEADLINE_RESERVE_SEC = 120
MIN_AGENT_RUNTIME_SEC = 30
HEAVY_VERIFIER_TRIAL_PREFIXES = ("torch-tensor-parallelism",)
BOOTSTRAP_SMOKE_IMAGES = [
    ("debian-bookworm", "python:3.10-slim-bookworm"),
    ("ubuntu-noble", "ubuntu:24.04"),
]
BIND_MOUNT_PROBE_TOKEN = "codefactory-terminal-bench-bind-probe-v1"


@dataclass(frozen=True)
class CapturedCommand:
    returncode: int
    output: str


@dataclass(frozen=True)
class PreflightResult:
    ok: bool
    blockers: list[str]
    details: list[str]


@dataclass(frozen=True)
class WatchdogIntervention:
    trial: str
    elapsed_sec: int
    containers: list[str]
    action: str


@dataclass(frozen=True)
class VerifierEnvironmentWarning:
    trial: str
    category: str
    evidence: str


VERIFIER_ENVIRONMENT_WARNING_PATTERNS = [
    (
        "browser-driver-unavailable",
        re.compile(
            r"Unable to obtain driver for chrome|"
            r"Failed to create driver or process file|"
            r"SessionNotCreatedException|"
            r"ChromeDriver only supports Chrome version",
            re.IGNORECASE,
        ),
    ),
    (
        "emulated-browser-runtime",
        re.compile(
            r"running under QEMU|"
            r"unknown platform bitness|"
            r"lacks support for the sse3 instruction set",
            re.IGNORECASE,
        ),
    ),
    (
        "verifier-python-bootstrap-network",
        re.compile(
            r"UNEXPECTED_EOF_WHILE_READING|"
            r"RemoteDisconnected|"
            r"Failed to download|"
            r"Failed to fetch|"
            r"Error reading from server\. Remote end closed connection|"
            r"Request failed after \d+ retries|"
            r"tls handshake eof|"
            r"error sending request for url|"
            r"Could not find a version that satisfies the requirement pytest|"
            r"No matching distribution found for pytest|"
            r"Unable to locate package curl|"
            r"/root/\.local/bin/env: No such file or directory|"
            r"uvx: command not found|"
            r"astral\.sh|"
            r"curl: \((?:18|35|56)\)",
            re.IGNORECASE,
        ),
    ),
]


class BenchmarkWatchdog:
    def __init__(
        self,
        timeout_sec: int,
        poll_interval_sec: int = 15,
        trial_timeout_overrides: dict[str, int] | None = None,
    ) -> None:
        self.timeout_sec = timeout_sec
        self.poll_interval_sec = poll_interval_sec
        self.trial_timeout_overrides = trial_timeout_overrides or {}
        self.job_path: Path | None = None
        self._first_seen: dict[str, float] = {}
        self._interventions: list[WatchdogIntervention] = []
        self._stopped = threading.Event()
        self._lock = threading.Lock()
        self._thread: threading.Thread | None = None

    @property
    def enabled(self) -> bool:
        return self.timeout_sec > 0

    def observe_output_line(self, line: str) -> None:
        if not self.enabled or self.job_path is not None:
            return
        match = re.search(r"\bjob_path=(?P<job_path>\S+)", line)
        if match:
            self.job_path = Path(match.group("job_path"))

    def start(self) -> None:
        if not self.enabled:
            return
        self._thread = threading.Thread(target=self._run, name="tb21-watchdog", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stopped.set()
        if self._thread:
            self._thread.join(timeout=5)

    def interventions(self) -> list[WatchdogIntervention]:
        with self._lock:
            return list(self._interventions)

    def check_once(self, now: float | None = None) -> list[str]:
        if not self.enabled or self.job_path is None:
            return []
        now = now if now is not None else time.monotonic()
        messages: list[str] = []
        for trial_path in self._running_trial_paths():
            trial_name = trial_path.name
            if trial_name not in self._first_seen:
                trial_age_sec = 0.0
                try:
                    trial_started_at = (trial_path / "config.json").stat().st_mtime
                    trial_age_sec = max(0.0, time.time() - trial_started_at)
                except OSError:
                    pass
                self._first_seen[trial_name] = now - trial_age_sec
            first_seen = self._first_seen[trial_name]
            elapsed = int(now - first_seen)
            timeout_sec = self._timeout_for_trial(trial_name)
            if elapsed < timeout_sec or self._already_intervened(trial_name):
                continue
            sidecar_status = self._stop_trial_sidecar(trial_path)
            if sidecar_status == "stop-failed":
                messages.append(
                    "benchmark_watchdog_timeout "
                    f"trial={trial_name} elapsed_sec={elapsed} "
                    "containers=<deferred> action=sidecar-stop-failed-retry"
                )
                continue
            containers = self._stop_trial_containers(trial_name)
            if sidecar_status == "verified-stopped" and containers:
                action = "sidecar-stop+docker-stop"
            elif sidecar_status == "verified-stopped":
                action = "sidecar-stop"
            elif containers:
                action = "docker-stop"
            else:
                action = "no-container-found"
            intervention = WatchdogIntervention(
                trial=trial_name,
                elapsed_sec=elapsed,
                containers=containers,
                action=action,
            )
            with self._lock:
                self._interventions.append(intervention)
            messages.append(
                "benchmark_watchdog_timeout "
                f"trial={trial_name} elapsed_sec={elapsed} "
                f"containers={','.join(containers) if containers else '<none>'} "
                f"action={intervention.action}"
            )
        return messages

    def _run(self) -> None:
        while not self._stopped.wait(self.poll_interval_sec):
            for message in self.check_once():
                print(message, flush=True)

    def _running_trial_paths(self) -> list[Path]:
        if self.job_path is None or not self.job_path.is_dir():
            return []
        running: list[Path] = []
        for path in self.job_path.iterdir():
            if not path.is_dir():
                continue
            if (path / "result.json").exists():
                continue
            if not (path / "config.json").exists():
                continue
            running.append(path)
        return sorted(running)

    def _already_intervened(self, trial_name: str) -> bool:
        with self._lock:
            return any(item.trial == trial_name for item in self._interventions)

    def _timeout_for_trial(self, trial_name: str) -> int:
        task_name = trial_name.split("__", 1)[0]
        return self.trial_timeout_overrides.get(task_name, self.timeout_sec)

    def _stop_trial_sidecar(self, trial_path: Path) -> str:
        runtime_path = trial_path / "agent" / "sidecar-runtime.json"
        try:
            runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return "stop-failed"
        except (OSError, json.JSONDecodeError):
            return "stop-failed"
        pid = runtime.get("pid")
        binary = runtime.get("binary")
        runtime_token = runtime.get("runtime_token")
        process_group_id = runtime.get("process_group_id")
        if runtime.get("status") == "stopped":
            return "not-running"
        if runtime.get("status") != "running" or not isinstance(pid, int) or pid <= 0:
            return "stop-failed"
        if not isinstance(binary, str) or not binary:
            return "stop-failed"
        if not isinstance(runtime_token, str) or not runtime_token:
            return "stop-failed"
        if process_group_id != pid:
            return "stop-failed"
        process_state = self._sidecar_process_state(
            pid, binary, runtime_token, process_group_id
        )
        if process_state == "absent":
            return "not-running"
        if process_state != "matches":
            return "stop-failed"
        if not self._signal_process_group(process_group_id, "-TERM"):
            process_state = self._sidecar_process_state(
                pid, binary, runtime_token, process_group_id
            )
            if process_state == "absent":
                return "not-running"
            return "stop-failed"
        for _ in range(10):
            process_state = self._sidecar_process_state(
                pid, binary, runtime_token, process_group_id
            )
            if process_state == "absent":
                return "verified-stopped"
            if process_state in {"unknown", "mismatch"}:
                return "stop-failed"
            time.sleep(0.1)
        if not self._signal_process_group(process_group_id, "-KILL"):
            return "stop-failed"
        for _ in range(10):
            process_state = self._sidecar_process_state(
                pid, binary, runtime_token, process_group_id
            )
            if process_state == "absent":
                return "verified-stopped"
            if process_state in {"unknown", "mismatch"}:
                return "stop-failed"
            time.sleep(0.1)
        return "stop-failed"

    @staticmethod
    def _sidecar_process_state(
        pid: int,
        binary: str,
        runtime_token: str,
        process_group_id: int,
    ) -> str:
        try:
            result = subprocess.run(
                ["ps", "-p", str(pid), "-o", "pgid=", "-o", "command="],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=5,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return "unknown"
        output = result.stdout.strip()
        if result.returncode == 1 and not output:
            return "absent"
        if result.returncode != 0 or not output:
            return "unknown"
        fields = output.split(maxsplit=1)
        if len(fields) != 2:
            return "unknown"
        try:
            actual_process_group_id = int(fields[0])
        except ValueError:
            return "unknown"
        command = fields[1]
        token_arg = f"--codefactory-runtime-token={runtime_token}"
        if actual_process_group_id != process_group_id:
            return "mismatch"
        if not command.startswith(f"{binary} ") or token_arg not in command.split():
            return "mismatch"
        return "matches"

    @staticmethod
    def _signal_process_group(process_group_id: int, signal: str) -> bool:
        try:
            result = subprocess.run(
                ["kill", signal, "--", f"-{process_group_id}"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=5,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return False
        return result.returncode == 0

    def _stop_trial_containers(self, trial_name: str) -> list[str]:
        prefix = docker_compose_project_prefix(trial_name)
        try:
            listed = subprocess.run(
                ["docker", "ps", "--format", "{{.Names}}"],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=10,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return []
        containers = [
            line.strip()
            for line in listed.stdout.splitlines()
            if line.strip().startswith(prefix)
        ]
        for container in containers:
            try:
                subprocess.run(
                    ["docker", "stop", container],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    text=True,
                    timeout=30,
                    check=False,
                )
            except (OSError, subprocess.TimeoutExpired):
                continue
        return containers


def docker_compose_project_prefix(trial_name: str) -> str:
    return trial_name.lower().replace(".", "-")


def comparable_label(args: argparse.Namespace, interventions: list[WatchdogIntervention] | None = None) -> str:
    if args.override_storage_mb:
        return "no"
    if getattr(args, "trial_hard_timeout_sec", 0):
        return "no"
    if float(getattr(args, "heavy_verifier_timeout_multiplier", 0) or 0) > 1:
        return "no"
    if getattr(args, "verifier_uv_http_timeout_sec", None):
        return "no"
    if getattr(args, "verifier_uv_torch_backend", None):
        return "no"
    if getattr(args, "verifier_proxy", None):
        return "no"
    if interventions:
        return "no"
    return "yes"


def heavy_verifier_timeout_overrides(args: argparse.Namespace) -> dict[str, int]:
    if not getattr(args, "trial_hard_timeout_sec", 0):
        return {}
    timeout_sec = int(getattr(args, "heavy_verifier_hard_timeout_sec", 0) or 0)
    if timeout_sec <= 0:
        return {}
    return {
        prefix: max(timeout_sec, int(args.trial_hard_timeout_sec))
        for prefix in HEAVY_VERIFIER_TRIAL_PREFIXES
    }


def subset_has_heavy_verifier_task(subset: dict) -> bool:
    for item in subset.get("tasks", []):
        task_name = str(item.get("name") or "").strip()
        short_name = task_name.rsplit("/", 1)[-1]
        if short_name in HEAVY_VERIFIER_TRIAL_PREFIXES:
            return True
    return False


def heavy_verifier_timeout_multiplier(
    args: argparse.Namespace, subset: dict
) -> str | None:
    if not subset_has_heavy_verifier_task(subset):
        return None
    multiplier = float(getattr(args, "heavy_verifier_timeout_multiplier", 0) or 0)
    if multiplier <= 1:
        return None
    return f"{multiplier:g}"


def format_timeout_overrides(overrides: dict[str, int]) -> str:
    if not overrides:
        return "<none>"
    return ", ".join(f"{name}:{timeout}" for name, timeout in sorted(overrides.items()))


def task_host_timeout_caps(args: argparse.Namespace, subset: dict) -> dict[str, int]:
    trial_timeout_sec = int(getattr(args, "trial_hard_timeout_sec", 0) or 0)
    if trial_timeout_sec <= 0:
        return {}

    trial_overrides = heavy_verifier_timeout_overrides(args)
    caps: dict[str, int] = {}
    for item in subset.get("tasks", []):
        task_name = str(item.get("name") or "").strip()
        if not task_name:
            continue
        short_name = task_name.rsplit("/", 1)[-1]
        hard_timeout_sec = trial_overrides.get(short_name, trial_timeout_sec)
        if hard_timeout_sec <= HOST_DEADLINE_RESERVE_SEC + MIN_AGENT_RUNTIME_SEC:
            raise SystemExit(
                f"trial hard timeout for {task_name} is too short: "
                f"{hard_timeout_sec}s must exceed the {HOST_DEADLINE_RESERVE_SEC}s "
                f"host reserve plus {MIN_AGENT_RUNTIME_SEC}s minimum Agent window"
            )
        host_cap_sec = hard_timeout_sec - HOST_DEADLINE_RESERVE_SEC
        caps[task_name] = host_cap_sec
    return caps


def comparability_notes(
    args: argparse.Namespace,
    interventions: list[WatchdogIntervention] | None = None,
) -> list[str]:
    notes: list[str] = []
    if args.override_storage_mb:
        notes.append("explicit Harbor storage override was used")
    if getattr(args, "trial_hard_timeout_sec", 0):
        notes.append("runner-level trial hard timeout watchdog was enabled")
    if float(getattr(args, "heavy_verifier_timeout_multiplier", 0) or 0) > 1:
        notes.append("Harbor verifier timeout multiplier was modified")
    if (
        getattr(args, "verifier_uv_http_timeout_sec", None)
        or getattr(args, "verifier_uv_torch_backend", None)
        or getattr(args, "verifier_proxy", None)
    ):
        notes.append("Harbor verifier runtime environment was modified")
    if interventions:
        notes.append("watchdog stopped one or more stale trial containers")
    return notes


def load_subset(path: Path) -> dict:
    data = json.loads(path.read_text())
    tasks = data.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise SystemExit(f"subset file has no tasks: {path}")
    for index, item in enumerate(tasks):
        if not isinstance(item, dict) or not str(item.get("name") or "").strip():
            raise SystemExit(f"subset task #{index + 1} is missing a name")
    return data


def build_env(args: argparse.Namespace, subset: dict) -> dict[str, str]:
    tasks = [str(item["name"]).strip() for item in subset["tasks"]]
    env = os.environ.copy()
    env.pop("CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC", None)
    env.pop("CODEFACTORY_BENCH_TASK_HOST_TIMEOUTS_JSON", None)
    env.pop("CODEFACTORY_BENCH_TASK_AGENT_TIMEOUTS_JSON", None)
    current_pythonpath = env.get("PYTHONPATH")
    env.update(
        {
            "PYTHONPATH": (
                f"{REPO_ROOT}{os.pathsep}{current_pythonpath}"
                if current_pythonpath
                else str(REPO_ROOT)
            ),
            "CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE": "1",
            "CODEFACTORY_BENCH_ENDPOINT": args.endpoint,
            "CODEFACTORY_BENCH_TASK_NAMES": ",".join(tasks),
            "CODEFACTORY_BENCH_TASK_LIMIT": str(len(tasks)),
            "CODEFACTORY_BENCH_CONCURRENCY": str(args.concurrency),
            "CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC": str(args.model_timeout_sec),
            "CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC": str(args.shell_timeout_sec),
            "CODEFACTORY_BENCH_SECRET_TIMEOUT_SEC": str(args.secret_timeout_sec),
            "CODEFACTORY_BENCH_JOB_ROOT": str(REPO_ROOT / ".codefactory/benchmark-jobs"),
            "CODEFACTORY_BENCH_ALLOW_PARTIAL_IMPORT": "1",
        }
    )
    task_agent_timeouts = {
        str(item["name"]).strip(): int(item["agent_timeout_sec"])
        for item in subset["tasks"]
        if isinstance(item.get("agent_timeout_sec"), (int, float))
        and item["agent_timeout_sec"] > 0
    }
    if task_agent_timeouts:
        env["CODEFACTORY_BENCH_TASK_AGENT_TIMEOUTS_JSON"] = json.dumps(
            task_agent_timeouts, sort_keys=True, separators=(",", ":")
        )
    host_timeout_caps = task_host_timeout_caps(args, subset)
    if host_timeout_caps:
        env["CODEFACTORY_BENCH_TASK_HOST_TIMEOUTS_JSON"] = json.dumps(
            host_timeout_caps, sort_keys=True, separators=(",", ":")
        )
    if args.agent_wall_timeout_sec > 0:
        env["CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC"] = str(
            args.agent_wall_timeout_sec
        )
    if args.verifier_uv_http_timeout_sec:
        env["CODEFACTORY_BENCH_VERIFIER_UV_HTTP_TIMEOUT_SEC"] = str(
            args.verifier_uv_http_timeout_sec
        )
    if args.verifier_uv_torch_backend:
        env["CODEFACTORY_BENCH_VERIFIER_UV_TORCH_BACKEND"] = str(
            args.verifier_uv_torch_backend
        )
    verifier_timeout_multiplier = heavy_verifier_timeout_multiplier(args, subset)
    if verifier_timeout_multiplier:
        env["CODEFACTORY_BENCH_VERIFIER_TIMEOUT_MULTIPLIER"] = (
            verifier_timeout_multiplier
        )
    if args.model:
        env["CODEFACTORY_BENCH_MODEL_OVERRIDE"] = args.model
    if args.override_storage_mb:
        env["CODEFACTORY_BENCH_OVERRIDE_STORAGE_MB"] = str(args.override_storage_mb)
    if args.docker_apt_proxy:
        env["CODEFACTORY_BENCH_DOCKER_APT_PROXY"] = args.docker_apt_proxy
    if args.verifier_proxy:
        env["CODEFACTORY_BENCH_VERIFIER_PROXY"] = args.verifier_proxy
    if args.provider_proxy:
        env["HTTP_PROXY"] = args.provider_proxy
        env["HTTPS_PROXY"] = args.provider_proxy
        env["ALL_PROXY"] = args.provider_proxy
        env["http_proxy"] = args.provider_proxy
        env["https_proxy"] = args.provider_proxy
        env["all_proxy"] = args.provider_proxy
        env["NO_PROXY"] = LOOPBACK_NO_PROXY
        env["no_proxy"] = LOOPBACK_NO_PROXY
    return env


def cargo_command() -> list[str]:
    return [
        "cargo",
        "test",
        TEST_NAME,
        "--lib",
        "--",
        "--ignored",
        "--nocapture",
    ]


def safe_plan(
    args: argparse.Namespace, subset_path: Path, subset: dict, env: dict[str, str]
) -> str:
    tasks = [str(item["name"]).strip() for item in subset["tasks"]]
    explicit_key = "yes" if env.get("CODEFACTORY_BENCH_API_KEY") else "no"
    model = args.model or "<settings default>"
    return "\n".join(
        [
            "# Terminal-Bench 2.1 regression subset run plan",
            "",
            f"- subset: `{subset.get('id', SUBSET_PATH.stem)}`",
            f"- subset path: `{subset_path}`",
            f"- tasks: `{len(tasks)}`",
            f"- endpoint: `{args.endpoint}`",
            f"- model: `{model}`",
            f"- concurrency: `{args.concurrency}`",
            f"- min_docker_cpus: `{args.min_docker_cpus}`",
            f"- min_docker_memory_gb: `{args.min_docker_memory_gb}`",
            f"- min_docker_free_gb: `{args.min_docker_free_gb}`",
            f"- resource_preflight: `{'skipped' if args.skip_resource_preflight else 'enabled'}`",
            "- bind_mount_preflight: `enabled`",
            f"- preflight_retries: `{args.preflight_retries}`",
            f"- agent_binary: `{env.get('CODEFACTORY_BENCH_AGENT_BINARY') or '<build from current source before launch>'}`",
            f"- agent_build_timeout_sec: `{args.agent_build_timeout_sec}`",
            f"- override_storage_mb: `{args.override_storage_mb or '<none>'}`",
            f"- official_comparable: `{comparable_label(args)}`",
            f"- explicit CODEFACTORY_BENCH_API_KEY present: `{explicit_key}`",
            f"- keychain timeout: `{args.secret_timeout_sec}s`",
            f"- trial_hard_timeout_sec: `{args.trial_hard_timeout_sec or '<disabled>'}`",
            f"- task_host_timeout_caps: `{format_timeout_overrides(task_host_timeout_caps(args, subset))}`",
            f"- heavy_verifier_timeout_overrides: `{format_timeout_overrides(heavy_verifier_timeout_overrides(args))}`",
            f"- heavy_verifier_timeout_multiplier: `{heavy_verifier_timeout_multiplier(args, subset) or '<none>'}`",
            f"- docker_apt_proxy: `{args.docker_apt_proxy or '<none>'}`",
            f"- verifier_proxy: `{args.verifier_proxy or '<none>'}`",
            f"- provider_proxy: `{args.provider_proxy or '<none>'}`",
            f"- provider_bridge_retries: `{args.provider_bridge_retries}`",
            f"- verifier_uv_http_timeout_sec: `{args.verifier_uv_http_timeout_sec or '<none>'}`",
            f"- verifier_uv_torch_backend: `{args.verifier_uv_torch_backend or '<none>'}`",
            "- partial_import_diagnostic: `enabled`",
            f"- job root: `{env['CODEFACTORY_BENCH_JOB_ROOT']}`",
            f"- agent PYTHONPATH root: `{REPO_ROOT}`",
            "- command: `cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`",
            "",
            "Tasks:",
            *[f"- `{task}`" for task in tasks],
        ]
    )


def run_capture(command: list[str], timeout: int) -> CapturedCommand:
    try:
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
        return CapturedCommand(completed.returncode, completed.stdout)
    except subprocess.TimeoutExpired as exc:
        output = exc.stdout or ""
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
        return CapturedCommand(124, output + f"\npreflight command timed out after {timeout}s")


def prepare_agent_binary(
    env: dict[str, str], timeout_sec: int
) -> PreflightResult:
    explicit_binary = env.get("CODEFACTORY_BENCH_AGENT_BINARY", "").strip()
    if explicit_binary:
        binary = Path(explicit_binary).expanduser().resolve()
        if not binary.is_file() or not os.access(binary, os.X_OK):
            return PreflightResult(
                False,
                [
                    "The explicit CODEFACTORY_BENCH_AGENT_BINARY is missing or not executable."
                ],
                [f"explicit agent binary: {binary}"],
            )
        env["CODEFACTORY_BENCH_AGENT_BINARY"] = str(binary)
        return PreflightResult(
            True,
            [],
            [
                f"agent binary source: explicit ({binary})",
                f"agent binary sha256: {sha256_file(binary)}",
            ],
        )

    manifest = REPO_ROOT / "src-tauri/Cargo.toml"
    binary_name = (
        "codefactory-agent-headless.exe"
        if os.name == "nt"
        else "codefactory-agent-headless"
    )
    binary = REPO_ROOT / "src-tauri/target/debug" / binary_name
    if not manifest.is_file():
        return PreflightResult(
            False,
            ["Could not find the src-tauri Cargo workspace for the benchmark Agent build."],
            [f"expected manifest: {manifest}"],
        )

    command = [
        "cargo",
        "build",
        "--manifest-path",
        str(manifest),
        "-p",
        "codefactory-agent-headless",
    ]
    build = run_capture(command, timeout=timeout_sec)
    if build.returncode != 0:
        return PreflightResult(
            False,
            ["Building codefactory-agent-headless from the current source failed."],
            [
                f"command: {shlex.join(command)}",
                f"exit_code: {build.returncode}",
                tail(build.output, 8000),
            ],
        )
    if not binary.is_file() or not os.access(binary, os.X_OK):
        return PreflightResult(
            False,
            [
                "Building codefactory-agent-headless completed but did not produce an executable binary."
            ],
            [
                f"expected binary: {binary}",
                tail(build.output, 8000),
            ],
        )

    binary = binary.resolve()
    env["CODEFACTORY_BENCH_AGENT_BINARY"] = str(binary)
    return PreflightResult(
        True,
        [],
        [
            f"agent binary source: built from current source ({binary})",
            f"agent binary sha256: {sha256_file(binary)}",
        ],
    )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_bind_mount_preflight(timeout_sec: int) -> PreflightResult:
    probe_dir = REPO_ROOT / ".codefactory/benchmark-preflight"
    host_marker = probe_dir / "host-to-container.txt"
    container_marker = probe_dir / "container-to-host.txt"
    probe_dir.mkdir(parents=True, exist_ok=True)
    host_marker.write_text(BIND_MOUNT_PROBE_TOKEN)
    container_marker.unlink(missing_ok=True)
    container_path = "/codefactory-probe"
    script = (
        f"test \"$(cat {container_path}/{host_marker.name})\" = "
        f"{shlex.quote(BIND_MOUNT_PROBE_TOKEN)} && "
        f"printf %s {shlex.quote(BIND_MOUNT_PROBE_TOKEN)} >"
        f"{container_path}/{container_marker.name} && "
        "printf 'host marker readable\\n'"
    )
    command = [
        "docker",
        "run",
        "--rm",
        "-v",
        f"{probe_dir.resolve()}:{container_path}:rw",
        "ubuntu:24.04",
        "sh",
        "-lc",
        script,
    ]
    try:
        captured = run_capture(command, timeout=timeout_sec)
        if captured.returncode != 0:
            return PreflightResult(
                False,
                [
                    "Docker could not read the benchmark host directory through its bind mount."
                ],
                [
                    f"probe directory: {probe_dir.resolve()}",
                    tail(captured.output, 4000),
                ],
            )
        try:
            container_value = container_marker.read_text()
        except OSError:
            container_value = ""
        if container_value != BIND_MOUNT_PROBE_TOKEN:
            return PreflightResult(
                False,
                [
                    "Docker container-to-host bind mount writes are not visible; refusing to launch Harbor."
                ],
                [
                    f"probe directory: {probe_dir.resolve()}",
                    "Use a Docker-shared persistent project path such as /Users/<user>/Projects, not this checkout path.",
                    tail(captured.output, 4000),
                ],
            )
        return PreflightResult(
            True,
            [],
            [f"Docker bind mount is bidirectional: {probe_dir.resolve()}"],
        )
    finally:
        host_marker.unlink(missing_ok=True)
        container_marker.unlink(missing_ok=True)
        try:
            probe_dir.rmdir()
        except OSError:
            pass


def run_preflight(args: argparse.Namespace) -> PreflightResult:
    if args.skip_resource_preflight:
        return PreflightResult(True, [], ["resource preflight skipped by operator"])

    blockers: list[str] = []
    details: list[str] = []
    docker_info = run_capture(["docker", "info", "--format", "{{json .}}"], timeout=30)
    if docker_info.returncode != 0:
        return PreflightResult(
            False,
            ["Docker must be running before launching provider-backed Terminal-Bench runs."],
            [tail(docker_info.output, 4000)],
        )

    try:
        info = json.loads(docker_info.output)
    except json.JSONDecodeError:
        return PreflightResult(
            False,
            ["Could not parse `docker info`; refusing to launch an unclassified benchmark run."],
            [tail(docker_info.output, 4000)],
        )

    cpus = float(info.get("NCPU") or 0)
    memory_gb = float(info.get("MemTotal") or 0) / (1024**3)
    details.append(f"docker cpus: {cpus:.2f}")
    details.append(f"docker memory_gb: {memory_gb:.2f}")
    if cpus < args.min_docker_cpus:
        blockers.append(
            f"Docker reports {cpus:.2f} CPUs; require at least {args.min_docker_cpus:.2f} for this subset/concurrency."
        )
    if memory_gb < args.min_docker_memory_gb:
        blockers.append(
            f"Docker reports {memory_gb:.2f} GiB memory; require at least {args.min_docker_memory_gb:.2f} GiB."
        )

    free_values: list[float] = []
    apt_proxy_setup = ""
    if args.docker_apt_proxy:
        quoted_proxy = shlex.quote(args.docker_apt_proxy)
        apt_proxy_setup = (
            f"APT_PROXY={quoted_proxy}; "
            "printf 'Acquire::http::Proxy \"%s\";\\nAcquire::https::Proxy \"%s\";\\n' "
            '"$APT_PROXY" "$APT_PROXY" >/etc/apt/apt.conf.d/99codefactory-proxy && '
        )
        details.append(f"docker apt proxy: {args.docker_apt_proxy}")
    smoke_script = (
        apt_proxy_setup
        +
        "df -Pk / | awk 'NR==2 { printf \"root_free_gb=%.2f\\n\", $4 / 1024 / 1024 }' && "
        "apt-get update -qq && "
        "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends curl ca-certificates && "
        "curl --version >/dev/null"
    )
    preflight_attempts = max(1, int(getattr(args, "preflight_retries", 1)) + 1)
    for label, image in BOOTSTRAP_SMOKE_IMAGES:
        smoke = CapturedCommand(1, "")
        for attempt in range(1, preflight_attempts + 1):
            smoke = run_capture(
                ["docker", "run", "--rm", image, "sh", "-lc", smoke_script],
                timeout=args.preflight_timeout_sec,
            )
            details.append(
                f"bootstrap smoke image: {label} ({image}) attempt {attempt}/{preflight_attempts}"
            )
            details.append(tail(smoke.output, 4000))
            if smoke.returncode == 0:
                break
            if attempt < preflight_attempts:
                details.append(
                    f"bootstrap smoke retrying {label} after return_code={smoke.returncode}"
                )
        if smoke.returncode != 0:
            blockers.append(
                f"Docker verifier bootstrap smoke failed for {label}; apt/curl dependency setup may be misclassified as agent failure."
            )
        free_match = re.search(r"root_free_gb=(?P<free>[0-9.]+)", smoke.output)
        if free_match:
            free_values.append(float(free_match.group("free")))

    if free_values:
        free_gb = min(free_values)
        if free_gb < args.min_docker_free_gb:
            blockers.append(
                f"Docker root filesystem has {free_gb:.2f} GiB free; require at least {args.min_docker_free_gb:.2f} GiB."
            )
    else:
        blockers.append("Could not measure Docker root filesystem free space.")

    return PreflightResult(not blockers, blockers, details)


def run_command(
    env: dict[str, str],
    watchdog: BenchmarkWatchdog | None = None,
) -> tuple[int, str, list[WatchdogIntervention]]:
    command = cargo_command()
    process = subprocess.Popen(
        command,
        cwd=REPO_ROOT / "src-tauri",
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    output: list[str] = []
    assert process.stdout is not None
    if watchdog:
        watchdog.start()
    try:
        for line in process.stdout:
            if watchdog:
                watchdog.observe_output_line(line)
            output.append(line)
            print(line, end="")
    finally:
        if watchdog:
            watchdog.stop()
    return process.wait(), "".join(output), watchdog.interventions() if watchdog else []


def parse_output(output: str) -> dict[str, object]:
    result_match = re.search(
        r"provider_bridge_result status=(?P<status>\S+) exit_code=(?P<exit_code>.*?) "
        r"job_path=(?P<job_path>\S+)",
        output,
    )
    preview_match = re.search(
        r"provider_bridge_preview .*?model=(?P<model>\S+) .*?task_limit=(?P<task_limit>\d+) "
        r"concurrency=(?P<concurrency>\d+) .*?override_storage_mb=(?P<override_storage_mb>\S+) "
        r"job_path=(?P<job_path>\S+)",
        output,
    )
    imported_match = re.search(
        r"provider_bridge_imported run=(?P<run>\S+) dataset=(?P<dataset>\S+) "
        r"agent=(?P<agent>\S+) model=(?P<model>\S+) comparable=(?P<comparable>\S+) "
        r"trials=(?P<trials>\d+) mean_reward=(?P<mean_reward>[0-9.]+)",
        output,
    )
    trials = [
        {
            "task": match.group("task"),
            "reward": match.group("reward"),
            "failure_class": match.group("failure_class"),
        }
        for match in re.finditer(
            r"provider_bridge_trial task=(?P<task>\S+) reward=(?P<reward>[0-9.]+) "
            r"failure_class=(?P<failure_class>.+)",
            output,
        )
    ]
    timeout = "Benchmark provider secret lookup timed out" in output
    return {
        "result": result_match.groupdict() if result_match else None,
        "preview": preview_match.groupdict() if preview_match else None,
        "imported": imported_match.groupdict() if imported_match else None,
        "trials": trials,
        "no_partial_import": "provider_bridge_no_partial_import" in output,
        "credential_timeout": timeout,
    }


def is_transient_provider_failure(
    exit_code: int, output: str, parsed: dict[str, object]
) -> bool:
    if exit_code == 0 and is_transient_verifier_environment_failure(parsed):
        return True
    if exit_code == 0:
        return False
    lowered = output.lower()
    has_transient_marker = any(
        marker in lowered
        for marker in [
            "connecterror",
            "readerror",
            "remotedisconnected",
            "remote disconnected",
            "connection reset",
            "connection aborted",
            "temporarily unavailable",
            "network is unreachable",
            "failed to download",
            "failed to fetch",
            "tls handshake eof",
            "request failed after",
            "error sending request",
        ]
    )
    if not has_transient_marker:
        return False
    if parsed.get("no_partial_import"):
        return True
    provider_result = parsed.get("result") or {}
    if isinstance(provider_result, dict):
        return provider_result.get("status") != "completed"
    return False


def is_transient_verifier_environment_failure(parsed: dict[str, object]) -> bool:
    trials = parsed.get("trials") or []
    failed_tasks = [
        str(trial.get("task") or "").split("/")[-1]
        for trial in trials
        if isinstance(trial, dict) and str(trial.get("reward") or "") in {"0", "0.0"}
    ]
    if not failed_tasks:
        return False

    job_path = parsed_job_path(parsed)
    if not job_path:
        return False
    return has_transient_verifier_dependency_failure(job_path, failed_tasks)


def parsed_job_path(parsed: dict[str, object]) -> str | None:
    preview = parsed.get("preview")
    if isinstance(preview, dict) and preview.get("job_path"):
        return str(preview["job_path"])
    provider_result = parsed.get("result")
    if isinstance(provider_result, dict) and provider_result.get("job_path"):
        return str(provider_result["job_path"])
    return None


def has_transient_verifier_dependency_failure(
    job_path: str | Path,
    failed_tasks: list[str] | None = None,
) -> bool:
    root = Path(job_path)
    if not root.is_dir():
        return False
    failed_task_prefixes = {
        task.lower()
        for task in failed_tasks or []
        if task and task.strip()
    }
    transient_pattern = re.compile(
        r"Failed to download|"
        r"Failed to fetch|"
        r"Error reading from server\. Remote end closed connection|"
        r"Request failed after \d+ retries|"
        r"tls handshake eof|"
        r"error sending request for url|"
        r"UNEXPECTED_EOF_WHILE_READING|"
        r"RemoteDisconnected|"
        r"connection reset|"
        r"connection aborted|"
        r"network is unreachable|"
        r"temporarily unavailable|"
        r"Unable to locate package curl|"
        r"/root/\.local/bin/env: No such file or directory|"
        r"uvx: command not found|"
        r"curl: \((?:18|35|56)\)",
        re.IGNORECASE,
    )
    for stdout_path in sorted(root.glob("*/verifier/test-stdout.txt")):
        trial_name = stdout_path.parents[1].name.lower()
        if failed_task_prefixes and not any(
            trial_name.startswith(f"{task}__") for task in failed_task_prefixes
        ):
            continue
        try:
            text = stdout_path.read_text(errors="replace")
        except OSError:
            continue
        if transient_pattern.search(text):
            return True
    return False


def is_transient_no_partial_provider_failure(
    exit_code: int, output: str, parsed: dict[str, object]
) -> bool:
    return bool(parsed.get("no_partial_import")) and is_transient_provider_failure(
        exit_code, output, parsed
    )


def run_command_with_retries(
    args: argparse.Namespace,
    env: dict[str, str],
    subset: dict,
) -> tuple[int, str, dict[str, object], list[WatchdogIntervention]]:
    attempts = max(1, int(args.provider_bridge_retries) + 1)
    combined_outputs: list[str] = []
    all_interventions: list[WatchdogIntervention] = []
    last_exit_code = 1
    last_output = ""
    last_parsed: dict[str, object] = {}

    for attempt in range(1, attempts + 1):
        if attempt > 1:
            retry_line = (
                f"provider_bridge_retry attempt={attempt} max_attempts={attempts} "
                "reason=transient-provider-network-failure"
            )
            print(f"\n{retry_line}")
            combined_outputs.append(f"\n{retry_line}\n")
        watchdog = BenchmarkWatchdog(
            timeout_sec=args.trial_hard_timeout_sec,
            poll_interval_sec=args.watchdog_poll_interval_sec,
            trial_timeout_overrides=heavy_verifier_timeout_overrides(args),
        )
        exit_code, output, interventions = run_command(env, watchdog)
        parsed = parse_output(output)

        combined_outputs.append(
            f"\n# Provider bridge attempt {attempt}/{attempts}\n{output}"
        )
        all_interventions.extend(interventions)
        last_exit_code = exit_code
        last_output = output
        last_parsed = parsed
        if (
            exit_code == 0
            and parsed.get("result")
            and parsed["result"].get("status") != "completed"  # type: ignore[index,union-attr]
        ):
            last_exit_code = 1

        if not is_transient_provider_failure(last_exit_code, output, parsed):
            break
        if attempt == attempts:
            break

    combined_output = "".join(combined_outputs) or last_output
    return last_exit_code, combined_output, last_parsed, all_interventions


def write_preflight_blocker_report(
    args: argparse.Namespace,
    subset: dict,
    preflight: PreflightResult,
) -> Path:
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H-%M-%SZ")
    report_path = EVIDENCE_DIR / f"terminal-bench-21-regression-subset-{timestamp}.md"
    tasks = [str(item["name"]).strip() for item in subset["tasks"]]
    lines = [
        "# Terminal-Bench 2.1 Regression Subset Evidence",
        "",
        f"- generated_at: `{timestamp}`",
        f"- subset: `{subset.get('id', SUBSET_PATH.stem)}`",
        f"- source_run_id: `{subset.get('source_run_id', '')}`",
        f"- task_count: `{len(tasks)}`",
        f"- endpoint: `{args.endpoint}`",
        "- exit_code: `2`",
        f"- override_storage_mb: `{args.override_storage_mb or '<none>'}`",
        "- official_comparable: `no`",
        "- harbor_started: `no`",
        "- trials: `0`",
        f"- explicit_key_present: `{'yes' if os.environ.get('CODEFACTORY_BENCH_API_KEY') else 'no'}`",
        f"- heavy_verifier_timeout_overrides: `{format_timeout_overrides(heavy_verifier_timeout_overrides(args))}`",
        f"- verifier_uv_torch_backend: `{args.verifier_uv_torch_backend or '<none>'}`",
        "",
        "## Blocker",
        "",
        "The provider-backed benchmark was not launched because a required preflight failed.",
        "",
        "## Preflight Blockers",
        "",
        *[f"- {blocker}" for blocker in preflight.blockers],
        "",
        "## Preflight Details",
        "",
        "```text",
        "\n".join(preflight.details),
        "```",
        "",
    ]
    report_path.write_text("\n".join(lines))
    return report_path


def write_report(
    args: argparse.Namespace,
    subset: dict,
    exit_code: int,
    output: str,
    parsed: dict[str, object],
    interventions: list[WatchdogIntervention] | None = None,
    agent_preflight: PreflightResult | None = None,
) -> Path:
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H-%M-%SZ")
    report_path = EVIDENCE_DIR / f"terminal-bench-21-regression-subset-{timestamp}.md"
    tasks = [str(item["name"]).strip() for item in subset["tasks"]]
    imported = parsed.get("imported")
    preview = parsed.get("preview")
    provider_result = parsed.get("result")
    trials = parsed.get("trials") or []
    job_path = (
        preview["job_path"]
        if preview
        else provider_result["job_path"]
        if provider_result
        else None
    )
    partial_job = load_partial_job(job_path) if job_path else None
    agent_usage = load_agent_usage(job_path)
    agent_completion = load_agent_completion_evidence(job_path)
    verifier_warnings = detect_verifier_environment_warnings(job_path)
    interventions = interventions or []
    official_comparable = comparable_label(args, interventions)
    if exit_code != 0 or not imported:
        official_comparable = "no"
    if imported and str(imported.get("comparable", "")).lower() != "true":
        official_comparable = "no"
    provider_status = provider_result["status"] if provider_result else ""
    partial_imported_failed_run = bool(
        imported and (exit_code != 0 or provider_status not in ("", "completed"))
    )
    lines = [
        "# Terminal-Bench 2.1 Regression Subset Evidence",
        "",
        f"- generated_at: `{timestamp}`",
        f"- subset: `{subset.get('id', SUBSET_PATH.stem)}`",
        f"- source_run_id: `{subset.get('source_run_id', '')}`",
        f"- task_count: `{len(tasks)}`",
        f"- endpoint: `{args.endpoint}`",
        f"- exit_code: `{exit_code}`",
        f"- override_storage_mb: `{args.override_storage_mb or '<none>'}`",
        f"- official_comparable: `{official_comparable}`",
        f"- explicit_key_present: `{'yes' if os.environ.get('CODEFACTORY_BENCH_API_KEY') else 'no'}`",
        f"- trial_hard_timeout_sec: `{args.trial_hard_timeout_sec or '<disabled>'}`",
        f"- task_host_timeout_caps: `{format_timeout_overrides(task_host_timeout_caps(args, subset))}`",
        f"- heavy_verifier_timeout_overrides: `{format_timeout_overrides(heavy_verifier_timeout_overrides(args))}`",
        f"- heavy_verifier_timeout_multiplier: `{heavy_verifier_timeout_multiplier(args, subset) or '<none>'}`",
        f"- verifier_uv_torch_backend: `{args.verifier_uv_torch_backend or '<none>'}`",
        "- partial_import_diagnostic: `enabled`",
        "",
    ]
    if agent_preflight:
        lines.extend(
            [
                "## Agent Binary Preflight",
                "",
                *[f"- {detail}" for detail in agent_preflight.details],
                "",
            ]
        )
    notes = comparability_notes(args, interventions)
    if exit_code == 124:
        notes.append("benchmark process exceeded its outer wall timeout")
    elif exit_code != 0:
        notes.append("benchmark runner exited nonzero")
    if not imported:
        notes.append("no Harbor run was imported")
    if imported and str(imported.get("comparable", "")).lower() != "true":
        notes.append("imported Harbor run was marked non-comparable")
    if notes:
        lines.extend(
            [
                "## Comparability Notes",
                "",
                *[f"- {note}" for note in notes],
                "",
            ]
        )
    if preview:
        lines.extend(
            [
                "## Preview",
                "",
                f"- model: `{preview['model']}`",
                f"- task_limit: `{preview['task_limit']}`",
                f"- concurrency: `{preview['concurrency']}`",
                f"- override_storage_mb: `{preview['override_storage_mb']}`",
                f"- job_path: `{preview['job_path']}`",
                "",
            ]
        )
    if provider_result:
        lines.extend(
            [
                "## Provider Bridge",
                "",
                f"- status: `{provider_result['status']}`",
                f"- exit_code: `{provider_result['exit_code']}`",
                f"- job_path: `{provider_result['job_path']}`",
                "",
            ]
        )
    if agent_usage:
        lines.extend(
            [
                "## Agent Usage",
                "",
                f"- trials_with_metadata: `{agent_usage['trials_with_metadata']}`",
                f"- model_requests: `{agent_usage['model_requests']}`",
                f"- prompt_tokens: `{agent_usage['prompt_tokens']}`",
                f"- completion_tokens: `{agent_usage['completion_tokens']}`",
                f"- total_tokens: `{agent_usage['total_tokens']}`",
                f"- tool_calls: `{agent_usage['tool_calls']}`",
                "",
            ]
        )
    if agent_completion:
        lines.extend(
            [
                "## Agent Completion Evidence",
                "",
                f"- completed_trials: `{agent_completion['completed_trials']} / {agent_completion['trials_with_evidence']}`",
                f"- recorded_outcomes: `{agent_completion['recorded_outcomes']}`",
                f"- external_tool_requests: `{agent_completion['external_tool_requests']}`",
                f"- recorded_non_external_outcomes: `{agent_completion['recorded_non_external_outcomes']}`",
                f"- blockers: `{agent_completion['blockers'] or '<none>'}`",
                f"- final_stop_summaries: `{agent_completion['final_stop_summaries'] or '<none>'}`",
                "",
            ]
        )
    if imported:
        pass_count = sum(1 for item in trials if float(item["reward"]) > 0)
        lines.extend(
            [
                "## Result",
                "",
                f"- run: `{imported['run']}`",
                f"- dataset: `{imported['dataset']}`",
                f"- agent: `{imported['agent']}`",
                f"- model: `{imported['model']}`",
                f"- harbor_import_comparable: `{imported['comparable']}`",
                f"- trials: `{imported['trials']}`",
                f"- pass_count: `{pass_count}`",
                f"- mean_reward: `{imported['mean_reward']}`",
                "",
                "## Trials",
                "",
                "| Task | Reward | Failure class |",
                "| --- | ---: | --- |",
            ]
        )
        for item in trials:
            lines.append(
                f"| `{item['task']}` | `{item['reward']}` | `{item['failure_class']}` |"
            )
        lines.append("")
        if partial_imported_failed_run:
            lines.extend(
                [
                    "## Partial Import Note",
                    "",
                    "The provider bridge returned a non-zero exit code, but CodeFactory imported completed Harbor trials for diagnostic scoring and failure analysis.",
                    "",
                ]
            )
    elif parsed.get("credential_timeout"):
        lines.extend(
            [
                "## Blocker",
                "",
                "The run did not start Harbor because provider credential lookup timed out.",
                "Unlock or authorize the OS credential store, or launch with an explicit in-memory `CODEFACTORY_BENCH_API_KEY`.",
                "",
            ]
        )
    elif parsed.get("no_partial_import"):
        lines.extend(
            [
                "## Blocker",
                "",
                "The provider bridge failed before Harbor produced an importable partial job. No completed trial rows were available for scoring.",
                "",
            ]
        )
    else:
        lines.extend(
            [
                "## Result",
                "",
                "The provider bridge command did not import a completed Harbor job.",
                "",
            ]
        )
        if partial_job:
            lines.extend(partial_job)
    if interventions:
        lines.extend(
            [
                "## Watchdog Interventions",
                "",
                "The regression runner stopped stale trial containers so the remaining matrix could finish.",
                "",
                "| Trial | Elapsed sec | Action | Containers |",
                "| --- | ---: | --- | --- |",
            ]
        )
        for item in interventions:
            lines.append(
                f"| `{item.trial}` | `{item.elapsed_sec}` | `{item.action}` | `{', '.join(item.containers) or '<none>'}` |"
            )
        lines.append("")
    if verifier_warnings:
        lines.extend(
            [
                "## Verifier Environment Warnings",
                "",
                "These warnings do not change Harbor rewards, but they mark local verifier runtime conditions that can weaken score interpretation.",
                "",
                "| Trial | Category | Evidence |",
                "| --- | --- | --- |",
            ]
        )
        for item in verifier_warnings:
            lines.append(
                f"| `{item.trial}` | `{item.category}` | `{escape_table_cell(item.evidence)}` |"
            )
        lines.append("")
    lines.extend(
        [
            "## Output Tail",
            "",
            "```text",
            tail(output, 12000),
            "```",
            "",
        ]
    )
    report_path.write_text("\n".join(lines))
    return report_path


def load_agent_usage(job_path: str | Path | None) -> dict[str, int] | None:
    if not job_path:
        return None
    root = Path(job_path)
    if not root.is_dir():
        return None
    totals = {
        "trials_with_metadata": 0,
        "model_requests": 0,
        "prompt_tokens": 0,
        "completion_tokens": 0,
        "total_tokens": 0,
        "tool_calls": 0,
    }
    for metadata_path in sorted(root.glob("*/agent/run-metadata.json")):
        try:
            metadata = json.loads(metadata_path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if metadata.get("runtime_subject") != "rust-core":
            continue
        usage = metadata.get("usage")
        if not isinstance(usage, dict):
            usage = {}
        totals["trials_with_metadata"] += 1
        for field in (
            "model_requests",
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
        ):
            value = usage.get(field)
            if isinstance(value, int) and value >= 0:
                totals[field] += value
        tool_calls = metadata.get("tool_calls")
        if isinstance(tool_calls, int) and tool_calls >= 0:
            totals["tool_calls"] += tool_calls
    return totals if totals["trials_with_metadata"] else None


def load_agent_completion_evidence(
    job_path: str | Path | None,
) -> dict[str, object] | None:
    if not job_path:
        return None
    root = Path(job_path)
    if not root.is_dir():
        return None
    totals: dict[str, object] = {
        "trials_with_evidence": 0,
        "completed_trials": 0,
        "recorded_outcomes": 0,
        "external_tool_requests": 0,
        "recorded_non_external_outcomes": 0,
        "blockers": "",
        "final_stop_summaries": "",
    }
    blockers: list[str] = []
    final_summaries: list[str] = []
    for metadata_path in sorted(root.glob("*/agent/run-metadata.json")):
        try:
            metadata = json.loads(metadata_path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if metadata.get("runtime_subject") != "rust-core":
            continue
        evidence = metadata.get("completion_evidence")
        if not isinstance(evidence, dict):
            continue
        totals["trials_with_evidence"] = int(totals["trials_with_evidence"]) + 1
        if evidence.get("completed") is True:
            totals["completed_trials"] = int(totals["completed_trials"]) + 1
        outcome_count = evidence.get("outcome_count")
        if isinstance(outcome_count, int) and outcome_count >= 0:
            totals["recorded_outcomes"] = int(totals["recorded_outcomes"]) + outcome_count
        tool_calls = metadata.get("tool_calls")
        if isinstance(tool_calls, int) and tool_calls >= 0:
            totals["external_tool_requests"] = int(totals["external_tool_requests"]) + tool_calls
        raw_blockers = evidence.get("blockers")
        if isinstance(raw_blockers, list):
            for blocker in raw_blockers:
                if isinstance(blocker, str) and blocker.strip() and blocker not in blockers:
                    blockers.append(blocker.strip())
        final_path = metadata_path.parent / "final.txt"
        try:
            final_text = " ".join(final_path.read_text().split())
        except OSError:
            final_text = ""
        if final_text:
            final_summaries.append(final_text[:500])
    trials_with_evidence = int(totals["trials_with_evidence"])
    if not trials_with_evidence:
        return None
    totals["recorded_non_external_outcomes"] = max(
        int(totals["recorded_outcomes"]) - int(totals["external_tool_requests"]),
        0,
    )
    totals["blockers"] = "; ".join(blockers).replace("`", "'")
    totals["final_stop_summaries"] = " | ".join(final_summaries).replace("`", "'")
    return totals


def load_partial_job(job_path: str) -> list[str] | None:
    root = Path(job_path)
    run_result_path = root / "result.json"
    if not run_result_path.is_file():
        return None
    try:
        run_result = json.loads(run_result_path.read_text())
    except (OSError, json.JSONDecodeError):
        return None

    stats = run_result.get("stats") if isinstance(run_result, dict) else None
    if not isinstance(stats, dict):
        stats = {}
    lines = [
        "## Partial Harbor State",
        "",
        f"- run: `{run_result.get('id', '')}`",
        f"- finished_at: `{run_result.get('finished_at')}`",
        f"- completed_trials: `{stats.get('n_completed_trials')}`",
        f"- errored_trials: `{stats.get('n_errored_trials')}`",
        f"- running_trials: `{stats.get('n_running_trials')}`",
        f"- pending_trials: `{stats.get('n_pending_trials')}`",
        f"- cancelled_trials: `{stats.get('n_cancelled_trials')}`",
        "",
        "## Partial Trials",
        "",
        "| Trial | Task | Reward | Exception | Tool calls |",
        "| --- | --- | ---: | --- | ---: |",
    ]

    for trial_path in sorted(root.glob("*/result.json")):
        try:
            trial = json.loads(trial_path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        reward = (
            (trial.get("verifier_result") or {})
            .get("rewards", {})
            .get("reward", "")
        )
        exception = (trial.get("exception_info") or {}).get("exception_type") or ""
        metadata = (trial.get("agent_result") or {}).get("metadata") or {}
        tool_calls = metadata.get("tool_calls", "")
        lines.append(
            "| `{trial}` | `{task}` | `{reward}` | `{exception}` | `{tool_calls}` |".format(
                trial=trial_path.parent.name,
                task=trial.get("task_name", ""),
                reward=reward,
                exception=exception,
                tool_calls=tool_calls,
            )
        )
    lines.append("")
    return lines


def detect_verifier_environment_warnings(
    job_path: str | Path | None,
) -> list[VerifierEnvironmentWarning]:
    if not job_path:
        return []
    root = Path(job_path)
    if not root.is_dir():
        return []

    warnings: list[VerifierEnvironmentWarning] = []
    for stdout_path in sorted(root.glob("*/verifier/test-stdout.txt")):
        trial = stdout_path.parents[1].name
        try:
            text = stdout_path.read_text(errors="replace")
        except OSError:
            continue
        for category, pattern in VERIFIER_ENVIRONMENT_WARNING_PATTERNS:
            evidence = first_matching_line(text, pattern)
            if evidence:
                warnings.append(
                    VerifierEnvironmentWarning(
                        trial=trial,
                        category=category,
                        evidence=evidence,
                    )
                )
    return warnings


def first_matching_line(text: str, pattern: re.Pattern[str]) -> str | None:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped and pattern.search(stripped):
            return stripped[:240]
    return None


def escape_table_cell(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def tail(value: str, limit: int) -> str:
    return value if len(value) <= limit else value[-limit:]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run the fixed Terminal-Bench 2.1 regression subset through CodeFactory provider bridge."
    )
    parser.add_argument("--subset", type=Path, default=SUBSET_PATH)
    parser.add_argument("--endpoint", default="deepseek")
    parser.add_argument("--model")
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--model-timeout-sec", type=int, default=120)
    parser.add_argument("--shell-timeout-sec", type=int, default=300)
    parser.add_argument(
        "--agent-wall-timeout-sec",
        type=int,
        default=0,
        help="Optional CodeFactory sidecar wall timeout. Default 0 delegates to Harbor.",
    )
    parser.add_argument(
        "--trial-hard-timeout-sec",
        type=int,
        default=DEFAULT_TRIAL_HARD_TIMEOUT_SEC,
    )
    parser.add_argument(
        "--heavy-verifier-hard-timeout-sec",
        type=int,
        default=DEFAULT_HEAVY_VERIFIER_HARD_TIMEOUT_SEC,
        help=(
            "Per-trial watchdog timeout for known heavy verifiers such as "
            "torch-tensor-parallelism. Set to 0 to disable overrides."
        ),
    )
    parser.add_argument(
        "--heavy-verifier-timeout-multiplier",
        type=float,
        default=float(
            os.environ.get(
                "CODEFACTORY_BENCH_HEAVY_VERIFIER_TIMEOUT_MULTIPLIER",
                str(DEFAULT_HEAVY_VERIFIER_TIMEOUT_MULTIPLIER),
            )
        ),
        help=(
            "Harbor verifier timeout multiplier applied only when the subset "
            "contains a known heavy verifier task. Set to 1 to disable."
        ),
    )
    parser.add_argument("--watchdog-poll-interval-sec", type=int, default=15)
    parser.add_argument("--secret-timeout-sec", type=int, default=20)
    parser.add_argument("--min-docker-cpus", type=float, default=4.0)
    parser.add_argument("--min-docker-memory-gb", type=float, default=6.0)
    parser.add_argument("--min-docker-free-gb", type=float, default=20.0)
    parser.add_argument("--preflight-timeout-sec", type=int, default=120)
    parser.add_argument("--preflight-retries", type=int, default=1)
    parser.add_argument(
        "--agent-build-timeout-sec",
        type=int,
        default=900,
        help="Maximum time to build the current-source Rust headless Agent before Harbor starts.",
    )
    parser.add_argument("--skip-resource-preflight", action="store_true")
    parser.add_argument(
        "--docker-apt-proxy",
        default=os.environ.get("CODEFACTORY_BENCH_DOCKER_APT_PROXY"),
    )
    parser.add_argument(
        "--verifier-proxy",
        default=os.environ.get("CODEFACTORY_BENCH_VERIFIER_PROXY"),
    )
    parser.add_argument("--provider-bridge-retries", type=int, default=2)
    parser.add_argument(
        "--provider-proxy",
        default=os.environ.get("CODEFACTORY_BENCH_PROVIDER_PROXY"),
    )
    parser.add_argument(
        "--verifier-uv-http-timeout-sec",
        type=int,
        default=(
            int(os.environ["CODEFACTORY_BENCH_VERIFIER_UV_HTTP_TIMEOUT_SEC"])
            if os.environ.get("CODEFACTORY_BENCH_VERIFIER_UV_HTTP_TIMEOUT_SEC")
            else None
        ),
        help=(
            "UV_HTTP_TIMEOUT value passed through the provider bridge to Harbor "
            "verifiers for large dependency downloads."
        ),
    )
    parser.add_argument(
        "--verifier-uv-torch-backend",
        default=os.environ.get("CODEFACTORY_BENCH_VERIFIER_UV_TORCH_BACKEND"),
        help=(
            "UV_TORCH_BACKEND value passed to Harbor verifiers. The default cpu "
            "avoids pulling CUDA wheels in local Mac/QEMU diagnostic runs."
        ),
    )
    parser.add_argument("--override-storage-mb", type=int)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    subset = load_subset(args.subset)
    env = build_env(args, subset)
    print(safe_plan(args, args.subset, subset, env))
    if args.dry_run:
        return 0
    preflight = run_preflight(args)
    if not preflight.ok:
        report_path = write_preflight_blocker_report(args, subset, preflight)
        print("\nResource preflight failed:")
        for blocker in preflight.blockers:
            print(f"- {blocker}")
        print(f"\nEvidence report: {report_path}")
        return 2
    print("\nVerifying bidirectional Docker bind mounts...", flush=True)
    bind_mount_preflight = run_bind_mount_preflight(args.preflight_timeout_sec)
    if not bind_mount_preflight.ok:
        report_path = write_preflight_blocker_report(
            args, subset, bind_mount_preflight
        )
        print("\nDocker bind mount preflight failed:")
        for blocker in bind_mount_preflight.blockers:
            print(f"- {blocker}")
        print(f"\nEvidence report: {report_path}")
        return 2
    for detail in bind_mount_preflight.details:
        print(f"- {detail}")
    print("\nPreparing current-source CodeFactory headless Agent...", flush=True)
    agent_preflight = prepare_agent_binary(env, args.agent_build_timeout_sec)
    if not agent_preflight.ok:
        report_path = write_preflight_blocker_report(args, subset, agent_preflight)
        print("\nAgent binary preflight failed:")
        for blocker in agent_preflight.blockers:
            print(f"- {blocker}")
        print(f"\nEvidence report: {report_path}")
        return 2
    for detail in agent_preflight.details:
        print(f"- {detail}")
    exit_code, output, parsed, interventions = run_command_with_retries(
        args, env, subset
    )
    report_path = write_report(
        args,
        subset,
        exit_code,
        output,
        parsed,
        interventions,
        agent_preflight,
    )
    print(f"\nEvidence report: {report_path}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
