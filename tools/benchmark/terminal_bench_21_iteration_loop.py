#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SUBSET_PATH = REPO_ROOT / "docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json"
CANARY_SUBSET_PATH = REPO_ROOT / "docs/benchmark-subsets/terminal-bench-21-canary-subset-v1.json"
EVIDENCE_DIR = REPO_ROOT / "docs/evidence-packs"
REGRESSION_RUNNER = REPO_ROOT / "tools/benchmark/run_terminal_bench_21_regression_subset.py"
DEFAULT_BASELINE = (
    EVIDENCE_DIR
    / "terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md"
)
DEFAULT_CANARY_TASKS = [
    "write-compressor",
    "filter-js-from-html",
    "mteb-retrieve",
    "count-dataset-tokens",
]


@dataclass(frozen=True)
class EvidenceSummary:
    path: Path
    run: str | None
    subset: str | None
    pass_count: int | None
    trials: int | None
    mean_reward: float | None
    failure_counts: dict[str, int]


def read_subset(path: Path) -> dict:
    data = json.loads(path.read_text())
    tasks = data.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise SystemExit(f"subset has no tasks: {path}")
    return data


def write_canary_subset(source: dict, task_names: list[str], directory: Path) -> Path:
    available = {str(item["name"]): item for item in source["tasks"]}
    missing = [name for name in task_names if name not in available]
    if missing:
        raise SystemExit(f"canary tasks not found in subset: {', '.join(missing)}")
    canary = dict(source)
    canary["id"] = f"{source.get('id', SUBSET_PATH.stem)}-canary"
    canary["selection_policy"] = (
        "Canary scope for score-driven CodeFactory agent iteration. "
        "It is a fast gate before the fixed 18-task regression subset."
    )
    canary["tasks"] = [available[name] for name in task_names]
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "terminal-bench-21-canary-subset.json"
    path.write_text(json.dumps(canary, indent=2) + "\n")
    return path


def latest_regression_evidence() -> Path | None:
    candidates = [
        path
        for path in EVIDENCE_DIR.glob("terminal-bench-21-regression-subset-*.md")
        if "baseline" not in path.name
    ]
    if not candidates:
        return None
    return max(candidates, key=lambda path: path.stat().st_mtime)


def parse_evidence(path: Path) -> EvidenceSummary:
    text = path.read_text()
    run = field(text, "run")
    subset = field(text, "subset")
    pass_count = int_field(text, "pass_count")
    trials = int_field(text, "trials") or int_field(text, "task_count")
    mean_reward = float_field(text, "mean_reward")
    failure_counts = parse_count_section(text, "Failure Class Counts")
    if not failure_counts:
        failure_counts = parse_trial_failure_counts(text)
    return EvidenceSummary(
        path=path,
        run=run,
        subset=subset,
        pass_count=pass_count,
        trials=trials,
        mean_reward=mean_reward,
        failure_counts=failure_counts,
    )


def field(text: str, name: str) -> str | None:
    match = re.search(rf"^- {re.escape(name)}: `([^`]+)`", text, re.MULTILINE)
    return match.group(1) if match else None


def parse_count_section(text: str, title: str) -> dict[str, int]:
    section_match = re.search(
        rf"^## {re.escape(title)}\n(?P<section>.*?)(?:\n## |\Z)",
        text,
        re.MULTILINE | re.DOTALL,
    )
    if not section_match:
        return {}
    counts: dict[str, int] = {}
    for match in re.finditer(
        r"^\| `(?P<name>[^`]+)` \| `(?P<count>\d+)` \|",
        section_match.group("section"),
        re.MULTILINE,
    ):
        counts[normalize_failure_class(match.group("name"))] = int(match.group("count"))
    return counts


