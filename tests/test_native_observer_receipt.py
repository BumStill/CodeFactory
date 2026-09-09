"""The narrow observer result must never turn into full desktop evidence."""
import copy
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts.assert_native_observer_receipt import main, publish_validated_receipt, validate_receipt


SHA = "a" * 40
RUN = "aabc1234-5678-4abc-8abc-123456789abc"
DIGEST = "b" * 64
MISSING = ["native_theme_click", "native_text_input", "restart_theme_persistence",
           "owned_window_screenshots", "descendant_process_cleanup", "world_directory_cleanup",
           "request_observation", "credential_observation", "embedded_build_identity"]


def valid_slice():
    identifier = "com.codefactory.scenario." + RUN.replace("-", "")
    return {
        "schema_version": 1, "scope": "native-desktop-observer-slice", "run_id": RUN,
        "identifier": identifier, "expected_build_sha": SHA,
        "build_identity_source": "ci_input_unverified_in_binary",
        "source_executable_sha256": DIGEST, "copy_executable_sha256": DIGEST,
        "driver_sha256": "c" * 64,
        "process": {"run_id": RUN, "pid": 12345, "start_token": "1700000000:123",
                    "executable_sha256": DIGEST, "bundle_id": identifier},
        "observer_slice": {"status": "passed", "reason_codes": [],
                           "ax_window_seen": True, "settings_control": True},
        "cleanup": {"child_reaped": True, "signal_attempts": 1, "world_directory": "retained"},
        "full_probe": {"status": "blocked", "missing": MISSING.copy()},
        "request_count": None, "credential_access_count": None,
    }


