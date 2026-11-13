# SPDX-License-Identifier: Apache-2.0
import unittest

from tools.governance.check_supply_chain import signing_errors, workflow_errors


class SupplyChainGovernanceTests(unittest.TestCase):
    def test_tag_ref_fails_and_full_sha_passes(self):
        tagged = "jobs:\n  example:\n    steps:\n      - uses: actions/checkout@v4\n"
        pinned = (
            "jobs:\n  example:\n    steps:\n"
            "      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n"
        )
        self.assertTrue(workflow_errors(tagged, "fixture.yml"))
        self.assertEqual(workflow_errors(pinned, "fixture.yml"), [])

    def test_signing_secret_is_confined_to_the_sign_step(self):
        workflow = """
jobs:
  build-windows:
    permissions:
      contents: read
    steps:
      - name: Build
        run: echo build
  sign-updater:
    steps:
      - name: Sign updater artifacts
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        run: echo sign
"""
        self.assertEqual(signing_errors(workflow), [])
        leaked = workflow.replace(
            "      - name: Build\n        run: echo build",
            "      - name: Build\n        env:\n          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}\n        run: echo build",
        )
        self.assertTrue(signing_errors(leaked))


if __name__ == "__main__":
    unittest.main()
