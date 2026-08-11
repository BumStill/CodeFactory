#!/usr/bin/env python3
"""Reject manual user takeover contracts for system-owned technical failures.

This is intentionally a semantic, context-aware gate rather than a forbidden-word
list. Product documentation must remain free to describe historical defects and
forbidden copy, while normative specs and production UI copy must not make a user
click retry/continue/resend to recover a technical objective.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Iterable, Sequence


REPO_ROOT = Path(__file__).resolve().parents[2]
VIOLATION_CODE = "OBJECTIVE_MANUAL_TECHNICAL_HANDOFF"

DOC_PREFIXES = ("docs/specs/", "docs/design/")
SOURCE_PREFIXES = ("src/", "src-tauri/src/")
SOURCE_SUFFIXES = {".rs", ".ts", ".tsx", ".js", ".jsx", ".html"}
TEST_PATH_RE = re.compile(
    r"(?:^|/)(?:tests?|__tests__|fixtures?)(?:/|$)|"
    r"\.(?:test|spec|stories)\.(?:[cm]?[jt]sx?|rs)$",
    re.IGNORECASE,
)

ACTION_RE = re.compile(
    r"(?:"
    r"继续(?:执行|处理|任务)|"
    r"回复.{0,6}继续|"
    r"重试(?:失败(?:步骤|项|任务)?|原端点|读取|任务|执行|请求)?|"
    r"已修复.{0,6}重试|"
    r"重新(?:发送|运行)(?:需要继续的内容|内容|消息|任务|失败步骤)?|"
    r"回到对话(?:处理)?|"
    r"再试一次|"
    r"\b(?:retry|continue|resend|back\s+to\s+chat)\b"
    r")",
    re.IGNORECASE,
)

TECHNICAL_RE = re.compile(
    r"(?:"
    r"技术|失败|错误|异常|中断|超时|耗尽|不可用|断开|崩溃|"
    r"恢复|修复|授权|权限|凭据|端点|候选|路由|"
    r"provider|transport|auth|credential|permission|route|"
    r"error|failed|failure|timeout|timed[_ -]?out|channel[_ -]?closed|"
    r"blocked|incident|panic|remediation|restart|"
    r"CI|测试失败|验证失败|检查失败|进程退出|应用退出"
    r")",
    re.IGNORECASE,
)

TAKEOVER_RE = re.compile(
    r"(?:"
    r"按钮|点击|动作|入口|"
    r"用户(?:(?:需要|必须|应当|应|可).{0,8})?(?:点击|操作|触发|回复|发送|推动)|"
    r"等待用户|由用户|需要你|"
    r"请(?:你|用户|在|先|回复|重新|点击|稍后|提供|打开|检查|完成|修复|手动|重试|继续|再试)|"
    r"手动|人工|输入框|"
    r"CTA|\baction\b|(?:aria-)?label\s*[:=]|\bbutton\b|onClick|"
    r"retry_primary|user[_ -]?action|please\s+(?:retry|continue|resend)"
    r")",
    re.IGNORECASE,
)

SYSTEM_OWNED_RE = re.compile(
    r"(?:"
    r"系统(?:正在|会|将|自动|继续|持有)|自动(?:重试|续接|恢复|继续)|"
    r"无需用户|不需要用户|system[_ -]?owned|supervisor|remediation|"
    r"recovery owner|waiting_system|platform_incident|failed_internal"
    r")",
    re.IGNORECASE,
)
TYPED_USER_WAIT_RE = re.compile(
    r"(?:core[_ -]?input|核心输入|typed\s+input|needs_business_decision|business\s+decision|业务决定)",
    re.IGNORECASE,
)

SAFE_SECTION_RE = re.compile(
    r"(?:"
    r"问题|背景|现状|历史|根因|风险|非目标|兼容|Compatibility|证据|Evidence|"
    r"文案禁区|禁用|禁止|反例|不可接受|旧行为|被取代|superseded"
    r")",
    re.IGNORECASE,
)

DIRECT_NEGATION_PREFIXES = (
    r"不得",
    r"不应",
    r"不要",
    r"不再",
    r"不允许",
    r"不显示",
    r"不提供",
    r"不要求",
    r"不依赖",
    r"不生成",
    r"不使用",
    r"不出现",
    r"不需要",
    r"无需",
    r"禁止",
    r"删除",
    r"移除",
    r"取代",
    r"拒绝",
    r"无(?:人工|手动)?",
    r"没有(?:人工|手动)?",
    r"未",
    r"不能(?:要求|显示|提供|依赖|生成|使用|作为|变成)",
    r"不能把",
    r"must\s+not",
    r"do\s+not",
    r"without\s+(?:a\s+)?user",
    r"without(?:\s+a)?",
    r"no\s+manual",
    r"no",
    r"never",
)
DIRECT_NEGATION_RE = re.compile(
    rf"(?:{'|'.join(DIRECT_NEGATION_PREFIXES)}).{{0,32}}{ACTION_RE.pattern}",
    re.IGNORECASE,
)
NEGATIVE_INTRO_RE = re.compile(
    r"(?:不得|禁止|不应|不再|不要|删除|移除|拒绝).{0,24}"
    r"(?:以下|这些|该|此类)?(?:文案|按钮|动作|入口|CTA|恢复契约)",
    re.IGNORECASE,
)
FORBIDDEN_TABLE_HEADER_RE = re.compile(r"\|[^\n|]*(?:禁止|不得|禁用)[^\n|]*\|", re.IGNORECASE)
HISTORICAL_INLINE_RE = re.compile(
    r"旧(?:文案|行为|实现|契约).{0,100}(?:取代|删除|不再|禁止|为准)",
    re.IGNORECASE,
)
NEGATIVE_ASSERTION_RE = re.compile(
    r"(?:negative|forbidden|负向|禁用).{0,28}(?:copy|assert|文案|断言)|"
    r"(?:发布阻断|阻断缺陷|forbidden copy)",
    re.IGNORECASE,
)
CORE_INPUT_ACTION_RE = re.compile(
    r"(?:重试|重新)(?:登录|验证|授权)|验证失败.{0,12}重试|"
    r"(?:retry|restart)\s+(?:login|oauth|authentication)",
    re.IGNORECASE,
)
COMMENT_LINE_RE = re.compile(r"^\s*(?://|/\*|\*|\*/|<!--)")
HEADING_RE = re.compile(r"^\s{0,3}#{1,6}\s+(.+?)\s*$")
TABLE_SEPARATOR_RE = re.compile(r"^:?-{3,}:?$")
DIFF_HUNK_RE = re.compile(
    r"^@@\s+-\d+(?:,\d+)?\s+\+(?P<start>\d+)(?:,(?P<count>\d+))?\s+@@"
)


@dataclass(frozen=True)
class Violation:
    code: str
    path: str
    line: int
    message: str
    excerpt: str


def _normalise_path(path: str | Path) -> str:
    return PurePosixPath(str(path).replace("\\", "/")).as_posix().lstrip("./")


def _is_protected_path(path: str) -> bool:
    if path.startswith(DOC_PREFIXES):
        return path.endswith(".md")
    if not path.startswith(SOURCE_PREFIXES):
        return False
    if TEST_PATH_RE.search(path):
        return False
    return PurePosixPath(path).suffix.lower() in SOURCE_SUFFIXES


def _is_source_path(path: str) -> bool:
    return path.startswith(SOURCE_PREFIXES)


def _directly_negates_action(line: str) -> bool:
    return bool(DIRECT_NEGATION_RE.search(line))


def _previous_lines_forbid_copy(lines: Sequence[str], index: int) -> bool:
    start = max(0, index - 3)
    return any(
        NEGATIVE_INTRO_RE.search(lines[i]) or FORBIDDEN_TABLE_HEADER_RE.search(lines[i])
        for i in range(start, index)
    )


def _context(lines: Sequence[str], index: int, radius: int = 4) -> str:
    start = max(0, index - radius)
    end = min(len(lines), index + radius + 1)
    return "\n".join(lines[start:end])


def _table_cells(line: str) -> list[str] | None:
    stripped = line.strip()
    if not (stripped.startswith("|") and stripped.endswith("|")):
        return None
    return [cell.strip() for cell in stripped[1:-1].split("|")]


def _rust_test_lines(lines: Sequence[str]) -> set[int]:
    """Best-effort line map for conventional `#[cfg(test)] mod tests { ... }` blocks."""

    test_lines: set[int] = set()
    cfg_test_pending = False
    in_test_module = False
    brace_depth = 0
    for index, line in enumerate(lines):
        if not in_test_module and "#[cfg(test)]" in line:
            cfg_test_pending = True
            test_lines.add(index)
            continue
        if cfg_test_pending and not in_test_module:
            test_lines.add(index)
            if re.search(r"\bmod\s+tests\b", line) and "{" in line:
                in_test_module = True
                brace_depth = line.count("{") - line.count("}")
            elif line.strip() and not line.lstrip().startswith("#"):
                cfg_test_pending = False
            continue
        if in_test_module:
            test_lines.add(index)
            brace_depth += line.count("{") - line.count("}")
            if brace_depth <= 0:
                in_test_module = False
                cfg_test_pending = False
    return test_lines


def _looks_like_action_control(line: str, context: str, source: bool) -> bool:
    markdown_control = bool(re.search(r"\[[^\]\n]*(?:重试|继续|重新发送|回到对话|retry|continue|resend)", line, re.I))
    action = ACTION_RE.search(line)
    if not action:
        return False
    if re.search(r"(?:回复|重新发送|回到对话|已修复|再试)", action.group(0), re.I):
        return True
    local_start = max(0, action.start() - 80)
    local_end = min(len(line), action.end() + 80)
    local_context = line[local_start:local_end]
    takeover_context = re.sub(
        r"(?:无需|不需要)用户.{0,16}(?:操作|点击|触发|回复|发送|推动)?",
        "",
        local_context,
        flags=re.IGNORECASE,
    )
    explicit_takeover = bool(TAKEOVER_RE.search(takeover_context))
    if source:
        return explicit_takeover
    return explicit_takeover or markdown_control


def validate_text(path: str | Path, text: str) -> list[Violation]:
    """Validate one in-memory file and return semantic contract violations."""

    normalised = _normalise_path(path)
    if not _is_protected_path(normalised):
        return []

    source = _is_source_path(normalised)
    lines = text.splitlines()
    rust_test_lines = _rust_test_lines(lines) if normalised.endswith(".rs") else set()
    section = ""
    violations: list[Violation] = []
    forbidden_table_columns: set[int] = set()

    for index, line in enumerate(lines):
        heading = HEADING_RE.match(line) if not source else None
        if heading:
            section = heading.group(1)
            forbidden_table_columns.clear()
            continue
        if source and COMMENT_LINE_RE.match(line):
            continue
        if index in rust_test_lines:
            continue
        if source and re.search(r"\bassert(?:_eq|_ne)?!\s*\(", line):
            continue

        cells = _table_cells(line) if not source else None
        action_in_forbidden_column = False
        if cells is None:
            forbidden_table_columns.clear()
        elif any("禁止" in cell or "不得" in cell or "禁用" in cell for cell in cells):
            forbidden_table_columns = {
                cell_index
                for cell_index, cell in enumerate(cells)
                if "禁止" in cell or "不得" in cell or "禁用" in cell
            }
        elif not all(TABLE_SEPARATOR_RE.fullmatch(cell) for cell in cells):
            action_in_forbidden_column = any(
                cell_index < len(cells) and ACTION_RE.search(cells[cell_index])
                for cell_index in forbidden_table_columns
            )

        action = ACTION_RE.search(line)
        if not action:
            continue
        if not source and SAFE_SECTION_RE.search(section):
            continue
        if (
            _directly_negates_action(line)
            or _previous_lines_forbid_copy(lines, index)
            or HISTORICAL_INLINE_RE.search(line)
            or NEGATIVE_ASSERTION_RE.search(line)
            or action_in_forbidden_column
        ):
            continue
        if CORE_INPUT_ACTION_RE.search(line):
            continue

        context = _context(lines, index)
        if not TECHNICAL_RE.search(context):
            continue
        takeover = _looks_like_action_control(line, context, source)
        if SYSTEM_OWNED_RE.search(line) and TYPED_USER_WAIT_RE.search(line):
            continue
        if SYSTEM_OWNED_RE.search(context) and not takeover:
            continue
        if not takeover:
            continue

        excerpt = line.strip()
        if len(excerpt) > 220:
            excerpt = excerpt[:217] + "..."
        violations.append(
            Violation(
                code=VIOLATION_CODE,
                path=normalised,
                line=index + 1,
                message=(
                    f"system-owned technical state requires manual '{action.group(0)}'; "
                    "project it as typed objective recovery and auto-resume after safe input"
                ),
                excerpt=excerpt,
            )
        )

    return violations


def validate_paths(
    repo_root: Path,
    paths: Iterable[str | Path],
    relevant_lines: dict[str, set[int]] | None = None,
) -> list[Violation]:
    violations: list[Violation] = []
    for raw_path in paths:
        relative = _normalise_path(raw_path)
        if not _is_protected_path(relative):
            continue
        target = repo_root / relative
        if not target.is_file():
            continue
        file_violations = validate_text(
            relative, target.read_text(encoding="utf-8", errors="replace")
        )
        if relevant_lines is not None:
            added = relevant_lines.get(relative, set())
            file_violations = [
                violation
                for violation in file_violations
                if any(abs(violation.line - line) <= 4 for line in added)
            ]
        violations.extend(file_violations)
    return violations


def _git(*args: str, repo_root: Path) -> str | None:
    try:
        result = subprocess.run(
            ["git", *args],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return result.stdout.strip()


def changed_files(repo_root: Path, base: str | None = None) -> list[str] | None:
    resolved_base = (base or os.environ.get("GOVERNANCE_BASE_SHA", "")).strip()
    if not resolved_base:
        resolved_base = _git("rev-parse", "--verify", "--quiet", "origin/main", repo_root=repo_root) or ""
    if not resolved_base:
        return None
    output = _git("diff", "--name-only", f"{resolved_base}...HEAD", repo_root=repo_root)
    if output is None:
        return None
    return [line.strip() for line in output.splitlines() if line.strip()]


def _parse_added_line_numbers(diff: str) -> dict[str, set[int]]:
    """Return new-file line numbers from a zero-context unified diff."""

    result: dict[str, set[int]] = {}
    current_path: str | None = None
    for raw_line in diff.splitlines():
        if raw_line.startswith("+++ "):
            marker = raw_line[4:].strip()
            if marker == "/dev/null":
                current_path = None
            else:
                current_path = marker[2:] if marker.startswith("b/") else marker
                current_path = _normalise_path(current_path)
                result.setdefault(current_path, set())
            continue
        hunk = DIFF_HUNK_RE.match(raw_line)
        if not hunk or current_path is None:
            continue
        start = int(hunk.group("start"))
        count = int(hunk.group("count") or "1")
        if count:
            result[current_path].update(range(start, start + count))
    return result


def changed_line_numbers(
    repo_root: Path, base: str | None = None
) -> dict[str, set[int]] | None:
    resolved_base = (base or os.environ.get("GOVERNANCE_BASE_SHA", "")).strip()
    if not resolved_base:
        resolved_base = _git("rev-parse", "--verify", "--quiet", "origin/main", repo_root=repo_root) or ""
    if not resolved_base:
        return None
    diff = _git(
        "diff",
        "--unified=0",
        "--diff-filter=ACMR",
        f"{resolved_base}...HEAD",
        "--",
        repo_root=repo_root,
    )
    if diff is None:
        return None
    return _parse_added_line_numbers(diff)


def validate_changed_paths(
    repo_root: Path, base: str | None = None
) -> list[Violation] | None:
    line_numbers = changed_line_numbers(repo_root, base)
    if line_numbers is None:
        return None
    return validate_paths(repo_root, line_numbers, relevant_lines=line_numbers)


def _all_protected_files(repo_root: Path) -> list[str]:
    paths: list[str] = []
    for prefix in (*DOC_PREFIXES, *SOURCE_PREFIXES):
        base = repo_root / prefix
        if not base.exists():
            continue
        for target in base.rglob("*"):
            if target.is_file():
                relative = target.relative_to(repo_root).as_posix()
                if _is_protected_path(relative):
                    paths.append(relative)
    return sorted(set(paths))


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", help="repo-relative files to validate")
    parser.add_argument("--base", help="base SHA/ref for changed-file validation")
    parser.add_argument("--all", action="store_true", help="scan every protected product/spec file")
    args = parser.parse_args(argv)

    if args.paths and args.all:
        parser.error("paths and --all are mutually exclusive")
    if args.paths:
        paths: list[str] | None = list(args.paths)
    elif args.all:
        paths = _all_protected_files(REPO_ROOT)
    else:
        changed = changed_line_numbers(REPO_ROOT, args.base)
        paths = list(changed) if changed is not None else None
    if paths is None:
        print("::error::objective non-interruption contract: unable to determine changed files")
        return 2

    if args.paths or args.all:
        violations = validate_paths(REPO_ROOT, paths)
    else:
        violations = validate_paths(REPO_ROOT, paths, relevant_lines=changed)
    if violations:
        print(f"objective-non-interruption-contract: {len(violations)} violation(s)")
        for violation in violations:
            print(
                f"::error file={violation.path},line={violation.line}::"
                f"{violation.code}: {violation.message} [{violation.excerpt}]"
            )
        return 1

    checked = sum(1 for path in paths if _is_protected_path(_normalise_path(path)))
    print(f"objective-non-interruption-contract: OK ({checked} protected file(s) checked)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
