"""M223 requirement regressions: budgets and check-run identities."""
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

class ReleaseBudgetRequirements(unittest.TestCase):
    def test_guarded_wait_is_bounded_to_sixty_minutes(self):
        workflow = (ROOT / '.github/workflows/auto-release.yml').read_text()
        step = workflow.split('      - name: Wait for guarded version bump merge')[1].split('      - name: Tag guarded merge')[0]
        self.assertIn('timeout-minutes: 60', step)
        self.assertIn('MAX_INFRA_RETRIES=3', step)
        self.assertIn('RETRY_COUNTS=', step)
        self.assertNotIn('120 minutes', step)

    def test_annotations_use_rest_job_identity_not_graphql_node_id(self):
        workflow = (ROOT / '.github/workflows/auto-release.yml').read_text()
        self.assertIn('detailsUrl:', workflow)
        self.assertIn('jobs/(?<id>[0-9]+)', workflow)
        self.assertNotIn('id: (.id // "")', workflow)

    def test_merge_requires_classifier_green_not_six_unrelated_checks(self):
        workflow = (ROOT / '.github/workflows/auto-release.yml').read_text()
        self.assertIn('[ "$DECISION" = "merge" ]', workflow)

if __name__ == '__main__':
    unittest.main()
