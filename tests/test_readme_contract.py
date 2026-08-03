from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

from tools.governance.validate_readme_contract import validate_pr_contract, validate_static


REPO_ROOT = Path(__file__).resolve().parents[1]

BASE_README = """<!-- README-CONTRACT: evergreen -->

# Example

## Features

Stable features.

## Install

See [install notes](docs/install.md).

## Quick start

Start the app.

## Build from source

Build locally.

## Data & privacy

Data stays local.

## Architecture

Desktop app.

## License

Apache-2.0.

[Latest releases](https://github.com/example/example/releases/latest)
"""


class ReadmeContractTests(unittest.TestCase):
    def _git(self, repo: Path, *args: str) -> str:
        result = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()

    def _new_repo(self, readme: str = BASE_README) -> tuple[tempfile.TemporaryDirectory[str], Path, str]:
        temp_dir: tempfile.TemporaryDirectory[str] = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        repo = Path(temp_dir.name)
        self._git(repo, "init", "-b", "main")
        self._git(repo, "config", "user.name", "README Test")
        self._git(repo, "config", "user.email", "readme-test@example.invalid")
        (repo / "README.md").write_text(readme, encoding="utf-8")
        (repo / "docs").mkdir()
        (repo / "docs" / "install.md").write_text("install", encoding="utf-8")
        self._git(repo, "add", ".")
        self._git(repo, "commit", "-m", "chore: initial")
        base_sha = self._git(repo, "rev-parse", "HEAD")
        return temp_dir, repo, base_sha

    def test_current_repository_readme_satisfies_static_contract(self) -> None:
        self.assertEqual(validate_static(REPO_ROOT), [])

    def test_required_change_must_include_readme_diff(self) -> None:
        _tmp, repo, base_sha = self._new_repo()
        (repo / "src.txt").write_text("feature", encoding="utf-8")
        self._git(repo, "add", "src.txt")
        self._git(repo, "commit", "-m", "feat: user visible change")

        errors = validate_pr_contract(
            repo,
            "README-Update: required\nREADME-Update-Reason: Adds a user-visible feature.",
            base_sha,
        )
        self.assertTrue(any("must change README.md" in error for error in errors))

        (repo / "README.md").write_text(BASE_README + "\nUpdated.\n", encoding="utf-8")
        self._git(repo, "add", "README.md")
        self._git(repo, "commit", "-m", "docs: describe the feature")
        self.assertEqual(
            validate_pr_contract(
                repo,
                "README-Update: required\nREADME-Update-Reason: Adds a user-visible feature.",
                base_sha,
            ),
            [],
        )

    def test_reviewed_change_can_leave_readme_untouched(self) -> None:
        _tmp, repo, base_sha = self._new_repo()
        (repo / "ci.txt").write_text("ci", encoding="utf-8")
        self._git(repo, "add", "ci.txt")
        self._git(repo, "commit", "-m", "ci: tighten checks")

        self.assertEqual(
            validate_pr_contract(
                repo,
                "README-Update: reviewed\nREADME-Update-Reason: Internal CI only; no product claim changed.",
                base_sha,
            ),
            [],
        )

    def test_machine_decision_and_reason_must_be_unambiguous(self) -> None:
        _tmp, repo, base_sha = self._new_repo()
        cases = (
            "README-Update-Reason: missing decision.",
            "README-Update: required\nREADME-Update: reviewed\nREADME-Update-Reason: duplicate decision.",
            "README-Update: reviewed\nREADME-Update-Reason: <explain here>",
            "```text\nREADME-Update: reviewed\nREADME-Update-Reason: hidden\n```",
        )
        for body in cases:
            with self.subTest(body=body):
                self.assertTrue(validate_pr_contract(repo, body, base_sha))

    def test_static_contract_rejects_exact_version_and_broken_link(self) -> None:
        _tmp, repo, _base_sha = self._new_repo(BASE_README.replace("Stable features.", "Released in v1.2.3."))
        errors = validate_static(repo)
        self.assertTrue(any("exact version" in error for error in errors))

        broken = BASE_README.replace("docs/install.md", "docs/missing.md")
        (repo / "README.md").write_text(broken, encoding="utf-8")
        errors = validate_static(repo)
        self.assertTrue(any("missing local link" in error for error in errors))

    def test_release_docs_point_to_governed_release_path(self) -> None:
        versioning = (REPO_ROOT / "VERSIONING.md").read_text(encoding="utf-8")
        development = (REPO_ROOT / "DEVELOPMENT.md").read_text(encoding="utf-8")
        for document in (versioning, development):
            self.assertIn(".github/workflows/auto-release.yml", document)
            self.assertIn("workflow_dispatch", document)
            self.assertIn("deliver_changes", document)
            self.assertNotIn("bump-version.ps1 [patch|minor|major]", document)

    def test_ci_and_release_workflows_declare_readme_contract(self) -> None:
        ci = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("README contract", ci)
        self.assertIn("validate_readme_contract.py --ci", ci)
        self.assertIn("README-Update: reviewed", auto_release)
        self.assertEqual(auto_release.count('--body "$PR_BODY"'), 2)

    def test_template_and_monthly_review_do_not_auto_edit_readme(self) -> None:
        template = (REPO_ROOT / ".github/pull_request_template.md").read_text(
            encoding="utf-8"
        )
        review = (REPO_ROOT / ".github/workflows/readme-review.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("README-Update: reviewed", template)
        self.assertIn("README-Update-Reason:", template)
        self.assertIn('cron: "0 3 1 * *"', review)
        self.assertIn("gh issue create", review)
        self.assertNotIn("git push", review)


if __name__ == "__main__":
    unittest.main()
