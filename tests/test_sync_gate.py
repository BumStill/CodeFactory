from __future__ import annotations

import subprocess
import unittest
from unittest.mock import patch

from tools.git.ensure_branch_current import includes_base


class SyncGateTests(unittest.TestCase):
    @patch("tools.git.ensure_branch_current.git")
    def test_active_merge_accepts_base_from_merge_head(self, git_mock) -> None:
        git_mock.side_effect = [
            subprocess.CompletedProcess([], 1, "", ""),
            subprocess.CompletedProcess([], 0, "MERGE_HEAD\n", ""),
            subprocess.CompletedProcess([], 0, "", ""),
        ]

        self.assertTrue(includes_base("origin/main"))
        self.assertEqual(
            git_mock.call_args_list[-1].args,
            ("merge-base", "--is-ancestor", "origin/main", "MERGE_HEAD"),
        )

    @patch("tools.git.ensure_branch_current.git")
    def test_normal_commit_still_requires_head_to_include_base(self, git_mock) -> None:
        git_mock.side_effect = [
            subprocess.CompletedProcess([], 1, "", ""),
            subprocess.CompletedProcess([], 1, "", ""),
        ]

        self.assertFalse(includes_base("origin/main"))


if __name__ == "__main__":
    unittest.main()
