from __future__ import annotations

import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


class ReleaseWorkflowTests(unittest.TestCase):
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