def parse_trial_failure_counts(text: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for line in text.splitlines():
        if not line.startswith("| `"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 3:
            continue
        if not re.fullmatch(r"`[0-9.]+`", cells[1]):
            continue
        failure_class = normalize_failure_class(cells[2].strip("`"))
        counts[failure_class] = counts.get(failure_class, 0) + 1
    return counts


def int_field(text: str, name: str) -> int | None:
    value = field(text, name)
    if value is None:
        return None
    try:
        return int(value)
    except ValueError:
        return None


def float_field(text: str, name: str) -> float | None:
    value = field(text, name)
    if value is None:
        return None
    try:
        return float(value)
    except ValueError:
        return None


def normalize_failure_class(raw: str) -> str:
    value = raw.strip()
    if value == "None":
        return "pass"
    option_match = re.fullmatch(r'Some\("([^"]+)"\)', value)
    if option_match:
        return option_match.group(1)
    return value


def run_subset(
    subset_path: Path,
    endpoint: str,
    model: str | None,
    concurrency: int,
    secret_timeout_sec: int,
) -> tuple[int, str]:
    command = [
        sys.executable,
        str(REGRESSION_RUNNER),
        "--subset",
        str(subset_path),
        "--endpoint",
        endpoint,
        "--concurrency",
        str(concurrency),
        "--secret-timeout-sec",
        str(secret_timeout_sec),
    ]
    if model:
        command.extend(["--model", model])
    process = subprocess.Popen(
        command,
        cwd=REPO_ROOT,
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


def extract_evidence_path(output: str) -> Path | None:
    match = re.search(r"Evidence report: (?P<path>\S+)", output)
    return Path(match.group("path")) if match else None


def delta_line(label: str, before: int | float | None, after: int | float | None) -> str:
    if before is None or after is None:
        return f"- {label}: `unknown`"
    delta = after - before
    sign = "+" if delta >= 0 else ""
    if isinstance(before, float) or isinstance(after, float):
        return f"- {label}: `{before:.6f}` -> `{after:.6f}` (`{sign}{delta:.6f}`)"
    return f"- {label}: `{before}` -> `{after}` (`{sign}{delta}`)"


def format_failure_counts(summary: EvidenceSummary | None) -> list[str]:
    if summary is None or not summary.failure_counts:
        return ["- no trial failure table available"]
    return [
        f"- `{name}`: `{count}`"
        for name, count in sorted(summary.failure_counts.items(), key=lambda item: item[0])
    ]


def write_iteration_report(
    args: argparse.Namespace,
    scope: str,
    subset_path: Path,
    baseline: EvidenceSummary | None,
    head: EvidenceSummary | None,
    exit_code: int | None,
    ran_command: bool,
    output: str,
) -> Path:
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H-%M-%SZ")
    report_path = EVIDENCE_DIR / f"terminal-bench-21-iteration-{timestamp}.md"
    lines = [
        "# Terminal-Bench 2.1 Product Iteration Report",
        "",
        f"- generated_at: `{timestamp}`",
        "- evaluation_axis: `codefactory-agent-capability`",
        "- evaluation_subject: `codefactory-headless`",
        f"- scope: `{scope}`",
        f"- subset_path: `{subset_path}`",
        f"- endpoint: `{args.endpoint}`",
        f"- model: `{args.model or '<settings default>'}`",
        f"- hypothesis: `{args.hypothesis}`",
        f"- target_failure_class: `{args.target_failure_class}`",
        f"- ran_command: `{'yes' if ran_command else 'no'}`",
    ]
    if exit_code is not None:
        lines.append(f"- exit_code: `{exit_code}`")
    lines.extend(
        [
            "",
            "## Baseline",
            "",
            f"- path: `{baseline.path if baseline else 'not available'}`",
            f"- run: `{baseline.run if baseline and baseline.run else 'not available'}`",
            f"- pass_count: `{baseline.pass_count if baseline and baseline.pass_count is not None else 'unknown'}`",
            f"- trials: `{baseline.trials if baseline and baseline.trials is not None else 'unknown'}`",
            f"- mean_reward: `{baseline.mean_reward if baseline and baseline.mean_reward is not None else 'unknown'}`",
            "",
            "## Head",
            "",
            f"- path: `{head.path if head else 'not available'}`",
            f"- run: `{head.run if head and head.run else 'not available'}`",
            f"- pass_count: `{head.pass_count if head and head.pass_count is not None else 'unknown'}`",
            f"- trials: `{head.trials if head and head.trials is not None else 'unknown'}`",
            f"- mean_reward: `{head.mean_reward if head and head.mean_reward is not None else 'unknown'}`",
            "",
            "## Delta",
            "",
            delta_line(
                "pass_count",
                baseline.pass_count if baseline else None,
                head.pass_count if head else None,
            ),
            delta_line(
                "mean_reward",
                baseline.mean_reward if baseline else None,
                head.mean_reward if head else None,
            ),
            "",
            "## Failure Class Counts",
            "",
            "Baseline:",
            *format_failure_counts(baseline),
            "",
            "Head:",
            *format_failure_counts(head),
            "",
            "## Next Improvement Queue",
            "",
            *next_actions(args.target_failure_class),
            "",
            "## Command Output Tail",
            "",
            "```text",
            tail(output, 12000) if output else "not executed",
            "```",
            "",
        ]
    )
    report_path.write_text("\n".join(lines))
    return report_path


def next_actions(target_failure_class: str) -> list[str]:
    mapping = {
        "tool-use": [
            "- P0: reduce repeated inspection by escalating to artifact implementation earlier.",
            "- P0: add command preflight for missing files, wrong cwd, command-not-found, and obvious non-productive reads.",
            "- P1: feed compact workspace inventory into the model before broad exploration.",
        ],
        "policy": [
            "- P0: split hard-deny policy from supervised service/build allowances.",
            "- P1: add background service lifecycle templates with pid, log, readiness, client check, and cleanup.",
        ],
        "verification": [
            "- P0: parse verifier/self-check output into a concrete repair_goal.",
            "- P1: block final answers until the smallest available self-check has run after a candidate fix.",
        ],
        "environment": [
            "- P0: preflight Docker CPU/memory/storage before counting the run as agent capability.",
            "- P1: tag environment failures as blocked and reroute to infrastructure queue.",
        ],
    }
    return mapping.get(
        target_failure_class,
        [
            "- P0: inspect the dominant failure class and choose one targeted canary before broader regression.",
            "- P1: rerun the fixed subset only after the targeted canary shows a behavior delta.",
        ],
    )


def tail(value: str, limit: int) -> str:
    return value if len(value) <= limit else value[-limit:]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run or plan a score-driven Terminal-Bench 2.1 product iteration."
    )
    parser.add_argument("--scope", choices=["canary", "regression"], default="canary")
    parser.add_argument("--subset", type=Path, default=SUBSET_PATH)
    parser.add_argument("--canary-subset", type=Path, default=CANARY_SUBSET_PATH)
    parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    parser.add_argument("--head", type=Path)
    parser.add_argument("--endpoint", default="deepseek")
    parser.add_argument("--model")
    parser.add_argument("--concurrency", type=int, default=2)
    parser.add_argument("--secret-timeout-sec", type=int, default=20)
    parser.add_argument("--target-failure-class", default="tool-use")
    parser.add_argument("--hypothesis", required=True)
    parser.add_argument(
        "--canary-task",
        action="append",
        dest="canary_tasks",
        help="Task name to include in canary scope. Can be passed multiple times.",
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help="Actually launch the provider-backed benchmark run. Without this, only write the iteration plan/report.",
    )
    args = parser.parse_args()

    subset_path = args.subset
    if args.scope == "canary":
        if args.canary_tasks:
            source_subset = read_subset(args.subset)
            custom_dir = REPO_ROOT / ".codefactory/benchmark-subsets"
            subset_path = write_canary_subset(
                source_subset,
                args.canary_tasks,
                custom_dir,
            )
        else:
            subset_path = args.canary_subset
            read_subset(subset_path)
    else:
        read_subset(subset_path)

    baseline = parse_evidence(args.baseline) if args.baseline.exists() else None
    output = ""
    exit_code: int | None = None
    head_path = args.head
    if args.execute:
        exit_code, output = run_subset(
            subset_path,
            endpoint=args.endpoint,
            model=args.model,
            concurrency=args.concurrency,
            secret_timeout_sec=args.secret_timeout_sec,
        )
        head_path = extract_evidence_path(output)
    elif head_path is None:
        head_path = latest_regression_evidence()

    head = parse_evidence(head_path) if head_path and head_path.exists() else None
    report_path = write_iteration_report(
        args=args,
        scope=args.scope,
        subset_path=subset_path,
        baseline=baseline,
        head=head,
        exit_code=exit_code,
        ran_command=args.execute,
        output=output,
    )
    print(f"Iteration report: {report_path}")
    return exit_code or 0


if __name__ == "__main__":
    raise SystemExit(main())
