#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
from collections import Counter
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
SUBSET_PATH = REPO_ROOT / "docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json"
EVIDENCE_DIR = REPO_ROOT / "docs/evidence-packs"


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text())


def task_name_from_result(result: dict[str, Any]) -> str | None:
    task_id = result.get("task_id")
    if isinstance(task_id, dict) and task_id.get("name"):
        return str(task_id["name"])
    task_name = result.get("task_name")
    if isinstance(task_name, str) and task_name:
        return task_name.rsplit("/", 1)[-1]
    config = result.get("config")
    if isinstance(config, dict):
        task = config.get("task")
        if isinstance(task, dict) and task.get("name"):
            return str(task["name"]).rsplit("/", 1)[-1]
    return None


def reward_from_result(result: dict[str, Any]) -> float:
    verifier = result.get("verifier_result")
    if isinstance(verifier, dict):
        rewards = verifier.get("rewards")
        if isinstance(rewards, dict):
            value = rewards.get("reward")
            if isinstance(value, (int, float)):
                return float(value)
    return 0.0


def failure_reason(result: dict[str, Any], subset_task: dict[str, Any]) -> str | None:
    exception = result.get("exception_info")
    if isinstance(exception, dict):
        message = str(exception.get("exception_message") or "")
        exception_type = str(exception.get("exception_type") or "")
        if "Command timed out" in message:
            return "command-timeout"
        if exception_type == "AddTestsDirError":
            return "harbor-tests-upload"
        if "CPU" in message or "cpu" in message:
            return "docker-cpu-limit"
        return exception_type or "exception"
    if reward_from_result(result) <= 0:
        bucket = subset_task.get("bucket")
        if bucket:
            return str(bucket)
        return "verifier-zero"
    return None


def load_trials(job_path: Path) -> dict[str, dict[str, Any]]:
    trials: dict[str, dict[str, Any]] = {}
    for result_path in sorted(job_path.glob("*/result.json")):
        result = read_json(result_path)
        task_name = task_name_from_result(result)
        if not task_name:
            continue
        trials[task_name] = {
            "result_path": result_path,
            "trial_dir": result_path.parent,
            "result": result,
        }
    return trials