class NativeObserverReceiptTests(unittest.TestCase):
    def test_real_supervisor_protocol_round_trips_through_public_cli(self):
        # Execute both real entry points with injected synthetic child adapters.
        # This proves protocol compatibility, not a native App/AX observation.
        root = pathlib.Path(__file__).resolve().parents[1]
        program = """
import { observeOwnedChild } from './scripts/run-macos-native-observer.mjs';
const run = 'aabc1234-5678-4abc-8abc-123456789abc';
const expected = {run_id: run, owner_token: '00000000-0000-4000-8000-000000000002',
  executable_sha256: 'b'.repeat(64), bundle_id: 'com.codefactory.scenario.' + run.replaceAll('-', '')};
const child = {pid: 12345, exitCode: null, signalCode: null};
const identity = {...expected, pid: child.pid, start_token: '1700000000:123'};
const result = await observeOwnedChild(expected, {
  checkWorld: async () => {}, launch: () => child, inspect: async () => identity,
  observe: async () => ({ax_window_seen: true, settings_control: true}),
  stop: async () => {child.signalCode = 'SIGTERM';}, waitForExit: async () => true,
});
console.log(JSON.stringify({schema_version: 1, scope: 'native-desktop-observer-slice',
  run_id: run, identifier: expected.bundle_id, expected_build_sha: 'a'.repeat(40),
  build_identity_source: 'ci_input_unverified_in_binary', source_executable_sha256: 'b'.repeat(64),
  copy_executable_sha256: 'b'.repeat(64), driver_sha256: 'c'.repeat(64), ...result}));
"""
        response = subprocess.run(["node", "--input-type=module", "-e", program], cwd=root,
                                  capture_output=True, text=True, check=True, timeout=10)
        receipt = json.loads(response.stdout)
        self.assertEqual(receipt, valid_slice())
        with tempfile.TemporaryDirectory() as folder:
            temp = pathlib.Path(folder)
            raw, public, output = temp / "private.json", temp / "public.json", temp / "github-output"
            command = [sys.executable, "-B", str(root / "scripts/assert_native_observer_receipt.py"),
                       "--receipt", str(raw), "--public-output", str(public), "--expected-build-sha", SHA]
            for forged in [False, True]:
                if forged:
                    receipt["full_probe"]["status"] = "passed"
                    receipt["raw_ax_text"] = "PRIVATE_SENTINEL"
                raw.write_text(json.dumps(receipt))
                result = subprocess.run(command, env={**os.environ, "GITHUB_OUTPUT": str(output)},
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 1 if forged else 0)
                self.assertNotIn("PRIVATE_SENTINEL", result.stdout + result.stderr + public.read_text())
                self.assertIn("public_receipt_ready=true", output.read_text())
                if not forged:
                    self.assertEqual(json.loads(public.read_text()), valid_slice())
                else:
                    self.assertEqual(json.loads(public.read_text())["status"], "failed")

    def test_narrow_observer_result_is_accepted_without_promoting_full_probe(self):
        self.assertEqual(validate_receipt(valid_slice(), SHA), [])

    def test_failures_missing_or_forged_identity_and_cleanup_cannot_pass(self):
        mutations = [
            lambda r: r.update(expected_build_sha="d" * 40),
            lambda r: r.update(copy_executable_sha256="d" * 64),
            lambda r: r["process"].update(executable_sha256="d" * 64),
            lambda r: r["process"].update(run_id="d" + RUN[1:]),
            lambda r: r["process"].update(bundle_id="com.codefactory.app"),
            lambda r: r["process"].update(pid=True),
            lambda r: r["process"].update(pid=2**31),
            lambda r: r["process"].update(start_token="1700000000:1000000"),
            lambda r: r["observer_slice"].update(ax_window_seen=False),
            lambda r: r["observer_slice"].update(settings_control=1),
            lambda r: r["observer_slice"].update(status="blocked"),
            lambda r: r["observer_slice"].update(reason_codes=["not_observed"]),
            lambda r: r["cleanup"].update(child_reaped=False),
            lambda r: r["cleanup"].update(signal_attempts=True),
            lambda r: r.update(process=None),
        ]
        for index, mutate in enumerate(mutations):
            receipt = valid_slice()
            mutate(receipt)
            with self.subTest(index=index):
                self.assertTrue(validate_receipt(receipt, SHA))

    def test_unobserved_counts_and_full_probe_cannot_be_forged_as_success(self):
        mutations = [
            lambda r: r.update(request_count=0),
            lambda r: r.update(credential_access_count=0),
            lambda r: r.update(build_identity_source="exact_release_artifact"),
            lambda r: r["full_probe"].update(status="passed"),
            lambda r: r["full_probe"].update(missing=[]),
            lambda r: r["full_probe"]["missing"].append(MISSING[0]),
            lambda r: r["cleanup"].update(world_directory="removed"),
        ]
        for index, mutate in enumerate(mutations):
            receipt = valid_slice()
            mutate(receipt)
            with self.subTest(index=index):
                self.assertTrue(validate_receipt(receipt, SHA))

    def test_unknown_fields_and_raw_private_values_are_not_echoed(self):
        for target in [None, "process", "cleanup", "observer_slice", "full_probe"]:
            receipt = copy.deepcopy(valid_slice())
            part = receipt if target is None else receipt[target]
            part["owner_token"] = "PRIVATE_SENTINEL"
            errors = validate_receipt(receipt, SHA)
            self.assertTrue(errors)
            self.assertNotIn("PRIVATE_SENTINEL", str(errors))
        for receipt in [None, [], True, 1, "PRIVATE_SENTINEL", {}]:
            self.assertTrue(validate_receipt(receipt, SHA))

    def test_failed_validation_only_publishes_a_fixed_safe_diagnostic(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            raw, public = root / "private.json", root / "public" / "receipt.json"
            receipt = valid_slice()
            receipt["private_text"] = "PRIVATE_SENTINEL"
            raw.write_text(json.dumps(receipt))
            self.assertFalse(publish_validated_receipt(raw, public, SHA))
            published = json.loads(public.read_text())
            self.assertEqual(published, {"schema_version": 1, "scope": "native-desktop-observer-validation",
                                        "status": "failed", "reason_codes": ["receipt_not_accepted"]})
            self.assertNotIn("PRIVATE_SENTINEL", public.read_text())

    def test_validated_slice_is_published_but_missing_or_duplicate_input_is_not(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            raw, public = root / "private.json", root / "public.json"
            self.assertFalse(publish_validated_receipt(raw, public, SHA))
            raw.write_text(json.dumps(valid_slice()))
            self.assertTrue(publish_validated_receipt(raw, public, SHA))
            self.assertEqual(json.loads(public.read_text()), valid_slice())
            raw.write_text('{"schema_version":1,"schema_version":2}')
            self.assertFalse(publish_validated_receipt(raw, public, SHA))
            self.assertEqual(json.loads(public.read_text())["status"], "failed")

    def test_upload_authority_is_only_emitted_after_safe_atomic_publication(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            raw, public, output = root / "private.json", root / "public.json", root / "github-output"
            raw.write_text('{"private_text":"PRIVATE_SENTINEL"}')
            public.symlink_to(raw)
            argv = ["assert_native_observer_receipt.py", "--receipt", str(raw), "--public-output", str(public),
                    "--expected-build-sha", SHA]
            with mock.patch("sys.argv", argv), mock.patch.dict("os.environ", {"GITHUB_OUTPUT": str(output)}):
                self.assertEqual(main(), 1)
                self.assertFalse(output.exists(), "unsafe destination must never authorize upload")
                public.unlink()
                self.assertEqual(main(), 1, "bad raw remains a failed result even with a safe diagnostic")
            self.assertEqual(output.read_text(), "public_receipt_ready=true\n")
            self.assertNotIn("PRIVATE_SENTINEL", public.read_text())


if __name__ == "__main__":
    unittest.main()
