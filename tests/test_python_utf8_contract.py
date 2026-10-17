"""Contract tests for the Windows CI UTF-8 fix (M49, Req R1-R4).

Spec: ``docs/specs/feature-specs/windows-ci-python-utf8.md``.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from tools.governance import validate_python_utf8_contract as contract

REPO_ROOT = Path(__file__).resolve().parents[1]
CHECKER = REPO_ROOT / "tools" / "governance" / "validate_python_utf8_contract.py"
README_VALIDATOR = REPO_ROOT / "tools" / "governance" / "validate_readme_contract.py"

UTF8_ENV_BLOCK = 'env:\n  PYTHONUTF8: "1"\n  PYTHONIOENCODING: utf-8\n'


class WorkflowUtf8EnvTests(unittest.TestCase):
    """R1 + R3(a): every workflow enables Python's UTF-8 mode."""

    def test_missing_env_is_a_violation(self) -> None:
        # Seen failing first: this is the pre-fix shape of every workflow.
        workflow = (
            "name: CI\n"
            "\n"
            "on:\n"
            "  push:\n"
            "    branches: [main]\n"
            "\n"
            "jobs:\n"
            "  check-frontend:\n"
            "    runs-on: windows-latest\n"
            "    steps:\n"
            "      - run: python -m unittest discover -s tests\n"
        )

        violations = contract.workflow_env_violations("ci.yml", workflow)

        self.assertTrue(violations, "a workflow without the UTF-8 env must fail")
        self.assertIn("PYTHONUTF8", violations[0])

    def test_workflow_level_env_is_accepted(self) -> None:
        workflow = "name: CI\n\n" + UTF8_ENV_BLOCK + "\non:\n  push:\n    branches: [main]\n\njobs:\n  check-frontend:\n    runs-on: windows-latest\n    steps:\n      - run: python -V\n"

        self.assertEqual(contract.workflow_env_violations("ci.yml", workflow), [])

    def test_job_level_env_is_equivalent(self) -> None:
        workflow = (
            "name: CI\n"
            "\n"
            "on:\n"
            "  push:\n"
            "    branches: [main]\n"
            "\n"
            "jobs:\n"
            "  check-frontend:\n"
            "    runs-on: windows-latest\n"
            "    env:\n"
            '      PYTHONUTF8: "1"\n'
            "      PYTHONIOENCODING: utf-8\n"
            "    steps:\n"
            "      - run: python -V\n"
            "  check-rust:\n"
            "    runs-on: windows-latest\n"
            "    env:\n"
            '      PYTHONUTF8: "1"\n'
            "      PYTHONIOENCODING: utf-8\n"
            "    steps:\n"
            "      - run: cargo test\n"
        )

        self.assertEqual(contract.workflow_env_violations("ci.yml", workflow), [])

    def test_real_workflows_all_declare_the_utf8_env(self) -> None:
        violations = contract.workflow_violations(REPO_ROOT / ".github" / "workflows")

        self.assertEqual(violations, [], "every workflow must opt into UTF-8 mode")


class PythonSubprocessEncodingTests(unittest.TestCase):
    """R2 + R3(b): text-mode subprocess calls pin their encoding."""

    def test_text_mode_without_encoding_is_a_violation(self) -> None:
        # Seen failing first: the pre-fix shape of ~30 CI-called scripts.
        source = (
            "import subprocess\n"
            "\n"
            "def run() -> str:\n"
            "    return subprocess.run(['git', 'status'], capture_output=True, text=True).stdout\n"
        )

        violations = contract.python_subprocess_violations("tools/x.py", source)

        self.assertEqual(len(violations), 1)
        self.assertIn("encoding", violations[0])

    def test_universal_newlines_without_encoding_is_a_violation(self) -> None:
        source = (
            "import subprocess\n"
            "\n"
            "def run() -> str:\n"
            "    return subprocess.check_output(['git', 'status'], universal_newlines=True)\n"
        )

        violations = contract.python_subprocess_violations("tools/x.py", source)

        self.assertEqual(len(violations), 1)

    def test_explicit_encoding_is_accepted(self) -> None:
        source = (
            "import subprocess\n"
            "\n"
            "def run() -> str:\n"
            "    return subprocess.run(\n"
            "        ['git', 'status'],\n"
            "        capture_output=True,\n"
            "        text=True,\n"
            '        encoding="utf-8",\n'
            '        errors="replace",\n'
            "    ).stdout\n"
        )

        self.assertEqual(contract.python_subprocess_violations("tools/x.py", source), [])

    def test_binary_mode_is_not_constrained(self) -> None:
        source = (
            "import subprocess\n"
            "\n"
            "def run() -> bytes:\n"
            "    return subprocess.run(['git', 'status'], capture_output=True).stdout\n"
        )

        self.assertEqual(contract.python_subprocess_violations("tools/x.py", source), [])

    def test_real_tools_and_scripts_pin_their_encoding(self) -> None:
        violations = contract.python_violations(REPO_ROOT)

        self.assertEqual(violations, [], "text-mode subprocess calls must pin encoding")


class CheckerCliTests(unittest.TestCase):
    """R3: the checker is wired as a CI-runnable command."""

    def test_cli_passes_on_this_repository(self) -> None:
        result = subprocess.run(
            [sys.executable, str(CHECKER)],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )

        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        self.assertIn("python-utf8-contract: OK", result.stdout)

    def test_cli_fails_on_a_workflow_without_the_env(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / ".github" / "workflows").mkdir(parents=True)
            (repo / "tools").mkdir()
            (repo / ".github" / "workflows" / "ci.yml").write_text(
                "name: CI\n\njobs:\n  check-frontend:\n    runs-on: windows-latest\n    steps:\n      - run: python -V\n",
                encoding="utf-8",
            )
            (repo / "tools" / "runner.py").write_text(
                "import subprocess\n\nsubprocess.run(['git', 'status'], text=True)\n",
                encoding="utf-8",
            )

            result = subprocess.run(
                [sys.executable, str(CHECKER), "--repo", str(repo)],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )

        self.assertEqual(result.returncode, 1, msg=result.stdout + result.stderr)
        self.assertIn("R1", result.stdout)
        self.assertIn("R2", result.stdout)


class ReadmeContractCp1252Tests(unittest.TestCase):
    """R4: Chinese diagnostics survive a cp1252 stdio locale."""

    def test_chinese_error_text_survives_cp1252_stdio(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "README.md").write_text(
                "<!-- README-CONTRACT: evergreen -->\n"
                "\n"
                "## Features\n"
                "## Install\n"
                "## Quick start\n"
                "## Build from source\n"
                "## Data & privacy\n"
                "## Architecture\n"
                "## License\n"
                "\n"
                "[下载](https://github.com/BumStill/CodeFactory/releases/latest)\n"
                "[部署说明](./部署说明.md)\n",
                encoding="utf-8",
            )

            environment = dict(os.environ)
            environment["PYTHONIOENCODING"] = "cp1252"
            environment["PYTHONUTF8"] = "0"

            result = subprocess.run(
                [sys.executable, str(README_VALIDATOR), "--repo", str(repo)],
                cwd=REPO_ROOT,
                capture_output=True,
                env=environment,
            )

        stdout = result.stdout.decode("utf-8", errors="replace")
        stderr = result.stderr.decode("utf-8", errors="replace")

        self.assertEqual(result.returncode, 1, msg=stdout + stderr)
        self.assertNotIn("UnicodeEncodeError", stderr)
        self.assertNotIn("UnicodeEncodeError", stdout)
        self.assertIn("部署说明.md", stdout, "the Chinese error text must survive")


if __name__ == "__main__":
    unittest.main()
