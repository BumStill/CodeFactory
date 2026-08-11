from __future__ import annotations

import copy
import json
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

import tools.governance.manage_main_branch_ruleset as main_gate
from tools.governance.manage_main_branch_ruleset import (
    apply_policy,
    build_ruleset_payload,
    contains,
    gh_api,
    validate_policy,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
POLICY_PATH = REPO_ROOT / ".github" / "rulesets" / "main.json"
EXPECTED_CHECKS = {
    "agent-bridge-linux",
    "check-frontend",
    "check-rust",
    "governance-baseline",
    "remote-real-app-gui",
}
GITHUB_ACTIONS_APP_ID = 15368


class GitHubMainGateTests(unittest.TestCase):
    def _policy(self) -> dict[str, object]:
        return json.loads(POLICY_PATH.read_text(encoding="utf-8"))

    def test_policy_is_safe_for_solo_maintainer_and_has_no_bypass(self) -> None:
        policy = self._policy()

        self.assertEqual(validate_policy(policy), [])
        self.assertTrue(policy["repository_settings"]["allow_auto_merge"])
        self.assertTrue(policy["cleanup"]["remove_classic_review_requirement"])

        ruleset = build_ruleset_payload(policy)
        self.assertEqual(ruleset["enforcement"], "active")
        self.assertEqual(
            ruleset["conditions"]["ref_name"]["include"], ["~DEFAULT_BRANCH"]
        )
        self.assertEqual(ruleset["bypass_actors"], [])

        by_type = {rule["type"]: rule for rule in ruleset["rules"]}
        self.assertIn("deletion", by_type)
        self.assertIn("non_fast_forward", by_type)

        pull_request = by_type["pull_request"]["parameters"]
        self.assertEqual(pull_request["required_approving_review_count"], 0)
        self.assertTrue(pull_request["required_review_thread_resolution"])
        self.assertEqual(pull_request["allowed_merge_methods"], ["squash"])

        status = by_type["required_status_checks"]["parameters"]
        self.assertTrue(status["strict_required_status_checks_policy"])
        self.assertEqual(
            {
                item["context"]
                for item in status["required_status_checks"]
            },
            EXPECTED_CHECKS,
        )
        self.assertEqual(
            {
                item["integration_id"]
                for item in status["required_status_checks"]
            },
            {GITHUB_ACTIONS_APP_ID},
        )

    def test_workflows_do_not_recreate_bypass_or_duplicate_check_contexts(self) -> None:
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )
        governance = (
            REPO_ROOT / ".github/workflows/governance-baseline.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("token: ${{ secrets.RELEASE_PAT }}", auto_release)
        self.assertIn("contents: read", auto_release)
        self.assertIn("actions: write", auto_release)
        self.assertIn("group: auto-release", auto_release)
        self.assertIn("cancel-in-progress: false", auto_release)
        self.assertIn("release-token-preflight:", auto_release)
        self.assertNotIn('git push origin "main"', auto_release)
        self.assertNotIn("git push origin main", auto_release)
        self.assertNotIn("refs/heads/main", auto_release)
        self.assertIn("gh pr create", auto_release)
        self.assertIn("gh pr merge", auto_release)
        self.assertIn("--auto", auto_release)
        self.assertNotIn("--admin", auto_release)
        self.assertIn("--match-head-commit", auto_release)
        self.assertNotIn("gh pr update-branch", auto_release)
        self.assertIn("automation/release-next", auto_release)
        self.assertIn("Quiesce open version PR", auto_release)
        self.assertIn("Reconcile interrupted version release", auto_release)
        self.assertIn("--disable-auto", auto_release)
        self.assertNotIn("--disable-auto || true", auto_release)
        self.assertIn("autoMergeRequest", auto_release)
        self.assertIn("Version PR auto-merge is still enabled", auto_release)
        self.assertIn("EXPECTED_HEAD", auto_release)
        for check in EXPECTED_CHECKS:
            self.assertIn(check, auto_release)
        self.assertIn("steps.reconcile.outputs.recovered != 'true'", auto_release)
        self.assertIn("gh release view", auto_release)
        self.assertIn("displayTitle", auto_release)
        self.assertIn("Recovered missing tag", auto_release)
        self.assertIn("Recovered missing Release dispatch", auto_release)
        self.assertIn("requested|waiting|pending|queued|in_progress", auto_release)
        self.assertLess(
            auto_release.index("Quiesce open version PR"),
            auto_release.index("Reconcile interrupted version release"),
        )
        self.assertLess(
            auto_release.index("Reconcile interrupted version release"),
            auto_release.index("Determine version slot from commits since last tag"),
        )
        self.assertIn("merge_sha=", auto_release)
        self.assertIn('git tag "$TAG" "$MERGE_SHA"', auto_release)
        wait_step = auto_release.split("Wait for guarded version bump merge", 1)[1]
        self.assertLess(
            wait_step.index("ACTUAL_HEAD="),
            wait_step.index('if [ "$STATE" = "MERGED" ]'),
        )
        tag_step = auto_release.split("Tag guarded merge and dispatch release build", 1)[1]
        self.assertIn('EXPECTED_VERSION="${TAG#v}"', tag_step)
        self.assertIn("Tag candidate version manifests do not match", tag_step)
        self.assertIn("Tag candidate changed files outside", tag_step)
        self.assertLess(
            auto_release.index("Wait for guarded version bump merge"),
            auto_release.rindex('git tag "$TAG" "$MERGE_SHA"'),
        )
        self.assertIn(
            'gh workflow run release.yml --ref main -f tag="$TAG"',
            auto_release,
        )
        self.assertIn("pull_request:\n    branches: [main]", governance)
        self.assertIn("push:\n    branches: [main]", governance)

        release = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("run-name: Release ${{ inputs.tag }}", release)

    def test_policy_cannot_exclude_default_branch_or_add_unknown_rules(self) -> None:
        excluded = copy.deepcopy(self._policy())
        excluded["ruleset"]["conditions"]["ref_name"]["exclude"] = [
            "~DEFAULT_BRANCH"
        ]
        self.assertIn(
            "ruleset must not exclude any ref",
            validate_policy(excluded),
        )

        extra_rule = copy.deepcopy(self._policy())
        extra_rule["ruleset"]["rules"].append({"type": "update"})
        self.assertIn(
            "rules must be exactly deletion, non_fast_forward, pull_request, required_status_checks",
            validate_policy(extra_rule),
        )

        bypass = copy.deepcopy(self._policy())
        bypass["ruleset"]["bypass_actors"] = [
            {
                "actor_id": GITHUB_ACTIONS_APP_ID,
                "actor_type": "Integration",
                "bypass_mode": "always",
            }
        ]
        self.assertIn(
            "ruleset bypass actors must be empty",
            validate_policy(bypass),
        )

    @patch("tools.governance.manage_main_branch_ruleset.inspect_live")
    @patch("tools.governance.manage_main_branch_ruleset.find_ruleset")
    @patch("tools.governance.manage_main_branch_ruleset.gh_api")
    def test_apply_stops_before_cleanup_when_ruleset_readback_does_not_match(
        self, api, find_ruleset, inspect_live
    ) -> None:
        policy = self._policy()
        mismatched = build_ruleset_payload(policy)
        mismatched["id"] = 91
        mismatched["enforcement"] = "evaluate"
        find_ruleset.side_effect = [None, mismatched]

        with self.assertRaisesRegex(RuntimeError, "read-back did not match"):
            apply_policy(policy)

        self.assertEqual(api.call_count, 1)
        self.assertEqual(api.call_args.kwargs["method"], "POST")
        inspect_live.assert_not_called()

    @patch("tools.governance.manage_main_branch_ruleset.inspect_live")
    @patch("tools.governance.manage_main_branch_ruleset.find_ruleset")
    @patch("tools.governance.manage_main_branch_ruleset.gh_api")
    def test_update_confirms_ruleset_before_removing_legacy_review(
        self, api, find_ruleset, inspect_live
    ) -> None:
        policy = self._policy()
        installed = build_ruleset_payload(policy)
        installed["id"] = 92
        find_ruleset.side_effect = [installed, installed]
        inspect_live.return_value = {
            "allow_auto_merge": True,
            "ruleset_matches": True,
            "classic_review_requirement_present": False,
        }

        apply_policy(policy)

        writes = [
            (call.args[1], call.kwargs.get("method", "GET"))
            for call in api.call_args_list
        ]
        self.assertEqual(
            writes,
            [
                ("rulesets/92", "PUT"),
                ("", "PATCH"),
                (
                    "branches/main/protection/required_pull_request_reviews",
                    "DELETE",
                ),
            ],
        )

    def test_server_added_fields_do_not_cause_false_drift(self) -> None:
        desired = build_ruleset_payload(self._policy())
        actual = copy.deepcopy(desired)
        actual["id"] = 93
        actual["rules"][0]["ruleset_source_type"] = "Repository"
        self.assertTrue(contains(actual, desired))

    @patch("tools.governance.manage_main_branch_ruleset.subprocess.run")
    def test_repository_root_api_does_not_add_a_trailing_slash(self, run) -> None:
        run.return_value = subprocess.CompletedProcess([], 0, '{"id": 1}', "")

        self.assertEqual(gh_api("BumStill/CodeFactory", ""), {"id": 1})
        self.assertEqual(
            run.call_args.args[0],
            ["gh", "api", "--method", "GET", "repos/BumStill/CodeFactory"],
        )

    @patch("tools.governance.manage_main_branch_ruleset.subprocess.run")
    def test_optional_classic_cleanup_is_idempotent_on_404(self, run) -> None:
        run.return_value = subprocess.CompletedProcess([], 1, "", "gh: Not Found (HTTP 404)")

        self.assertIsNone(
            gh_api(
                "BumStill/CodeFactory",
                "branches/main/protection/required_pull_request_reviews",
                method="DELETE",
                allow_not_found=True,
            )
        )

    @patch("tools.governance.manage_main_branch_ruleset.gh_graphql")
    def test_classic_review_verification_uses_uncached_graphql_state(
        self, graphql
    ) -> None:
        graphql.return_value = {
            "data": {
                "repository": {
                    "ref": {
                        "branchProtectionRule": {
                            "pattern": "main",
                            "requiresApprovingReviews": False,
                            "requiredApprovingReviewCount": None,
                        }
                    }
                }
            }
        }

        self.assertFalse(
            main_gate.classic_review_requirement_present("BumStill/CodeFactory")
        )

        graphql.return_value["data"]["repository"]["ref"][
            "branchProtectionRule"
        ] = {
            "pattern": "m*",
            "requiresApprovingReviews": True,
            "requiredApprovingReviewCount": 1,
        }
        self.assertTrue(
            main_gate.classic_review_requirement_present("BumStill/CodeFactory")
        )

        graphql.return_value["data"]["repository"]["ref"][
            "branchProtectionRule"
        ] = None
        self.assertFalse(
            main_gate.classic_review_requirement_present("BumStill/CodeFactory")
        )

        graphql.return_value = {"data": {"repository": {"ref": None}}}
        with self.assertRaisesRegex(RuntimeError, "response is incomplete"):
            main_gate.classic_review_requirement_present("BumStill/CodeFactory")


if __name__ == "__main__":
    unittest.main()
