from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


class ReleaseWorkflowTests(unittest.TestCase):
    def _git(self, repo: Path, *args: str) -> None:
        subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )

    def _new_release_history(self) -> Path:
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        repo = Path(temp_dir.name)
        self._git(repo, "init", "-b", "main")
        self._git(repo, "config", "user.name", "Release Test")
        self._git(repo, "config", "user.email", "release-test@example.invalid")
        self._git(repo, "commit", "--allow-empty", "-m", "chore: initial")
        self._git(repo, "tag", "v1.0.0")
        return repo

    def _commit(self, repo: Path, subject: str, body: str = "") -> None:
        args = ["commit", "--allow-empty", "-m", subject]
        if body:
            args.extend(["-m", body])
        self._git(repo, *args)

    def _release_plan(self, repo: Path, *args: str) -> dict[str, object]:
        result = subprocess.run(
            [
                "python3",
                str(REPO_ROOT / "tools/release/plan_release.py"),
                "--repo",
                str(repo),
                "--range",
                "v1.0.0..HEAD",
                *args,
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads(result.stdout)

    def test_adaptive_release_contract_is_auditable_and_never_push_triggered(
        self,
    ) -> None:
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )
        principle = (REPO_ROOT / "docs/principles/release-cadence.md").read_text(
            encoding="utf-8"
        )
        agents = (REPO_ROOT / "AGENTS.md").read_text(encoding="utf-8")

        trigger = auto_release.split("permissions:", 1)[0]
        self.assertNotIn("push:", trigger)
        self.assertIn("Release-Urgency: immediate", auto_release)
        self.assertIn("Release-Urgency: hold", auto_release)
        self.assertIn("allow_guarded_batch:", auto_release)
        self.assertIn("tools/release/plan_release.py", auto_release)
        self.assertIn("Release-Urgency", agents)
        self.assertIn(
            "A `hold` remains active for the whole unreleased batch",
            principle,
        )

    def test_force_cannot_bypass_a_guarded_batch(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: restore delivery",
            "Release-Urgency: immediate",
        )
        self._commit(
            repo,
            "feat: prepare staged rollout",
            "Release-Urgency: hold",
        )

        default_plan = self._release_plan(repo)
        forced_plan = self._release_plan(repo, "--force", "true")
        reviewed_plan = self._release_plan(
            repo,
            "--allow-guarded-batch",
            "true",
        )

        self.assertEqual(default_plan["slot"], "minor")
        self.assertEqual(default_plan["immediate"], 1)
        self.assertEqual(default_plan["hold"], 1)
        self.assertTrue(default_plan["skip"])
        self.assertTrue(forced_plan["skip"])
        self.assertFalse(reviewed_plan["skip"])

    def test_invalid_urgency_fails_closed_but_prose_does_not(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: clarify release notes",
            "This prose mentions Release-Urgency: hold but is not a trailer.",
        )
        prose_plan = self._release_plan(repo)
        self.assertFalse(prose_plan["skip"])
        self.assertEqual(prose_plan["hold"], 0)

        self._commit(
            repo,
            "fix: malformed urgency",
            "Release-Urgency: soon",
        )
        invalid_plan = self._release_plan(repo)
        self.assertTrue(invalid_plan["skip"])
        self.assertEqual(invalid_plan["invalid_urgency"], 1)

    def test_force_only_overrides_the_no_feat_or_fix_rule(self) -> None:
        repo = self._new_release_history()
        self._commit(repo, "docs: explain cadence")

        default_plan = self._release_plan(repo)
        forced_plan = self._release_plan(repo, "--force", "true")

        self.assertTrue(default_plan["skip"])
        self.assertEqual(default_plan["slot"], "none")
        self.assertFalse(forced_plan["skip"])
        self.assertEqual(forced_plan["slot"], "patch")

    def test_breaking_footer_selects_a_major_release(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: change persisted format",
            "BREAKING CHANGE: old databases require migration",
        )

        plan = self._release_plan(repo)

        self.assertFalse(plan["skip"])
        self.assertEqual(plan["slot"], "major")

        hyphenated_repo = self._new_release_history()
        self._commit(
            hyphenated_repo,
            "fix: change wire format",
            "BREAKING-CHANGE: old clients require migration",
        )
        hyphenated_plan = self._release_plan(hyphenated_repo)
        self.assertFalse(hyphenated_plan["skip"])
        self.assertEqual(hyphenated_plan["slot"], "major")

    def test_squash_footer_with_breaking_and_urgencies_stays_major(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: change persisted format",
            "Reviewed migration.\n\n"
            "BREAKING CHANGE: old databases require migration\n"
            "Release-Urgency: hold\n"
            "Release-Urgency: immediate",
        )

        guarded_plan = self._release_plan(repo)
        reviewed_plan = self._release_plan(
            repo,
            "--allow-guarded-batch",
            "true",
        )

        self.assertEqual(guarded_plan["slot"], "major")
        self.assertEqual(guarded_plan["hold"], 1)
        self.assertEqual(guarded_plan["immediate"], 1)
        self.assertTrue(guarded_plan["skip"])
        self.assertEqual(reviewed_plan["slot"], "major")
        self.assertFalse(reviewed_plan["skip"])

    def test_auto_release_dispatches_tag_from_main_for_shared_cache_scope(self) -> None:
        release = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )

        trigger = release.split("permissions:", 1)[0]
        self.assertIn("workflow_dispatch:", trigger)
        self.assertIn("tag:", trigger)
        self.assertNotIn("push:", trigger)
        self.assertIn("actions: write", auto_release)
        self.assertIn(
            'gh workflow run release.yml --ref main -f tag="$TAG"', auto_release
        )

    def test_release_prepares_one_draft_then_builds_platforms_in_parallel(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("prepare-release:", workflow)
        self.assertEqual(
            workflow.count("needs: [changelog, prepare-release]"),
            2,
            "Windows and macOS must share the same prerequisites",
        )
        self.assertIn(
            "needs: [changelog, build-windows, build-macos]", workflow
        )
        self.assertIn("gh release create", workflow)
        self.assertIn("--draft", workflow)
        self.assertIn("releases?per_page=100", workflow)
        self.assertIn("| .draft", workflow)
        self.assertEqual(workflow.count("includeUpdaterJson: false"), 2)

    def test_release_builds_the_requested_tag_not_the_dispatch_ref(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        self.assertGreaterEqual(workflow.count("ref: ${{ inputs.tag }}"), 5)
        self.assertIn(
            "CODEFACTORY_BUILD_GIT_SHA: "
            "${{ needs.prepare-release.outputs.tag_sha }}",
            workflow,
        )
        self.assertNotIn("github.ref_name", workflow)
        self.assertNotIn("GITHUB_REF_NAME", workflow)

    def test_release_automatically_reverifies_the_published_macos_asset(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("verify-published-macos:", workflow)
        job = workflow.split("verify-published-macos:", 1)[1]
        self.assertIn("needs: finalize", job)
        self.assertIn("runs-on: macos-latest", job)
        self.assertNotIn("GH_TOKEN:", job)
        self.assertIn("env -u GH_TOKEN curl", job)
        self.assertIn("--retry 4", job)
        self.assertIn("--retry-all-errors", job)
        self.assertIn("scripts/verify-macos-release-artifact.sh", job)
        self.assertIn('tee "$CODEFACTORY_RELEASE_EVIDENCE_DIR/verification.log"', job)
        self.assertIn("if: always()", job)
        self.assertIn("actions/upload-artifact@v4", job)

        ci_workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("python -m unittest tests.test_release_workflow", ci_workflow)

    def test_ci_runs_agent_bridge_and_evaluation_tests_on_linux(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("agent-bridge-linux:", workflow)
        job = workflow.split("agent-bridge-linux:", 1)[1]
        self.assertIn("runs-on: ubuntu-latest", job)
        self.assertIn("python-version: '3.12'", job)
        self.assertIn("harbor==0.15.0", job)
        self.assertIn("python -m unittest discover -s tests -p 'test_*.py'", job)


if __name__ == "__main__":
    unittest.main()
