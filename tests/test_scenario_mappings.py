import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from tools.governance import validate_scenario_test_governance as governance
from tools.governance.run_scenario_harness_gate import TRUST_ROOT_FILES, validate_trust_root_immutability
from tools.governance.scenario_execution import build_execution_plan

ROOT = Path(__file__).resolve().parents[1]
MAPPINGS = 'docs/testing/scenario-mappings.json'


class ScenarioMappingsTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.path = self.root / 'docs/testing/scenario-registry.json'
        self.path.parent.mkdir(parents=True)
        self.registry = governance.load_registry()
        self.path.write_text(json.dumps(self.registry))
        self.sid = self.registry['scenarios'][0]['id']
        self.entry = {'pattern': 'src/synthetic-added/*.ts', 'scenario_id': self.sid}

    def write(self, entries):
        (self.root / MAPPINGS).write_text(json.dumps({'schema_version': 1, 'mappings': entries}))

    def load(self, base=None):
        return governance.load_registry(self.path, base_ref=base)

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], text=True).strip()

    def commit_base(self):
        self.git('init', '-q')
        self.git('config', 'user.name', 'Synthetic')
        self.git('config', 'user.email', 'synthetic@example.invalid')
        self.git('add', '.')
        self.git('commit', '-qm', 'synthetic base')
        return self.git('rev-parse', 'HEAD')

    def test_addition_union_and_execution_plan(self):
        before = copy.deepcopy(self.registry)
        base = self.commit_base()
        self.write([self.entry])
        loaded = self.load(base)
        files = ['src/synthetic-added/nonexample.ts']
        self.assertEqual(governance._impacted_scenarios(files, loaded), {self.sid})
        self.assertEqual(governance.validate_change_contract('', 'Scenario-Test: ' + self.sid, files, loaded), [])
        plan = build_execution_plan(loaded, files, base_sha='a' * 40, head_sha='b' * 40)
        self.assertIn(self.sid, plan['scenario_ids'])
        self.assertEqual(json.loads(self.path.read_text()), before)
        self.assertNotIn(MAPPINGS, TRUST_ROOT_FILES)

    def test_delete_shrink_and_retarget_rejected(self):
        self.write([self.entry])
        base = self.commit_base()
        for entries in ([], [dict(self.entry, pattern='src/synthetic-added/one.ts')],
                        [dict(self.entry, scenario_id=self.registry['scenarios'][1]['id'])]):
            with self.subTest(entries=entries):
                self.write(entries)
                with self.assertRaisesRegex(ValueError, 'append-only'):
                    self.load(base)
        (self.root / MAPPINGS).unlink()
        with self.assertRaisesRegex(ValueError, 'append-only'):
            self.load(base)

    def test_unknown_inactive_broad_and_trust_root_rejected(self):
        self.registry['scenarios'][0]['gates'] = ['manual_canary']
        self.path.write_text(json.dumps(self.registry))
        self.write([self.entry])
        with self.assertRaisesRegex(ValueError, 'active'):
            self.load()
        self.path.write_text(json.dumps(governance.load_registry()))
        for pattern, sid in [('src/new.ts', 'MISSING-999'), ('**', self.sid),
                             ('*', self.sid), ('**/*', self.sid),
                             ('tools/governance/*.py', self.sid),
                             ('docs/testing/scenario-registry.json', self.sid)]:
            with self.subTest(pattern=pattern, sid=sid):
                self.write([{'pattern': pattern, 'scenario_id': sid}])
                with self.assertRaises(ValueError):
                    self.load()

    def test_malformed_symlink_and_bad_base_fail_closed(self):
        for value in ('[]', '{', '{"schema_version": 1, "mappings": [null]}'):
            (self.root / MAPPINGS).write_text(value)
            with self.assertRaises(ValueError):
                self.load()
        (self.root / MAPPINGS).unlink()
        (self.root / MAPPINGS).symlink_to(self.path)
        with self.assertRaises(ValueError):
            self.load()
        (self.root / MAPPINGS).unlink()
        with self.assertRaises(ValueError):
            self.load('missing-base')

    def test_addition_not_self_modify_but_registry_still_is(self):
        with tempfile.TemporaryDirectory() as directory:
            policy = Path(directory)
            for relative in (*TRUST_ROOT_FILES, 'src-tauri/src/lib.rs', 'src-tauri/Cargo.toml'):
                for root in (policy, self.root):
                    target = root / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes((ROOT / relative).read_bytes())
            self.write([self.entry])
            self.assertEqual(validate_trust_root_immutability(self.root, policy), [])
            self.path.write_text(self.path.read_text() + '\n')
            self.assertTrue(any('cannot self-modify' in error for error in validate_trust_root_immutability(self.root, policy)))

    def test_m156_adapter_uses_union(self):
        base = self.commit_base()
        self.write([self.entry])
        source = self.root / 'src/synthetic-added/second.ts'
        source.parent.mkdir(parents=True)
        source.write_text('export const synthetic = 2;')
        self.git('add', '.')
        self.git('commit', '-qm', 'synthetic addition')
        spec = importlib.util.spec_from_file_location('adapter', ROOT / 'scripts/delivery-scenario-body.py')
        adapter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(adapter)
        result = adapter.reconcile(ROOT / 'tools/governance/validate_scenario_test_governance.py', self.root, base, 'Synthetic body\n')
        self.assertIn('Scenario-Test: ' + self.sid, result['body'])
