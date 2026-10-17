"""Req E1-E4: quarantined flaky tests are retried exactly once, never skipped."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "run_flaky_retry.py"

TEST_NAME = "admission_records_a_wording_constraint_when_the_banner_changes"


class FlakyRetryTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        self.counter = self.dir / "counter.txt"
        self.summary = self.dir / "summary.md"
        self.quarantine = self.dir / "flaky-quarantine.json"

    def tearDown(self):
        self._tmp.cleanup()

    # -- helpers ---------------------------------------------------------
    def entry(self, **overrides):
        payload = {
            "test": TEST_NAME,
            "job": "check-rust",
            "evidence": "https://github.com/BumStill/CodeFactory/pull/666",
            "reason": "M93 wording-constraint assertion was order dependent",
            "expires_on": "2099-12-31",
        }
        payload.update(overrides)
        return payload

    def write_quarantine(self, entries):
        self.quarantine.write_text(json.dumps({"version": 1, "entries": entries}))

    def run_retry(self, job="check-rust", extra=()):
        env = dict(os.environ, FLAKY_COUNTER=str(self.counter))
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--quarantine",
                str(self.quarantine),
                "--job",
                job,
                "--summary-file",
                str(self.summary),
                *extra,
                "--",
                sys.executable,
                "-c",
                self.failing_command(),
            ],
            capture_output=True,
            text=True,
            env=env,
        )

    def failing_command(self, fail_times=1):
        return (
            "import os, pathlib, sys\n"
            f"name = {TEST_NAME!r}\n"
            "p = pathlib.Path(os.environ['FLAKY_COUNTER'])\n"
            "n = int(p.read_text()) if p.exists() else 0\n"
            "p.write_text(str(n + 1))\n"
            f"if n < {fail_times}:\n"
            "    print(f'test {name} ... FAILED')\n"
            "    sys.exit(101)\n"
            "print('all tests passed')\n"
        )

    def runs(self):
        return int(self.counter.read_text()) if self.counter.exists() else 0

    def output(self, result):
        return result.stdout + result.stderr

    # -- Req E2 ----------------------------------------------------------
    def test_quarantined_failure_passes_after_retry(self):
        self.write_quarantine([self.entry()])
        result = self.run_retry()
        self.assertEqual(result.returncode, 0, self.output(result))
        self.assertEqual(self.runs(), 2, "the command must run, then be retried once")
        summary = self.summary.read_text().lower()
        self.assertIn("flaky", summary)
        self.assertIn("retried", summary)
        self.assertIn(TEST_NAME.lower(), summary)

    def test_non_ascii_and_undecodable_output_does_not_crash_on_cp1252(self):
        # Windows CI (#680 first run): output with byte 0x8f crashed the wrapper
        # with UnicodeDecodeError under the runner's cp1252 default.
        self.write_quarantine([])
        env = dict(os.environ, PYTHONIOENCODING="cp1252", PYTHONUTF8="0")
        command = (
            "import sys\n"
            "sys.stdout.buffer.write(b'\\x8f \\xe4\\xb8\\xad\\xe6\\x96\\x87 selectSession: gone\\n')\n"
            "sys.stdout.buffer.flush()\n"
        )
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--quarantine", str(self.quarantine),
             "--job", "check-frontend", "--summary-file", str(self.summary),
             "--", sys.executable, "-c", command],
            capture_output=True, env=env,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn(b"UnicodeDecodeError", result.stdout + result.stderr)

    # -- Req E3 ----------------------------------------------------------
    def test_off_list_failure_fails(self):
        self.write_quarantine([])
        result = self.run_retry()
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertEqual(self.runs(), 1)

    def test_passing_test_is_not_retried(self):
        self.write_quarantine([self.entry()])
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--quarantine",
                str(self.quarantine),
                "--job",
                "check-rust",
                "--summary-file",
                str(self.summary),
                "--",
                sys.executable,
                "-c",
                self.failing_command(fail_times=0),
            ],
            capture_output=True,
            text=True,
            env=dict(os.environ, FLAKY_COUNTER=str(self.counter)),
        )
        self.assertEqual(result.returncode, 0, self.output(result))
        self.assertEqual(self.runs(), 1)
        self.assertFalse(self.summary.exists() and "retried" in self.summary.read_text().lower())

    def test_quarantined_test_still_fails_when_retry_also_fails(self):
        self.write_quarantine([self.entry()])
        result = self.run_retry()
        self.assertEqual(self.runs(), 2)
        # Second run is the retry; make both attempts fail.
        self.counter.unlink()
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--quarantine",
                str(self.quarantine),
                "--job",
                "check-rust",
                "--summary-file",
                str(self.summary),
                "--",
                sys.executable,
                "-c",
                self.failing_command(fail_times=2),
            ],
            capture_output=True,
            text=True,
            env=dict(os.environ, FLAKY_COUNTER=str(self.counter)),
        )
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertEqual(self.runs(), 2)

    def test_entry_scoped_to_another_job_does_not_retry(self):
        self.write_quarantine([self.entry(job="check-frontend")])
        result = self.run_retry(job="check-rust")
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertEqual(self.runs(), 1)

    # -- Req E4 ----------------------------------------------------------
    def test_expired_entry_fails_closed_without_running_tests(self):
        self.write_quarantine([self.entry(expires_on="2000-01-01")])
        result = self.run_retry()
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertIn("expired", self.output(result).lower())
        self.assertEqual(self.runs(), 0, "an invalid list must not run the test command")

    def test_missing_evidence_is_rejected(self):
        entry = self.entry()
        entry.pop("evidence")
        self.write_quarantine([entry])
        result = self.run_retry()
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertIn("evidence", self.output(result).lower())
        self.assertEqual(self.runs(), 0)

    def test_check_only_validates_the_committed_list(self):
        self.write_quarantine([self.entry()])
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--quarantine",
                str(self.quarantine),
                "--check-only",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, self.output(result))
        self.assertIn("1", result.stdout)

    def test_missing_command_is_an_error(self):
        self.write_quarantine([])
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--quarantine",
                str(self.quarantine),
                "--job",
                "check-rust",
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0, self.output(result))


if __name__ == "__main__":
    unittest.main()
