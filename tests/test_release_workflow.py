from __future__ import annotations

import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


class ReleaseWorkflowTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
