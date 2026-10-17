"""Req D1/D2/D3: CI must lint its own workflow files before they can merge."""

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "validate_ci_workflows.py"

GOOD_WORKFLOW = """name: good
on:
  pull_request:
    branches: [main]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: gated
        if: "${{ github.event.label.name == 'merge-queue: armed' }}"
        run: echo hi
"""

# Reproduces #674: a plain `if:` scalar that contains ": " does not parse.
BAD_WORKFLOW = """name: broken
on:
  pull_request:
jobs:
  merge:
    runs-on: ubuntu-latest
    steps:
      - name: gated
        if: ${{ github.event.label.name == 'merge-queue: armed' }}
        run: echo gated
"""


class CIWorkflowLintTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        (self.repo / ".github" / "workflows").mkdir(parents=True)
        (self.repo / ".github" / "rulesets").mkdir(parents=True)
        self.write_ruleset({"ruleset": {"name": "main-pr-and-ci-gate"}})

    def tearDown(self):
        self._tmp.cleanup()

    def write_workflow(self, name, text):
        (self.repo / ".github" / "workflows" / name).write_text(text)

    def write_ruleset(self, payload):
        (self.repo / ".github" / "rulesets" / "main.json").write_text(json.dumps(payload))

    def run_script(self, *extra):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--repo", str(self.repo), *extra],
            capture_output=True,
            text=True,
        )

    def output(self, result):
        return result.stdout + result.stderr

    def test_good_workflow_passes(self):
        self.write_workflow("ci.yml", GOOD_WORKFLOW)
        result = self.run_script()
        self.assertEqual(result.returncode, 0, self.output(result))

    def test_bad_unquoted_colon_in_if_is_rejected(self):
        self.write_workflow("merge-queue.yml", BAD_WORKFLOW)
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertIn("merge-queue.yml", self.output(result))

    def test_invalid_ruleset_json_is_rejected(self):
        (self.repo / ".github" / "rulesets" / "main.json").write_text('{"ruleset":')
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertIn("rulesets/main.json", self.output(result))

    def test_invalid_other_yaml_is_rejected(self):
        (self.repo / ".github" / "labeler.yml").write_text("labels:\n  - a: b: c\n")
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertIn("labeler.yml", self.output(result))

    def test_actionlint_failure_fails_the_gate(self):
        fake = self.repo / "fake-actionlint"
        fake.write_text("#!/bin/sh\necho 'workflow error from actionlint' >&2\nexit 1\n")
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        self.write_workflow("ci.yml", GOOD_WORKFLOW)
        result = self.run_script("--actionlint", str(fake))
        self.assertNotEqual(result.returncode, 0, self.output(result))
        self.assertIn("actionlint", self.output(result).lower())

    def test_actionlint_success_is_accepted_and_receives_workflow_files(self):
        record = self.repo / "actionlint-argv.txt"
        fake = self.repo / "fake-actionlint"
        fake.write_text(
            "#!/bin/sh\n"
            f'printf "%s\\n" "$@" > "{record}"\n'
            "exit 0\n"
        )
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        self.write_workflow("ci.yml", GOOD_WORKFLOW)
        result = self.run_script("--actionlint", str(fake))
        self.assertEqual(result.returncode, 0, self.output(result))
        argv = record.read_text()
        self.assertIn("ci.yml", argv)

    def test_workflows_directory_is_required_to_exist(self):
        (self.repo / ".github" / "workflows").rmdir()
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0, self.output(result))


if __name__ == "__main__":
    unittest.main()
