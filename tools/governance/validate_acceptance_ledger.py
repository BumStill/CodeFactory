#!/usr/bin/env python3
"""M162：规格逐条验收账本——每个 R 项都要有对应证据才算完成。

为什么存在
----------
2026-10-22 两次真实的假完成：M145 的 R3（会话诊断）漏做却声称完成，M155 的
R5（跨进程 smoke）被悄悄推迟。两次都不是"忘了写代码"，而是**规格里的 R 项从来
没有对应的机器判定**：CLI 只检查过 `git status` 与命令退出码，没有人比对
"规格写了 5 条 → 账本里 5 条都有证据"。

唯一权威（R1）
--------------
需求编号只从规格的「需求」章节条目里读，不在本脚本里另造映射表；账本的路径与
任务号也由规格文件名推导（`m162-xxx.md` → `docs/acceptance/m162.md`）。

账本契约
--------
`docs/acceptance/<task-id>.md`，每个 R 项一行（表格或列表皆可）：

    | R1 | done | `tests/test_x.py::test_r1` |
    | R2 | deferred | 原因：<为什么没做> | 后续：m170 |
    | R3 | n/a | <为什么本任务不适用> |

- `done` 必须带测试名（`path::test`）、证据路径（含 `:line`）或一条测试命令。
- `deferred` 必须同时写原因与后续任务，否则视为"悄悄推迟"。
- `n/a` 必须写不适用的原因。
- 未在规格中出现的编号、未知状态、缺行都是失败；这里是 fail-closed 的。
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

SPEC_GLOB = "docs/specs/**/*.md"
LEDGER_DIR = "docs/acceptance"

REQUIREMENT_ID = re.compile(r"^\s*[-*]\s+\*{0,2}(R\d+)\b")
SECTION_HEADER = re.compile(r"^(#{1,6})\s+(.*)$")
REQUIREMENT_SECTION = re.compile(r"(需求|requirements?)", re.IGNORECASE)
# 首列 = 编号 + 可选标题（`R1` / `R1 交付前生成账本` / `**R1**：…`），编号必须在行首。
LEDGER_ROW_ID = re.compile(r"^\*{0,2}(R\d+)\*{0,2}(?:[\s:：—|].*)?$")
TEST_ID = re.compile(r"[A-Za-z0-9_./\\-]+::[A-Za-z0-9_:.-]+")
EVIDENCE_PATH = re.compile(r"[A-Za-z0-9_./\\-]+\.(py|rs|ts|tsx|js|mjs|cjs|md|json|ya?ml|toml|ps1|sh)\b")
EVIDENCE_LOCATION = re.compile(r"[A-Za-z0-9_./\\-]+:\d+")
TEST_COMMAND = re.compile(
    r"\b(cargo test|pnpm test|npm test|npx |pytest|node --test|python3? -m unittest|go test|vitest)\b",
    re.IGNORECASE,
)
DEFERRED_REASON = re.compile(r"(原因|理由|reason)", re.IGNORECASE)
DEFERRED_FOLLOWUP = re.compile(r"(后续|follow[\s-]?up)", re.IGNORECASE)
SPEC_REFERENCE = re.compile(r"(docs/specs/[A-Za-z0-9._/\-]+\.md)")
LEDGER_DECLARATION = re.compile(r"Acceptance-Ledger:\s*`?([A-Za-z0-9._/\-]+\.md)`?")

STATUSES = {"done", "deferred", "n/a"}


# --------------------------------------------------------------------------- 规格

def parse_requirements(text: str) -> list[str]:
    """规格「需求」章节里的 R 编号，按出现顺序去重。"""
    requirements: list[str] = []
    section_level: int | None = None
    for raw in text.splitlines():
        header = SECTION_HEADER.match(raw)
        if header:
            level = len(header.group(1))
            if REQUIREMENT_SECTION.search(header.group(2)):
                section_level = level
            elif section_level is not None and level <= section_level:
                section_level = None
            continue
        if section_level is None:
            continue
        match = REQUIREMENT_ID.match(raw)
        if match and match.group(1) not in requirements:
            requirements.append(match.group(1))
    return requirements


def task_id_for_spec(spec_path: Path | str) -> str:
    """`m162-requirement-acceptance-ledger.md` → `m162`；无前缀则用文件名主干。"""
    name = Path(str(spec_path).replace("\\", "/")).name
    stem = name[:-3] if name.endswith(".md") else name
    match = re.match(r"^((?:m|mi|u|e2e)\d+)(?:[-_]|$)", stem, re.IGNORECASE)
    return match.group(1).lower() if match else stem


# --------------------------------------------------------------------------- 账本

def parse_ledger(text: str) -> dict[str, dict[str, str]]:
    """账本行 → {R 编号: {status, evidence}}；表格行与列表行等价。"""
    entries: dict[str, dict[str, str]] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("|"):
            cells = [cell.strip() for cell in line.strip("|").split("|")]
        else:
            bullet = re.match(r"^[-*]\s+(.*)$", line)
            if not bullet:
                continue
            cells = [cell.strip() for cell in bullet.group(1).split("|")]
        if len(cells) < 2:
            continue
        identifier = LEDGER_ROW_ID.match(cells[0])
        if not identifier:
            continue
        status = cells[1].strip().strip("`*").strip().lower()
        evidence = " | ".join(cells[2:]).strip()
        entries[identifier.group(1)] = {"status": status, "evidence": evidence}
    return entries


def _has_evidence(evidence: str) -> bool:
    return bool(
        TEST_ID.search(evidence)
        or EVIDENCE_LOCATION.search(evidence)
        or EVIDENCE_PATH.search(evidence)
        or TEST_COMMAND.search(evidence)
    )


def _strip_markers(text: str) -> str:
    return text.strip().strip("|").strip()


def check_spec(spec_path: Path, ledger_path: Path) -> list[str]:
    """比对一份规格与它的账本，返回人话问题列表（空列表 = 通过）。"""
    if not spec_path.exists():
        return [f"规格缺失：{spec_path} 不存在，无法确定需要验收哪些 R 项"]

    requirements = parse_requirements(spec_path.read_text(encoding="utf-8"))
    if not requirements:
        return []

    if not ledger_path.exists():
        return [
            f"账本缺失：{ledger_path.name} 未创建；规格 {spec_path.name} 的 "
            f"{'、'.join(requirements)} 需要逐条证据（路径 {ledger_path}）"
        ]

    entries = parse_ledger(ledger_path.read_text(encoding="utf-8"))
    problems: list[str] = []
    for requirement in requirements:
        entry = entries.get(requirement)
        if entry is None:
            problems.append(
                f"{requirement} 未出现在账本 {ledger_path.name} 中："
                "规格里的每一项都要有一行状态与证据"
            )
            continue
        status = entry["status"]
        evidence = _strip_markers(entry["evidence"])
        if status not in STATUSES:
            problems.append(
                f"{requirement} 的状态 `{status or '(空)'}` 无效（账本 {ledger_path.name}）："
                "只允许 done / deferred / n/a"
            )
        elif status == "done":
            if not evidence or not _has_evidence(evidence):
                problems.append(
                    f"{requirement} 标为 done 但没有测试名或证据路径（账本 {ledger_path.name}）："
                    "写 `path::test`、`file:line` 或一条测试命令"
                )
        elif status == "deferred":
            if not DEFERRED_REASON.search(evidence) or not DEFERRED_FOLLOWUP.search(evidence):
                problems.append(
                    f"{requirement} 标为 deferred 但没有写清原因与后续任务（账本 {ledger_path.name}）："
                    "形如 `原因：<为什么没做> | 后续：<任务号>`"
                )
        elif status == "n/a":
            if len(evidence) < 2:
                problems.append(
                    f"{requirement} 标为 n/a 但没有说明为什么不适用（账本 {ledger_path.name}）"
                )
    return problems


def _repo_relative(repo_root: Path, path: Path) -> Path:
    try:
        return path.resolve().relative_to(repo_root.resolve())
    except ValueError:
        return path


def ledger_for_spec(repo_root: Path, spec_path: Path, declared: str | None = None) -> Path:
    if declared:
        return (repo_root / declared).resolve() if not Path(declared).is_absolute() else Path(declared)
    return repo_root / LEDGER_DIR / f"{task_id_for_spec(spec_path)}.md"


def _git_changed_files(repo_root: Path, base_ref: str | None) -> list[str]:
    if not base_ref:
        return []
    commands = []
    for candidate in (f"origin/{base_ref}", base_ref):
        commands.append(["git", "-C", str(repo_root), "diff", "--name-only", f"{candidate}...HEAD"])
    commands.append(["git", "-C", str(repo_root), "diff", "--name-only", "HEAD"])
    commands.append(["git", "-C", str(repo_root), "diff", "--name-only", "--cached"])
    changed: list[str] = []
    for command in commands:
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        except (OSError, subprocess.SubprocessError):
            continue
        if result.returncode != 0:
            continue
        for name in result.stdout.splitlines():
            name = name.strip()
            if name and name not in changed:
                changed.append(name)
    return changed


# --------------------------------------------------------------------------- 入口

def check_pr_acceptance_ledger(
    repo_root: Path,
    body: str | None,
    base_ref: str | None = None,
) -> list[str]:
    """PR 引用的规格（正文声明 + base..HEAD 变更）必须账本齐全。"""
    repo_root = Path(repo_root)
    declared_ledger: str | None = None
    spec_references: list[str] = []

    if body:
        declaration = LEDGER_DECLARATION.search(body)
        if declaration:
            declared_ledger = declaration.group(1)
        for match in SPEC_REFERENCE.finditer(body):
            reference = match.group(1)
            if reference not in spec_references:
                spec_references.append(reference)

    for changed in _git_changed_files(repo_root, base_ref):
        normalized = changed.replace("\\", "/")
        if normalized.startswith("docs/specs/") and normalized.endswith(".md") and normalized not in spec_references:
            spec_references.append(normalized)

    problems: list[str] = []
    for reference in spec_references:
        spec_path = repo_root / reference
        if not spec_path.exists():
            # A spec the PR talks about but did not ship is only a problem when the
            # PR also changed it; a stale link in prose must not block delivery.
            continue
        ledger_path = ledger_for_spec(repo_root, spec_path, declared_ledger)
        for problem in check_spec(spec_path, ledger_path):
            problems.append(f"{reference}: {problem}")
    return problems


def _self_check_existing_ledgers(repo_root: Path) -> list[str]:
    """没有 PR 上下文时（push 到 main）：已存在的账本仍必须与规格自洽。"""
    problems: list[str] = []
    ledger_dir = repo_root / LEDGER_DIR
    if not ledger_dir.is_dir():
        return problems
    specs = sorted(repo_root.glob(SPEC_GLOB))
    for ledger in sorted(ledger_dir.glob("*.md")):
        task_id = ledger.stem.lower()
        matches = [spec for spec in specs if task_id_for_spec(spec) == task_id]
        if not matches:
            problems.append(f"账本 {ledger.name} 没有对应的规格（docs/specs 下找不到 {task_id}-*.md）")
            continue
        problems.extend(check_spec(matches[0], ledger))
    return problems


def read_body(body_file: str | None = None) -> str | None:
    if body_file:
        path = Path(body_file)
        if path.exists():
            return path.read_text(encoding="utf-8")
        return None
    import os

    event_path = os.environ.get("GITHUB_EVENT_PATH")
    if not event_path:
        return None
    try:
        event = json.loads(Path(event_path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    pull_request = event.get("pull_request") or {}
    body = pull_request.get("body")
    return body if isinstance(body, str) else None


def build_report(repo_root: Path, body: str | None, base_ref: str | None) -> dict:
    if body or base_ref:
        problems = check_pr_acceptance_ledger(repo_root, body, base_ref)
        checked = ["pr_referenced_specs"]
    else:
        problems = _self_check_existing_ledgers(repo_root)
        checked = ["existing_ledgers"]
    return {
        "status": "fail" if problems else "pass",
        "repo_root": str(repo_root),
        "checked": checked,
        "problems": problems,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="M162 规格逐条验收账本校验")
    parser.add_argument("--repo", default=".", help="仓库根目录")
    parser.add_argument("--body-file", default=None, help="PR 正文本地预检（M156 通道）")
    parser.add_argument("--base-ref", default=None, help="对比基线分支，取变更的规格文件")
    # M156 预检通道对部分校验器会追加 --title/--ci；本校验不需要它们，接受但忽略，
    # 以免通道把"参数不认识"误报成账本失败。
    parser.add_argument("--title", default=None, help=argparse.SUPPRESS)
    parser.add_argument("--ci", action="store_true", help=argparse.SUPPRESS)
    arguments = parser.parse_args(argv)

    repo_root = Path(arguments.repo).resolve()
    report = build_report(repo_root, read_body(arguments.body_file), arguments.base_ref)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 1 if report["problems"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
