#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SUBSET_PATH = REPO_ROOT / "docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json"
EVIDENCE_DIR = REPO_ROOT / "docs/evidence-packs"
TEST_NAME = "benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings"


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
    env.update(
        {
            "CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE": "1",
            "CODEFACTORY_BENCH_ENDPOINT": args.endpoint,
            "CODEFACTORY_BENCH_TASK_NAMES": ",".join(tasks),
            "CODEFACTORY_BENCH_TASK_LIMIT": str(len(tasks)),
            "CODEFACTORY_BENCH_CONCURRENCY": str(args.concurrency),
            "CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC": str(args.model_timeout_sec),
            "CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC": str(args.shell_timeout_sec),
            "CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC": str(args.agent_wall_timeout_sec),
            "CODEFACTORY_BENCH_SECRET_TIMEOUT_SEC": str(args.secret_timeout_sec),
            "CODEFACTORY_BENCH_JOB_ROOT": str(REPO_ROOT / ".codefactory/benchmark-jobs"),
        }
    )
    if args.model:
        env["CODEFACTORY_BENCH_MODEL_OVERRIDE"] = args.model
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
            f"- explicit CODEFACTORY_BENCH_API_KEY present: `{explicit_key}`",
            f"- keychain timeout: `{args.secret_timeout_sec}s`",
            f"- job root: `{env['CODEFACTORY_BENCH_JOB_ROOT']}`",
            "- command: `cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`",
            "",
            "Tasks:",
            *[f"- `{task}`" for task in tasks],
        ]
    )


def run_command(env: dict[str, str]) -> tuple[int, str]:
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
    for line in process.stdout:
        output.append(line)
        print(line, end="")
    return process.wait(), "".join(output)


def parse_output(output: str) -> dict[str, object]:
    preview_match = re.search(
        r"provider_bridge_preview .*?model=(?P<model>\S+) .*?task_limit=(?P<task_limit>\d+) "
        r"concurrency=(?P<concurrency>\d+) .*?job_path=(?P<job_path>\S+)",
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
        "preview": preview_match.groupdict() if preview_match else None,
        "imported": imported_match.groupdict() if imported_match else None,
        "trials": trials,
        "credential_timeout": timeout,
    }


def write_report(
    args: argparse.Namespace,
    subset: dict,
    exit_code: int,
    output: str,
    parsed: dict[str, object],
) -> Path:
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H-%M-%SZ")
    report_path = EVIDENCE_DIR / f"terminal-bench-21-regression-subset-{timestamp}.md"
    tasks = [str(item["name"]).strip() for item in subset["tasks"]]
    imported = parsed.get("imported")
    preview = parsed.get("preview")
    trials = parsed.get("trials") or []
    lines = [
        "# Terminal-Bench 2.1 Regression Subset Evidence",
        "",
        f"- generated_at: `{timestamp}`",
        f"- subset: `{subset.get('id', SUBSET_PATH.stem)}`",
        f"- source_run_id: `{subset.get('source_run_id', '')}`",
        f"- task_count: `{len(tasks)}`",
        f"- endpoint: `{args.endpoint}`",
        f"- exit_code: `{exit_code}`",
        f"- explicit_key_present: `{'yes' if os.environ.get('CODEFACTORY_BENCH_API_KEY') else 'no'}`",
        "",
    ]
    if preview:
        lines.extend(
            [
                "## Preview",
                "",
                f"- model: `{preview['model']}`",
                f"- task_limit: `{preview['task_limit']}`",
                f"- concurrency: `{preview['concurrency']}`",
                f"- job_path: `{preview['job_path']}`",
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
                f"- comparable: `{imported['comparable']}`",
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
    else:
        lines.extend(
            [
                "## Result",
                "",
                "The provider bridge command did not import a completed Harbor job.",
                "",
            ]
        )
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
    parser.add_argument("--shell-timeout-sec", type=int, default=120)
    parser.add_argument("--agent-wall-timeout-sec", type=int, default=780)
    parser.add_argument("--secret-timeout-sec", type=int, default=20)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    subset = load_subset(args.subset)
    env = build_env(args, subset)
    print(safe_plan(args, args.subset, subset, env))
    if args.dry_run:
        return 0
    exit_code, output = run_command(env)
    parsed = parse_output(output)
    report_path = write_report(args, subset, exit_code, output, parsed)
    print(f"\nEvidence report: {report_path}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