def summarize(subset: dict[str, Any], trials: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for subset_task in subset["tasks"]:
        task = str(subset_task["name"])
        trial = trials.get(task)
        if not trial:
            rows.append(
                {
                    "task": task,
                    "reward": 0.0,
                    "failure_class": "missing-result",
                    "failure_reason": "missing-result",
                    "bucket": subset_task.get("bucket"),
                    "trial_dir": None,
                    "tokens": None,
                    "cost": None,
                }
            )
            continue
        result = trial["result"]
        agent_result = result.get("agent_result")
        metadata = agent_result.get("metadata") if isinstance(agent_result, dict) else None
        rows.append(
            {
                "task": task,
                "reward": reward_from_result(result),
                "failure_class": subset_task.get("baseline_failure_class") or "pass",
                "failure_reason": failure_reason(result, subset_task),
                "bucket": subset_task.get("bucket"),
                "trial_dir": str(trial["trial_dir"]),
                "tokens": {
                    "input": agent_result.get("n_input_tokens") if isinstance(agent_result, dict) else None,
                    "cache": agent_result.get("n_cache_tokens") if isinstance(agent_result, dict) else None,
                    "output": agent_result.get("n_output_tokens") if isinstance(agent_result, dict) else None,
                },
                "cost": agent_result.get("cost_usd") if isinstance(agent_result, dict) else None,
                "tool_calls": metadata.get("tool_calls") if isinstance(metadata, dict) else None,
            }
        )
    return rows


def score_label(pass_count: int, total: int) -> str:
    ratio = pass_count / max(total, 1)
    if ratio < 0.10:
        return "low full-benchmark baseline"
    if ratio < 0.35:
        return "early scaffold baseline"
    if ratio < 0.60:
        return "developing agent capability"
    return "strong subset capability"


def write_report(subset: dict[str, Any], job_path: Path, rows: list[dict[str, Any]]) -> Path:
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H-%M-%SZ")
    report_path = EVIDENCE_DIR / f"terminal-bench-21-regression-subset-baseline-{timestamp}.md"
    total = len(rows)
    pass_count = sum(1 for row in rows if float(row["reward"]) > 0)
    mean_reward = sum(float(row["reward"]) for row in rows) / max(total, 1)
    class_counts = Counter(str(row["failure_class"]) for row in rows)
    reason_counts = Counter(str(row["failure_reason"] or "pass") for row in rows)
    bucket_counts = Counter(str(row["bucket"] or "unknown") for row in rows)
    missing_usage = sum(
        1
        for row in rows
        if row["tokens"] is None
        or all(value is None for value in row["tokens"].values())
        or row["cost"] is None
    )

    lines = [
        "# Terminal-Bench 2.1 Regression Subset Baseline",
        "",
        f"- generated_at: `{timestamp}`",
        f"- source_job_path: `{job_path}`",
        f"- source_run_id: `{subset.get('source_run_id', '')}`",
        f"- subset: `{subset.get('id', SUBSET_PATH.stem)}`",
        f"- dataset: `{subset.get('dataset', '')}`",
        f"- evaluation_axis: `{subset.get('evaluation_axis', '')}`",
        f"- evaluation_subject: `{subset.get('evaluation_subject', '')}`",
        f"- model_backend: `{subset.get('model_backend', '')}`",
        f"- task_count: `{total}`",
        f"- pass_count: `{pass_count}`",
        f"- mean_reward: `{mean_reward:.6f}`",
        f"- level: `{score_label(pass_count, total)}`",
        f"- missing_usage_or_cost_trials: `{missing_usage}`",
        "",
        "This is an offline subset projection from the completed full Harbor job, not a fresh provider-backed rerun.",
        "",
        "## Failure Class Counts",
        "",
        "| Failure class | Count |",
        "| --- | ---: |",
    ]
    for name, count in sorted(class_counts.items()):
        lines.append(f"| `{name}` | `{count}` |")
    lines.extend(["", "## Failure Reason Counts", "", "| Failure reason | Count |", "| --- | ---: |"])
    for name, count in sorted(reason_counts.items()):
        lines.append(f"| `{name}` | `{count}` |")
    lines.extend(["", "## Selection Bucket Counts", "", "| Bucket | Count |", "| --- | ---: |"])
    for name, count in sorted(bucket_counts.items()):
        lines.append(f"| `{name}` | `{count}` |")
    lines.extend(
        [
            "",
            "## Trials",
            "",
            "| Task | Reward | Failure class | Failure reason | Bucket | Tool calls | Trial dir |",
            "| --- | ---: | --- | --- | --- | ---: | --- |",
        ]
    )
    for row in rows:
        trial_dir = row["trial_dir"] or ""
        tool_calls = "" if row["tool_calls"] is None else str(row["tool_calls"])
        lines.append(
            f"| `{row['task']}` | `{row['reward']:.1f}` | `{row['failure_class']}` | "
            f"`{row['failure_reason'] or 'pass'}` | `{row['bucket']}` | `{tool_calls}` | `{trial_dir}` |"
        )
    lines.extend(
        [
            "",
            "## Score-Driven Improvement Direction",
            "",
            "- P0: keep evaluation infrastructure separate from agent capability by treating credential, Docker resource, and Harbor upload failures as blockers or environment failures.",
            "- P1: reduce exception/timeout outcomes first; target `command-timeout`, service lifecycle, and long command supervision so failures reach verifier output instead of aborting trials.",
            "- P2: convert verifier-zero tasks into structured repair goals with expected artifact, failing assertion, smallest rerun command, and final-before-verify gate.",
            "- P3: move tool-use errors into a planner policy with cwd/file inventory, background process templates, and automatic alternatives for missing commands/files.",
            "- P4: only rerun the full 89-task benchmark after this 18-task subset improves from `4 / 18` to at least `7 / 18` under the same attribution axis.",
            "",
        ]
    )
    report_path.write_text("\n".join(lines))
    return report_path


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Summarize the fixed Terminal-Bench 2.1 regression subset from an existing full Harbor job."
    )
    parser.add_argument("--subset", type=Path, default=SUBSET_PATH)
    parser.add_argument("--job-path", type=Path)
    args = parser.parse_args()

    subset = read_json(args.subset)
    job_path = args.job_path or Path(str(subset["source_job_path"]))
    if not job_path.exists():
        raise SystemExit(f"job path does not exist: {job_path}")
    rows = summarize(subset, load_trials(job_path))
    report_path = write_report(subset, job_path, rows)
    pass_count = sum(1 for row in rows if float(row["reward"]) > 0)
    mean_reward = sum(float(row["reward"]) for row in rows) / max(len(rows), 1)
    print(
        "subset_baseline "
        f"subset={subset.get('id', SUBSET_PATH.stem)} "
        f"tasks={len(rows)} pass={pass_count} mean_reward={mean_reward:.6f} "
        f"report={report_path}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
