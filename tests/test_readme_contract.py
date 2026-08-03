from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

from unittest import mock

from tools.governance import validate_readme_contract as contract
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


def _write(text: str) -> Path:
    temp = tempfile.NamedTemporaryFile(  # noqa: SIM115 - kept alive by the caller's assertion
        "w", suffix="README.md", delete=False, encoding="utf-8"
    )
    temp.write(text)
    temp.close()
    return Path(temp.name)


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

    # A toolchain pin is exactly what "Build from source" is supposed to
    # document, and the gate already knows how to ignore code — it strips
    # fenced blocks before reading the PR decision. Not applying the same
    # treatment to the version scan turned normal build docs into a CI failure,
    # with advice ("versions belong in Release notes") that is wrong for a
    # toolchain pin. Verified against the real validator on 2026-08-03.
    def test_versions_inside_code_are_documentation_not_release_claims(self) -> None:
        fenced = BASE_README.replace(
            "Build locally.",
            "```bash\nrustup toolchain install 1.83.0\nnode --version   # v20.11.0\n```",
        )
        self.assertEqual(
            [e for e in validate_static(REPO_ROOT, _write(fenced)) if "exact version" in e],
            [],
            "a fenced build command must not read as a release version claim",
        )

        inline = BASE_README.replace("Build locally.", "Requires Rust `1.83.0` or newer.")
        self.assertEqual(
            [e for e in validate_static(REPO_ROOT, _write(inline)) if "exact version" in e],
            [],
            "an inline-code toolchain pin must not read as a release version claim",
        )

    def test_prose_versions_are_still_rejected(self) -> None:
        # The rule itself must survive: an unquoted version in prose is the
        # stale-README problem this gate exists to prevent.
        prose = BASE_README.replace("Stable features.", "Download CodeFactory 1.77.0 now.")
        self.assertTrue(
            any("exact version" in e for e in validate_static(REPO_ROOT, _write(prose))),
            "a bare version in prose must still fail",
        )
        # …including one that merely sits next to code.
        mixed = BASE_README.replace(
            "Stable features.", "Version 1.77.0 ships with `cargo build`."
        )
        self.assertTrue(
            any("exact version" in e for e in validate_static(REPO_ROOT, _write(mixed))),
            "stripping code must not swallow the surrounding prose",
        )

    # The event payload is a snapshot from when the run was triggered, so a
    # re-run replays the ORIGINAL body. On 2026-08-03 that pinned PR #301
    # (the v1.77.0 version bump) in a permanent failure: the body was corrected
    # immediately, but three re-runs all re-read the stale payload and the only
    # escape was pushing a commit. A required check must judge the PR as it
    # stands, which is also what a reviewer sees.
    def test_live_pr_body_wins_over_the_stale_event_snapshot(self) -> None:
        with mock.patch.object(contract, "subprocess") as sp:
            sp.run.return_value = mock.Mock(returncode=0, stdout="README-Update: reviewed\n", stderr="")
            self.assertEqual(contract._live_pr_body(301), "README-Update: reviewed\n")
            sp.run.assert_called_once()
            self.assertIn("301", sp.run.call_args[0][0])

    def test_live_body_lookup_falls_back_instead_of_hard_failing(self) -> None:
        # No gh, no network, or a PR the token cannot read must not turn the
        # contract into an unconditional failure — fall back to the payload.
        with mock.patch.object(contract, "subprocess") as sp:
            sp.run.return_value = mock.Mock(returncode=1, stdout="", stderr="gh: not found")
            self.assertIsNone(contract._live_pr_body(301))
        self.assertIsNone(contract._live_pr_body(None))
        self.assertIsNone(contract._live_pr_body("not-a-number"))

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
