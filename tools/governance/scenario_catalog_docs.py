#!/usr/bin/env python3
"""Render and validate registry-derived scenario catalog documentation."""

from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
REGISTRY_PATH = REPO_ROOT / "docs" / "testing" / "scenario-registry.json"

SUMMARY_START = "<!-- scenario-registry-summary:start -->"
SUMMARY_END = "<!-- scenario-registry-summary:end -->"
CATEGORIES_START = "<!-- scenario-registry-categories:start -->"
CATEGORIES_END = "<!-- scenario-registry-categories:end -->"
CASES_START = "<!-- scenario-registry-cases:start -->"
CASES_END = "<!-- scenario-registry-cases:end -->"


def load_registry(path: Path = REGISTRY_PATH) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _managed_block(start: str, body: list[str], end: str) -> str:
    return "\n".join([start, *body, end])


def render_summary(registry: dict[str, Any]) -> str:
    scenarios = registry.get("scenarios") or []
    cases = registry.get("complex_e2e_cases") or []
    priorities = Counter(item.get("priority") for item in scenarios)
    statuses = Counter(item.get("automation_status") for item in cases)
    gaps = sum(len(item.get("remaining_gaps") or []) for item in cases)
    pr_statuses = Counter(
        (item.get("pull_request_gate") or {}).get("status", "missing")
        for item in cases
    )
    return _managed_block(
        SUMMARY_START,
        [
            f"- 逻辑 Scenario：`{len(scenarios)}`（P0 `{priorities['P0']}`，P1 `{priorities['P1']}`，P2 `{priorities['P2']}`）",
            "- Complex E2E："
            f"`{len(cases)}`（implemented `{statuses['implemented']}`，"
            f"partially_implemented `{statuses['partially_implemented']}`，"
            f"designed `{statuses['designed']}`）",
            f"- 剩余自动化缺口：`{gaps}`",
            "- PR slice："
            f"implemented `{pr_statuses['implemented']}`，"
            f"partially_implemented `{pr_statuses['partially_implemented']}`，"
            f"missing `{pr_statuses['missing']}`",
        ],
        SUMMARY_END,
    )


def render_categories(registry: dict[str, Any]) -> str:
    scenarios = registry.get("scenarios") or []
    counts = Counter(item.get("category") for item in scenarios)
    by_priority = Counter(
        (item.get("category"), item.get("priority")) for item in scenarios
    )
    rows = [
        "| 分类 | 总数 | P0 | P1 | P2 |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for category in registry.get("categories") or []:
        category_id = category.get("id")
        rows.append(
            f"| {category.get('name')} (`{category_id}`) | {counts[category_id]} | "
            f"{by_priority[(category_id, 'P0')]} | "
            f"{by_priority[(category_id, 'P1')]} | "
            f"{by_priority[(category_id, 'P2')]} |"
        )
    return _managed_block(CATEGORIES_START, rows, CATEGORIES_END)


def render_cases(registry: dict[str, Any]) -> str:
    rows = [
        "| Case | 名称 | 优先级 | 总体状态 | 剩余缺口 | PR slice | PR 缺口 |",
        "| --- | --- | --- | --- | ---: | --- | ---: |",
    ]
    for case in registry.get("complex_e2e_cases") or []:
        pr_gate = case.get("pull_request_gate") or {}
        rows.append(
            f"| {case.get('id')} | {case.get('name')} | {case.get('priority')} | "
            f"`{case.get('automation_status')}` | "
            f"{len(case.get('remaining_gaps') or [])} | "
            f"`{pr_gate.get('status', 'missing')}` | "
            f"{len(pr_gate.get('remaining_gaps') or [])} |"
        )
    return _managed_block(CASES_START, rows, CASES_END)


def _validate_block(path: Path, expected: str, start: str, end: str) -> list[str]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"cannot read {path}: {exc}"]

    start_count = text.count(start)
    end_count = text.count(end)
    if start_count != 1 or end_count != 1:
        return [
            f"{path.relative_to(REPO_ROOT)} must contain exactly one managed block "
            f"{start} ... {end}"
        ]
    actual = text[text.index(start) : text.index(end) + len(end)]
    if actual != expected:
        return [
            f"{path.relative_to(REPO_ROOT)} registry-derived block is stale; "
            "render it again from docs/testing/scenario-registry.json"
        ]
    return []


def validate_catalog_docs(
    repo_root: Path = REPO_ROOT, registry: dict[str, Any] | None = None
) -> list[str]:
    active_registry = registry or load_registry(
        repo_root / "docs" / "testing" / "scenario-registry.json"
    )
    checks = (
        (
            repo_root / "docs" / "testing" / "README.md",
            render_summary(active_registry),
            SUMMARY_START,
            SUMMARY_END,
        ),
        (
            repo_root / "docs" / "specs" / "feature-specs" / "scenario-test-governance.md",
            render_summary(active_registry),
            SUMMARY_START,
            SUMMARY_END,
        ),
        (
            repo_root / "docs" / "specs" / "feature-specs" / "scenario-test-governance.md",
            render_categories(active_registry),
            CATEGORIES_START,
            CATEGORIES_END,
        ),
        (
            repo_root / "docs" / "specs" / "feature-specs" / "scenario-test-governance.md",
            render_cases(active_registry),
            CASES_START,
            CASES_END,
        ),
    )
    errors: list[str] = []
    for path, expected, start, end in checks:
        errors.extend(_validate_block(path, expected, start, end))
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--render",
        choices=("summary", "categories", "cases"),
        help="print one canonical managed block instead of checking documents",
    )
    args = parser.parse_args()
    registry = load_registry()
    if args.render:
        renderers = {
            "summary": render_summary,
            "categories": render_categories,
            "cases": render_cases,
        }
        print(renderers[args.render](registry))
        return 0

    errors = validate_catalog_docs(REPO_ROOT, registry)
    if errors:
        print(f"scenario-catalog-docs: {len(errors)} error(s)")
        for error in errors:
            print(f"::error::{error}")
        return 1
    print("scenario-catalog-docs: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
