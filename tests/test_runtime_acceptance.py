from __future__ import annotations

import json
import os
import stat
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

from tools.agent.run_runtime_acceptance import (
    RuntimeConfig,
    _minimal_runtime_env,
    _redact_text,
    load_runtime_config,
    run_runtime_acceptance,
)


class RuntimeAcceptanceTests(unittest.TestCase):
    def test_runtime_environment_is_allowlisted_and_sensitive_output_is_redacted(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            os.environ["UNRELATED_SECRET"] = "must-not-be-inherited"
            try:
                runtime_env = _minimal_runtime_env(Path(temp_dir))
            finally:
                os.environ.pop("UNRELATED_SECRET", None)

        self.assertNotIn("UNRELATED_SECRET", runtime_env)
        self.assertEqual(runtime_env["HOME"], temp_dir)
        self.assertNotIn(
            "must-not-leak",
            _redact_text(
                "api_key=must-not-leak password:also-private",
                ("must-not-leak",),
            ),
        )

    def test_dev_app_wrapper_holds_a_caffeinate_assertion(self) -> None:
        wrapper = (
            Path(__file__).resolve().parents[1] / "scripts" / "install-dev-app-wrapper.sh"
        ).read_text(encoding="utf-8")

        self.assertIn("exec /usr/bin/caffeinate -dimsu", wrapper)

    def test_loads_active_model_from_current_codefactory_endpoint(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            settings_path = Path(temp_dir) / "settings.json"
            settings_path.write_text(
                json.dumps(
                    {
                        "default_endpoint": "deepseek",
                        "default_model": "gpt-5.5",
                        "endpoints": {
                            "deepseek": {
                                "base_url": "https://api.deepseek.com",
                                "key_ref": "codefactory.endpoint.deepseek",
                                "api_style": "openai",
                                "active_model": "deepseek/deepseek-v4-pro",
                            }
                        },
                    }
                ),
                encoding="utf-8",
            )

            config = load_runtime_config(settings_path)

            self.assertEqual(config.endpoint_id, "deepseek")
            self.assertEqual(config.model, "deepseek-v4-pro")
            self.assertEqual(config.base_url, "https://api.deepseek.com")
            self.assertEqual(config.key_ref, "codefactory.endpoint.deepseek")

    @unittest.skipUnless(sys.platform == "darwin", "requires macOS workspace sandbox")
    def test_executes_local_tool_protocol_and_writes_redacted_evidence(self) -> None:
        secret = "runtime-acceptance-secret"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            cwd = root / "project"
            evidence_dir = root / "evidence"
            cwd.mkdir()
            (cwd / "value.txt").write_text("9\n", encoding="utf-8")
            sidecar = root / "fake-sidecar.py"
            sidecar.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import sys

                    start = json.loads(sys.stdin.readline())
                    assert start["api_key"]
                    print(json.dumps({
                        "type": "tool_request",
                        "id": "tool-1",
                        "command": "cat value.txt",
                        "timeout_sec": 10,
                    }), flush=True)
                    result = json.loads(sys.stdin.readline())
                    assert result["id"] == "tool-1"
                    assert result["return_code"] == 0
                    assert result["stdout"].strip() == "9"
                    print(json.dumps({
                        "type": "finished",
                        "final_text": "verified " + start["api_key"],
                        "execution_contract_sha256": start["execution_contract_sha256"],
                        "completion_evidence": {"completed": True},
                        "usage": {"total_tokens": 12},
                    }), flush=True)
                    """
                ),
                encoding="utf-8",
            )
            sidecar.chmod(sidecar.stat().st_mode | stat.S_IXUSR)
            contract = root / "execution_completion.md"
            contract.write_text("shared acceptance contract\n", encoding="utf-8")
            config = RuntimeConfig(
                endpoint_id="deepseek",
                base_url="https://api.deepseek.com",
                model="deepseek-v4-pro",
                key_ref="codefactory.endpoint.deepseek",
            )

            result = run_runtime_acceptance(
                instruction="Read value.txt and verify it is 9.",
                cwd=cwd,
                evidence_dir=evidence_dir,
                sidecar_path=sidecar,
                config=config,
                api_key=secret,
                contract_path=contract,
                screen_locked=True,
                max_steps=4,
                wall_time_budget_sec=30,
            )

            self.assertEqual(result["status"], "passed")
            self.assertEqual(result["proof_tier"], "agent-runtime-no-gui")
            self.assertTrue(result["screen_locked"])
            self.assertEqual(result["provider"], "deepseek")
            self.assertEqual(result["model"], "deepseek-v4-pro")
            self.assertEqual(result["tool_calls"], 1)
            self.assertEqual(result["completion_evidence"], {"completed": True})
            serialized = "\n".join(
                path.read_text(encoding="utf-8")
                for path in evidence_dir.iterdir()
                if path.is_file()
            )
            self.assertNotIn(secret, serialized)
            self.assertIn("cat value.txt", serialized)
            self.assertIn('"return_code": 0', serialized)

    @unittest.skipUnless(sys.platform == "darwin", "requires macOS workspace sandbox")
    def test_workspace_sandbox_denies_writes_outside_selected_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            cwd = root / "project"
            evidence_dir = root / "evidence"
            cwd.mkdir()
            sidecar = root / "fake-sidecar.py"
            sidecar.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import sys

                    start = json.loads(sys.stdin.readline())
                    print(json.dumps({
                        "type": "tool_request",
                        "id": "tool-1",
                        "command": "touch ../escaped.txt",
                        "timeout_sec": 10,
                    }), flush=True)
                    result = json.loads(sys.stdin.readline())
                    assert result["id"] == "tool-1"
                    assert result["return_code"] != 0
                    print(json.dumps({
                        "type": "finished",
                        "final_text": "outside write denied",
                        "execution_contract_sha256": start["execution_contract_sha256"],
                        "completion_evidence": {"completed": False},
                        "usage": {"total_tokens": 1},
                    }), flush=True)
                    """
                ),
                encoding="utf-8",
            )
            sidecar.chmod(sidecar.stat().st_mode | stat.S_IXUSR)
            contract = root / "execution_completion.md"
            contract.write_text("shared acceptance contract\n", encoding="utf-8")

            result = run_runtime_acceptance(
                instruction="Attempt one outside write.",
                cwd=cwd,
                evidence_dir=evidence_dir,
                sidecar_path=sidecar,
                config=RuntimeConfig("deepseek", "https://api.deepseek.com", "m", "k"),
                api_key="secret",
                contract_path=contract,
                screen_locked=False,
                max_steps=2,
                wall_time_budget_sec=30,
            )

            self.assertEqual(result["status"], "failed")
            self.assertFalse((root / "escaped.txt").exists())
            trajectory = (evidence_dir / "trajectory.jsonl").read_text(encoding="utf-8")
            self.assertIn("Operation not permitted", trajectory)

    def test_rejects_a_working_directory_that_is_not_a_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            not_a_dir = root / "file.txt"
            not_a_dir.write_text("x", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "working directory"):
                run_runtime_acceptance(
                    instruction="test",
                    cwd=not_a_dir,
                    evidence_dir=root / "evidence",
                    sidecar_path=root / "missing",
                    config=RuntimeConfig("x", "https://example.test", "m", "k"),
                    api_key="secret",
                    contract_path=root / "missing-contract",
                    screen_locked=False,
                )


if __name__ == "__main__":
    unittest.main()
