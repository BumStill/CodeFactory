"""Fail-first safety contract for the non-gating native observer experiment."""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def secret_expressions(text):
    return [expression for expression in re.findall(r"\$\{\{(.*?)\}\}", text, re.S)
            if re.search(r"\bsecrets\s*(?:\.|\[)", expression, re.I)]


class NativeObserverWorkflowTests(unittest.TestCase):
    def workflow(self):
        return (ROOT / ".github/workflows/native-desktop-observer.yml").read_text()

    def test_unprivileged_exact_candidate_without_persisted_credentials(self):
        text = self.workflow()
        self.assertIn("pull_request:", text)
        self.assertNotIn("pull_request_target", text)
        self.assertIn("contents: read", text)
        self.assertNotIn("write", text.split("permissions:", 1)[1].split("concurrency:", 1)[0])
        self.assertIn("ref: ${{ github.event.pull_request.head.sha }}", text)
        self.assertIn("persist-credentials: false", text)
        self.assertEqual(secret_expressions(text), [])
        self.assertNotIn("continue-on-error", text)
        self.assertIn("timeout-minutes: 45", text)

    def test_secret_detection_distinguishes_source_paths_from_account_secrets(self):
        self.assertEqual(secret_expressions("paths: ['src-tauri/src/secrets.rs']"), [])
        for expression in ["${{ secrets.TOKEN }}", "${{ secrets['TOKEN'] }}", "${{ SECRETS.TOKEN }}"]:
            self.assertTrue(secret_expressions(expression))

    def test_private_world_and_real_candidate_use_the_canonical_observer(self):
        text = self.workflow()
        self.assertIn("run-macos-native-observer.mjs prepare", text)
        self.assertIn("--expected-build-sha", text)
        self.assertIn("CODEFACTORY_BUILD_GIT_SHA: ${{ github.event.pull_request.head.sha }}", text)
        self.assertIn("pnpm exec tauri build --debug --bundles app", text)
        self.assertIn("xcrun swiftc scripts/macos-native-observer.swift", text)
        self.assertIn("run-macos-native-observer.mjs observe", text)
        self.assertIn(".codefactory-cache/cargo-target/debug/bundle/macos/CodeFactory.app", text)

    def test_prelaunch_tests_and_tauri_bundle_use_the_same_canonical_target(self):
        text = self.workflow()
        self.assertIn("CARGO_TARGET_DIR: ${{ github.workspace }}/.codefactory-cache/cargo-target", text)
        self.assertIn("workspaces: src-tauri -> ../.codefactory-cache/cargo-target", text)
        self.assertNotIn("src-tauri/target", text)

    def test_referenced_sources_and_prelaunch_contract_suites_exist(self):
        text = self.workflow()
        for name in ["scripts/run-macos-native-observer.mjs", "scripts/macos-native-observer.swift",
                     "scripts/probe-macos-native-preflight.swift", "scripts/assert_native_observer_receipt.py",
                     "scripts/native-desktop-probe-contract.test.mjs",
                     "scripts/macos-native-observer-supervisor.test.mjs",
                     "tests/test_native_observer_receipt.py", "tests/test_native_observer_workflow.py",
                     "tests/test_desktop_isolation_contract.py"]:
            with self.subTest(name=name):
                self.assertTrue((ROOT / name).is_file(), name)
                self.assertIn(name, text)
        prelaunch = text.split("- name: Verify synthetic contracts before launching anything", 1)[1]
        prelaunch = prelaunch.split("- name: Prepare a private synthetic world", 1)[0]
        self.assertIn("node --test", prelaunch)
        self.assertIn("tests.test_native_observer_receipt", prelaunch)
        for selector in ["desktop_context", "config::settings::settings_persistence_tests", "secrets::tests"]:
            self.assertIn("--lib " + selector + " --locked", prelaunch)

    def test_only_public_receipt_is_uploaded_and_full_probe_cannot_be_green(self):
        text = self.workflow()
        upload = text.split("- name: Upload anonymous observer receipt", 1)[1]
        self.assertIn("if: ${{ always() && steps.publish.outputs.public_receipt_ready == 'true' }}", upload)
        self.assertIn("path: ${{ runner.temp }}/native-observer-evidence/receipt.json", upload)
        self.assertIn("if-no-files-found: error", upload)
        self.assertNotIn("**", upload)
        self.assertNotIn("state.json", upload)
        self.assertNotIn("owner.json", upload)
        self.assertIn("assert_native_observer_receipt.py", text)
        self.assertIn("--receipt \"$CODEFACTORY_OBSERVER_RAW\"", text)
        self.assertIn("--public-output \"$CODEFACTORY_OBSERVER_PUBLIC\"", text)
        self.assertIn("native-observer-private/receipt.json", text)
        validation = text.split("- name: Keep the observer slice separate", 1)[1].split("- name: Upload", 1)[0]
        self.assertIn("if: ${{ always() }}", validation)
        self.assertIn("id: publish", validation)
        self.assertNotIn("native-observer-private", upload)
        self.assertNotIn("CODEFACTORY_OBSERVER_RAW", upload)


if __name__ == "__main__":
    unittest.main()
