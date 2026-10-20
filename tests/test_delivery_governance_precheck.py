"""M128 synthetic fixtures: execute real governance rules without GitHub."""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('precheck', ROOT / 'scripts/delivery-governance-precheck.py')
precheck = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(precheck)


class GovernancePrecheckTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.tools = self.root / 'tools/governance'
        self.tools.mkdir(parents=True)

    def script(self, name, source):
        (self.tools / name).write_text(source)

    def test_real_rule_rejects_spec_then_accepts_system_recovery(self):
        shutil.copy(ROOT / 'tools/governance/validate_objective_non_interruption_contract.py', self.tools)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        spec = self.root / 'docs/specs/synthetic.md'
        spec.parent.mkdir(parents=True)
        spec.write_text('# Synthetic\n')
        subprocess.run(['git', '-C', str(self.root), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(self.root), '-c', 'user.name=Synthetic', '-c', 'user.email=synthetic@example.invalid', 'commit', '-qm', 'fixture'], check=True)
        base = subprocess.check_output(['git', '-C', str(self.root), 'rev-parse', 'HEAD'], text=True).strip()
        def commit_change():
            subprocess.run(['git', '-C', str(self.root), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(self.root), '-c', 'user.name=Synthetic', '-c', 'user.email=synthetic@example.invalid', 'commit', '-qm', 'spec change'], check=True)
        spec.write_text('# Requirements\n技术失败时用户必须点击重试按钮。\n')
        commit_change()
        rejected = precheck.check(self.root, base)
        self.assertFalse(rejected['ok'])
        self.assertIn('OBJECTIVE_MANUAL_TECHNICAL_HANDOFF', rejected['error'])
        self.assertIn('docs/specs/synthetic.md:2', rejected['error'])
        spec.write_text('# Requirements\n技术失败时系统自动重试并自愈。\n')
        commit_change()
        self.assertTrue(precheck.check(self.root, base)['ok'])

    def test_baseline_rule_failure_is_not_downgraded(self):
        self.script('validate_repo_governance_baseline.py', 'import json\nprint(json.dumps({"status":"fail","failures":[{"code":"BASELINE_MARKER_DRIFT","file":"synthetic.md","message":"missing marker"}]}))\nraise SystemExit(1)')
        result = precheck.check(self.root, 'HEAD')
        self.assertFalse(result['ok'])
        self.assertIn('BASELINE_MARKER_DRIFT', result['error'])

    def test_missing_crashed_and_timed_out_scripts_warn(self):
        missing = precheck.check(self.root, 'HEAD')
        self.assertTrue(missing['ok'])
        self.assertTrue(missing['warnings'])
        self.script('validate_repo_governance_baseline.py', 'raise RuntimeError("synthetic crash")')
        crashed = precheck.check(self.root, 'HEAD')
        self.assertTrue(crashed['ok'])
        self.assertIn('synthetic crash', str(crashed['warnings']))
        self.script('validate_repo_governance_baseline.py', 'import time; time.sleep(5)')
        timed = precheck.check(self.root, 'HEAD', timeout=0.05)
        self.assertTrue(timed['ok'])
        self.assertIn('timeout', str(timed['warnings']))

    def test_production_wiring_and_prompt(self):
        source = (ROOT / 'src-tauri/src/tools/delivery_thin.rs').read_text()
        self.assertLess(source.index('commands.precheck_governance('), source.index('commands.git(&["push"'))
        self.assertIn('治理预检警告', source)
        self.assertIn('OBJECTIVE_MANUAL_TECHNICAL_HANDOFF', (ROOT / 'src-tauri/src/agent/mod.rs').read_text())


if __name__ == '__main__':
    unittest.main()
