"""M162 R4：规格逐条验收账本的回归用例。

每个用例对应一条真实漏做/假完成（2026-10-22 的 M145 R3、M155 R5）：
规格里写了 R 项，交付时没有任何机器判定它是否真的做到了。
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "tools" / "governance" / "validate_acceptance_ledger.py"

SPEC_TEXT = """# M999：演示规格

## 需求

- R1 交付前生成账本
- **R2 校验账本完整性**
- R3 跨进程 smoke

## 实现

| 需求 | 落点 |
| --- | --- |
| R1 | `tools/governance/x.py` |

## 约束

- R9 这一条在「约束」里，不是需求项，账本不需要它
"""

COMPLETE_LEDGER = """# M999 验收账本

| 需求 | 状态 | 证据 |
| --- | --- | --- |
| R1 | done | `tests/test_acceptance_ledger.py::test_check_complete_ledger_passes` |
| R2 | done | `tools/governance/validate_acceptance_ledger.py:1` |
| R3 | deferred | 原因：跨进程 smoke 依赖 CI runner | 后续：m170 |
"""


def load_module():
    spec = importlib.util.spec_from_file_location("validate_acceptance_ledger", MODULE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class AcceptanceLedgerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.module = load_module()
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        (self.root / "docs" / "specs" / "feature-specs").mkdir(parents=True)
        (self.root / "docs" / "acceptance").mkdir(parents=True)
        self.spec_path = self.root / "docs" / "specs" / "feature-specs" / "m999-demo.md"
        self.spec_path.write_text(SPEC_TEXT, encoding="utf-8")
        self.ledger_path = self.root / "docs" / "acceptance" / "m999.md"

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def check(self) -> list[str]:
        return self.module.check_spec(self.spec_path, self.ledger_path)

    # R1 —— 需求项只从「需求」章节的条目标题里来，正文/实现表里的 R 引用不算新需求。
    def test_parse_requirements_reads_需求_section_only(self) -> None:
        requirements = self.module.parse_requirements(SPEC_TEXT)
        self.assertEqual(["R1", "R2", "R3"], requirements)

    def test_parse_ledger_accepts_table_and_bullet_rows(self) -> None:
        entries = self.module.parse_ledger(COMPLETE_LEDGER)
        self.assertEqual("done", entries["R1"]["status"])
        self.assertIn("test_acceptance_ledger", entries["R1"]["evidence"])
        bullet = self.module.parse_ledger("- R4 | deferred | 原因：x 后续：m1\n")
        self.assertEqual("deferred", bullet["R4"]["status"])

    # 账本行可以顺带写需求标题（可读性），编号仍必须在首列行首。
    def test_ledger_row_may_carry_requirement_title(self) -> None:
        entries = self.module.parse_ledger(
            "| R1 交付前生成账本（每项一行） | done | `tools/x.py:1` |\n"
            "| **R2**：校验账本 | done | `tools/y.py:2` |\n"
        )
        self.assertEqual(["R1", "R2"], list(entries))
        self.assertEqual("done", entries["R2"]["status"])

    # R2 —— 账本缺失是硬失败，不是警告。
    def test_missing_ledger_is_a_problem(self) -> None:
        problems = self.check()
        self.assertTrue(problems)
        self.assertIn("m999", " ".join(problems))

    def test_complete_ledger_passes(self) -> None:
        self.ledger_path.write_text(COMPLETE_LEDGER, encoding="utf-8")
        self.assertEqual([], self.check())

    # R2 —— 漏做一项（M145 R3 的真实形态）：规格有 R 项、账本没这行。
    def test_requirement_missing_from_ledger_fails(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace("| R3 | deferred | 原因：跨进程 smoke 依赖 CI runner | 后续：m170 |\n", ""),
            encoding="utf-8",
        )
        problems = self.check()
        self.assertTrue(any("R3" in problem for problem in problems), problems)

    # R4 —— deferred 无原因要失败（M155 R5 的真实形态：只说“推迟”，不说为什么、谁接）。
    def test_deferred_without_reason_fails(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace("原因：跨进程 smoke 依赖 CI runner | 后续：m170", "等下次"),
            encoding="utf-8",
        )
        problems = self.check()
        self.assertTrue(any("R3" in problem and "deferred" in problem for problem in problems), problems)

    def test_deferred_without_followup_fails(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace(" | 后续：m170", ""),
            encoding="utf-8",
        )
        problems = self.check()
        self.assertTrue(any("R3" in problem for problem in problems), problems)

    # R4 —— done 必须带测试名或证据路径，不能只写“已完成”。
    def test_done_without_evidence_fails(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace(
                "`tests/test_acceptance_ledger.py::test_check_complete_ledger_passes`", "已完成"
            ),
            encoding="utf-8",
        )
        problems = self.check()
        self.assertTrue(any("R1" in problem for problem in problems), problems)

    def test_done_with_command_evidence_passes(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace(
                "`tests/test_acceptance_ledger.py::test_check_complete_ledger_passes`",
                "`cargo test --manifest-path src-tauri/Cargo.toml m162_ledger`",
            ),
            encoding="utf-8",
        )
        self.assertEqual([], self.check())

    def test_unknown_status_fails(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace("| R1 | done |", "| R1 | maybe |"), encoding="utf-8"
        )
        problems = self.check()
        self.assertTrue(any("R1" in problem for problem in problems), problems)

    def test_not_applicable_needs_a_reason(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace("| R1 | done | `tests/test_acceptance_ledger.py::test_check_complete_ledger_passes` |", "| R1 | n/a |  |"),
            encoding="utf-8",
        )
        problems = self.check()
        self.assertTrue(any("R1" in problem for problem in problems), problems)

    # R3 —— PR body 声明的规格路径要能被解析到对应账本。
    def test_ledger_path_derives_task_id_from_spec_name(self) -> None:
        self.assertEqual("m999", self.module.task_id_for_spec(Path("docs/specs/feature-specs/m999-demo.md")))
        self.assertEqual("m162", self.module.task_id_for_spec(Path("m162-requirement-acceptance-ledger.md")))
        self.assertEqual("demo-spec", self.module.task_id_for_spec(Path("docs/specs/feature-specs/demo-spec.md")))

    def test_pr_body_spec_reference_is_enforced(self) -> None:
        body = "## 总结\n\nSpec: `docs/specs/feature-specs/m999-demo.md`\n"
        problems = self.module.check_pr_acceptance_ledger(self.root, body)
        self.assertTrue(any("m999" in problem for problem in problems), problems)
        self.ledger_path.write_text(COMPLETE_LEDGER, encoding="utf-8")
        self.assertEqual([], self.module.check_pr_acceptance_ledger(self.root, body))

    def test_pr_body_declared_ledger_path_wins(self) -> None:
        custom = self.root / "docs" / "acceptance" / "custom-ledger.md"
        custom.write_text(COMPLETE_LEDGER, encoding="utf-8")
        body = (
            "Spec: `docs/specs/feature-specs/m999-demo.md`\n"
            "Acceptance-Ledger: `docs/acceptance/custom-ledger.md`\n"
        )
        self.assertEqual([], self.module.check_pr_acceptance_ledger(self.root, body))

    def test_body_without_spec_reference_has_nothing_to_check(self) -> None:
        self.assertEqual([], self.module.check_pr_acceptance_ledger(self.root, "只改了一个拼写\n"))

    # R4 —— CLI 契约：交付预检用 --body-file/--repo/--base-ref 调用它，失败必须非零退出。
    def test_cli_exit_code_is_fail_closed(self) -> None:
        body_file = self.root / "body.md"
        body_file.write_text("Spec: `docs/specs/feature-specs/m999-demo.md`\n", encoding="utf-8")
        command = [
            sys.executable,
            str(MODULE_PATH),
            "--repo",
            str(self.root),
            "--body-file",
            str(body_file),
        ]
        failed = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(1, failed.returncode, failed.stdout + failed.stderr)
        self.assertIn("m999", failed.stdout)

        self.ledger_path.write_text(COMPLETE_LEDGER, encoding="utf-8")
        passed = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(0, passed.returncode, passed.stdout + passed.stderr)
        self.assertEqual("pass", json.loads(passed.stdout)["status"])

    # 回归：曾经把 origin/main...HEAD 与过期本地 main...HEAD 一起并进变更集，
    # 结果几周前的规格被当成"本次改动"，每次交付都因旧账变红。
    def test_only_one_base_ref_is_consulted(self) -> None:
        calls: list[tuple[str, ...]] = []

        def fake_git(repo_root, *arguments):
            calls.append(arguments)
            if arguments[0] == "rev-parse":
                return "deadbeef\n"
            if arguments[:3] == ("diff", "--name-only", "origin/main...HEAD"):
                return "docs/specs/feature-specs/m999-demo.md\n"
            return ""

        original = self.module._git
        self.module._git = fake_git
        try:
            changed = self.module._git_changed_files(self.root, "main")
        finally:
            self.module._git = original

        self.assertEqual(["docs/specs/feature-specs/m999-demo.md"], changed)
        self.assertIn(("diff", "--name-only", "origin/main...HEAD"), calls)
        self.assertNotIn(("diff", "--name-only", "main...HEAD"), calls)

    def test_working_tree_edits_count_as_this_pr_even_before_commit(self) -> None:
        def fake_git(repo_root, *arguments):
            if arguments[0] == "rev-parse":
                return None
            if arguments[:3] == ("diff", "--name-only", "HEAD"):
                return "docs/specs/feature-specs/m999-demo.md\n"
            return ""

        original = self.module._git
        self.module._git = fake_git
        try:
            changed = self.module._git_changed_files(self.root, None)
        finally:
            self.module._git = original
        self.assertEqual(["docs/specs/feature-specs/m999-demo.md"], changed)

    def test_cli_without_body_or_base_ref_self_checks_existing_ledgers(self) -> None:
        self.ledger_path.write_text(
            COMPLETE_LEDGER.replace(
                "| R2 | done | `tools/governance/validate_acceptance_ledger.py:1` |",
                "| R2 | done | only words |",
            ),
            encoding="utf-8",
        )
        # This case explicitly exercises the no-PR-context route, even on CI.
        environment = os.environ.copy()
        environment.pop("GITHUB_EVENT_PATH", None)
        broken = subprocess.run(
            [sys.executable, str(MODULE_PATH), "--repo", str(self.root)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env=environment,
        )
        self.assertEqual(1, broken.returncode, broken.stdout + broken.stderr)
        self.assertEqual(["existing_ledgers"], json.loads(broken.stdout)["checked"])


class DeliveryPrecheckWiringTest(unittest.TestCase):
    """R3：交付工具开 PR 前就用同一条 M156 预检通道跑账本校验。"""

    def test_precheck_checks_table_contains_ledger_validator(self) -> None:
        source = (REPO_ROOT / "src-tauri/src/agent/delivery.rs").read_text(encoding="utf-8")
        start = source.index("let checks = [")
        checks_table = source[start : source.index("];", start)]
        self.assertIn("tools/governance/validate_acceptance_ledger.py", checks_table)
        self.assertIn("验收账本", checks_table)

    def test_precheck_runs_before_push(self) -> None:
        source = (REPO_ROOT / "src-tauri/src/tools/delivery_thin.rs").read_text(encoding="utf-8")
        self.assertLess(
            source.index("commands.precheck_governance("),
            source.index('commands.git(&["push"'),
        )

    def test_baseline_wires_the_same_check(self) -> None:
        baseline = (REPO_ROOT / "tools/governance/validate_repo_governance_baseline.py").read_text(
            encoding="utf-8"
        )
        self.assertIn("validate_acceptance_ledger.py", baseline)
        self.assertIn("ACCEPTANCE_LEDGER_INCOMPLETE", baseline)

    def test_manifest_requires_the_ledger_validator(self) -> None:
        manifest = json.loads(
            (REPO_ROOT / ".codex/governance/baseline-manifest.json").read_text(encoding="utf-8")
        )
        self.assertIn("tools/governance/validate_acceptance_ledger.py", manifest["required_files"])


if __name__ == "__main__":
    unittest.main()
