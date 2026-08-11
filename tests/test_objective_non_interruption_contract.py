from __future__ import annotations

import unittest
import subprocess
import tempfile
from pathlib import Path

from tools.governance.validate_objective_non_interruption_contract import (
    _parse_added_line_numbers,
    validate_changed_paths,
    validate_text,
)


REPO_ROOT = Path(__file__).resolve().parents[1]


class ObjectiveNonInterruptionContractTests(unittest.TestCase):
    def _git(self, repo: Path, *args: str) -> str:
        result = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()

    def assert_rejected(self, path: str, text: str) -> None:
        violations = validate_text(path, text)
        self.assertTrue(violations, f"expected {path} to be rejected")
        self.assertEqual(violations[0].code, "OBJECTIVE_MANUAL_TECHNICAL_HANDOFF")

    def assert_allowed(self, path: str, text: str) -> None:
        self.assertEqual(validate_text(path, text), [], path)

    def test_normative_specs_cannot_require_manual_technical_recovery(self) -> None:
        cases = (
            "技术失败后显示“重试失败步骤”按钮，用户点击后继续。",
            "Provider 候选耗尽后提供 [继续执行]，等待用户恢复任务。",
            "授权通道超时后，请重新发送需要继续的内容。",
            "CI 失败时显示“回到对话处理”，由用户触发下一轮。",
            "运行环境错误后要求用户回复继续。",
        )
        for contract in cases:
            with self.subTest(contract=contract):
                self.assert_rejected(
                    "docs/specs/feature-specs/example.md",
                    f"# Example\n\n## Requirements Traceability\n\n- {contract}\n",
                )

    def test_ux_design_cannot_reintroduce_a_technical_takeover_button(self) -> None:
        self.assert_rejected(
            "docs/design/example-ux-design.md",
            """# UX

## 权限超时

权限通道失败后显示：

```text
[已修复，重试] [回到对话处理]
```
""",
        )

    def test_production_ui_and_backend_copy_are_checked(self) -> None:
        self.assert_rejected(
            "src/components/FailureCard.tsx",
            """export function FailureCard({ error }: Props) {
  return error ? <button aria-label="重试失败步骤">重试</button> : null;
}
""",
        )
        self.assert_rejected(
            "src-tauri/src/agent/recovery.rs",
            'let message = "模型服务失败，请回复继续执行";\n',
        )
        self.assert_rejected(
            "src/pages/StatusPage.tsx",
            'throw new Error("控制面请求超时，请重试");\n',
        )

    def test_problem_statements_and_forbidden_copy_examples_are_allowed(self) -> None:
        self.assert_allowed(
            "docs/design/example-business-design.md",
            """# Design

## 问题

旧实现会在技术失败后显示“重试失败步骤”，让用户承担调度职责。
""",
        )
        self.assert_allowed(
            "docs/design/example-ux-design.md",
            """# UX

## 文案禁区

- “回复继续”“继续执行”；
- “重新发送需要继续的内容”；
- “回到对话处理”。
""",
        )
        self.assert_allowed(
            "docs/specs/feature-specs/example.md",
            """# Spec

## Requirements Traceability

- 技术失败不得显示“重试”按钮，也不要求用户回复继续。
""",
        )

    def test_system_owned_recovery_and_normal_user_steer_are_allowed(self) -> None:
        self.assert_allowed(
            "docs/specs/feature-specs/example.md",
            """# Spec

## Primary User Path

Provider 超时后系统自动重试并继续执行，无需用户操作。
""",
        )
        self.assert_allowed(
            "src/components/ObjectiveProgress.tsx",
            'const status = error ? "系统正在继续处理" : "运行中";\n',
        )
        self.assert_allowed(
            "src-tauri/src/http_util.rs",
            'Err(AppError::Other(format!("{label} retry budget exhausted")))\n',
        )
        self.assert_allowed(
            "docs/specs/feature-specs/example.md",
            """# Spec

## Primary User Path

目标空闲时，用户可以继续输入新的分析要求，形成新的 steer。
""",
        )

    def test_comments_tests_and_unprotected_paths_are_not_product_copy(self) -> None:
        self.assert_allowed(
            "src/components/FailureCard.tsx",
            "// Regression: the old UI rendered a 重试失败步骤 button.\nexport const ok = true;\n",
        )
        self.assert_allowed(
            "src/components/FailureCard.test.tsx",
            'expect(screen.queryByText("重试失败步骤")).not.toBeInTheDocument();\n',
        )
        self.assert_allowed(
            "docs/long-tasks/historical.md",
            "历史版本曾要求用户点击重试失败步骤。\n",
        )

    def test_forbidden_markdown_table_column_is_descriptive_not_normative(self) -> None:
        self.assert_allowed(
            "docs/design/example-ux-design.md",
            """# UX

## 状态

| state | expected | 禁止 |
| --- | --- | --- |
| failed | system owner + next observation | 人工技术重试、回到对话处理 |
""",
        )

    def test_zero_context_diff_parser_tracks_only_added_lines(self) -> None:
        parsed = _parse_added_line_numbers(
            """diff --git a/src/a.tsx b/src/a.tsx
--- a/src/a.tsx
+++ b/src/a.tsx
@@ -2,0 +3,2 @@
+one
+two
diff --git a/docs/specs/x.md b/docs/specs/x.md
--- /dev/null
+++ b/docs/specs/x.md
@@ -0,0 +1,3 @@
+a
+b
+c
"""
        )
        self.assertEqual(parsed["src/a.tsx"], {3, 4})
        self.assertEqual(parsed["docs/specs/x.md"], {1, 2, 3})

    def test_changed_line_gate_ratchets_without_blocking_untouched_legacy_copy(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            self._git(repo, "init", "-b", "main")
            self._git(repo, "config", "user.name", "Contract Test")
            self._git(repo, "config", "user.email", "contract@example.invalid")
            target = repo / "docs/specs/feature-specs/example.md"
            target.parent.mkdir(parents=True)
            target.write_text(
                "# Spec\n\n## Requirements Traceability\n\n"
                "技术失败后显示 [继续执行]。\n" + "\n" * 12,
                encoding="utf-8",
            )
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-m", "initial legacy contract")
            base = self._git(repo, "rev-parse", "HEAD")

            target.write_text(
                target.read_text(encoding="utf-8") + "系统自动恢复，无需用户操作。\n",
                encoding="utf-8",
            )
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-m", "safe unrelated update")
            self.assertEqual(validate_changed_paths(repo, base), [])

            target.write_text(
                target.read_text(encoding="utf-8")
                + "Provider 超时后请点击 [重试失败步骤]。\n",
                encoding="utf-8",
            )
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-m", "reintroduce manual takeover")
            violations = validate_changed_paths(repo, base)
            self.assertIsNotNone(violations)
            self.assertEqual(len(violations or []), 1)
            self.assertIn("重试失败步骤", (violations or [])[0].excerpt)

    def test_governance_checker_executes_the_manifest_rule_in_ci(self) -> None:
        checker = (REPO_ROOT / "tools/governance/check_governance_rules.py").read_text(
            encoding="utf-8"
        )
        workflow = (REPO_ROOT / ".github/workflows/governance-baseline.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("check_objective_non_interruption_contract", checker)
        self.assertIn('rid == "objective-non-interruption-contract"', checker)
        self.assertIn("check_governance_rules.py", workflow)
        self.assertIn("github.event.before", workflow)


if __name__ == "__main__":
    unittest.main()
