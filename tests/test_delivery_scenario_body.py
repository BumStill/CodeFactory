import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / 'scripts/delivery-scenario-body.py'
VALIDATOR = ROOT / 'tools/governance/validate_scenario_test_governance.py'


class DeliveryScenarioBodyTests(unittest.TestCase):
    def test_synthetic_branch_repairs_all_handwritten_variants(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-C', directory, *args], text=True).strip()
            git('init', '-q')
            git('config', 'user.name', 'Synthetic')
            git('config', 'user.email', 'synthetic@example.invalid')
            path = root / 'docs/testing/scenario-registry.json'
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps({'scenarios': [
                {'id': 'SYN-101', 'change_patterns': ['src/alpha.*']},
                {'id': 'SYN-202', 'change_patterns': ['src/alpha.*', 'src/beta.*']},
                {'id': 'SYN-303', 'change_patterns': ['src/other.*']},
            ]}))
            git('add', '.')
            git('commit', '-qm', 'synthetic base')
            base = git('rev-parse', 'HEAD')
            (root / 'src').mkdir()
            (root / 'src/alpha.ts').write_text('export const synthetic = 1;')
            git('add', '.')
            git('commit', '-qm', 'synthetic change')
            tail = '\nREADME-Update: reviewed\nREADME-Update-Reason: synthetic fixture\n'
            for declaration in ['Scenario-Test: none', '', 'Scenario-Test: SC-001',
                                'Scenario-Test: SYN-101\nScenario-Test: SYN-202',
                                'Scenario-Test: SYN-101', 'Scenario-Test: ALL']:
                with self.subTest(declaration=declaration):
                    result = subprocess.run(['python3', str(ADAPTER), str(VALIDATOR), directory, base],
                                            input='Synthetic body\n' + declaration + tail,
                                            text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    payload = json.loads(result.stdout)
                    self.assertEqual([line for line in payload['body'].splitlines() if line.startswith('Scenario-Test:')],
                                     ['Scenario-Test: SYN-101, SYN-202'])
                    self.assertIn(tail.strip(), payload['body'])
                    self.assertTrue(payload['changed'])
                    again = subprocess.run(['python3', str(ADAPTER), str(VALIDATOR), directory, base],
                                           input=payload['body'], text=True, capture_output=True)
                    self.assertEqual(again.returncode, 0, again.stderr)
                    self.assertFalse(json.loads(again.stdout)['changed'])
            failed = subprocess.run(['python3', str(ADAPTER), str(VALIDATOR), directory, 'missing-base'],
                                    input=tail, text=True, capture_output=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn('cannot diff', failed.stderr)
            missing = subprocess.run(['python3', str(ADAPTER), str(root / 'absent.py'), directory, base],
                                     input=tail, text=True, capture_output=True)
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn('absent.py', missing.stderr)


    def _real_base(self):
        try:
            return subprocess.check_output(
                ['git', '-C', str(ROOT), 'merge-base', 'HEAD', 'origin/main'],
                text=True, stderr=subprocess.DEVNULL).strip()
        except (subprocess.CalledProcessError, FileNotFoundError):
            return ''

    def test_real_repository_change_set_declares_every_affected_scenario(self):
        """#842 fixture: the branch's real diff must union every matching pattern."""
        base = self._real_base()
        if not base:
            self.skipTest('origin/main is not available in this checkout')
        head = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
        if base == head:
            self.skipTest('the branch has no committed change against origin/main yet')
        changed = subprocess.check_output(
            ['git', '-C', str(ROOT), 'diff', '--name-only', base, 'HEAD'], text=True).split()
        self.assertTrue(changed, 'the branch must change at least one file')
        result = subprocess.run(['python3', str(ADAPTER), str(VALIDATOR), str(ROOT), base],
                                input='Real fixture body\n', text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        declared = [line.strip() for line in json.loads(result.stdout)['body'].splitlines()
                    if line.startswith('Scenario-Test:')]
        self.assertLessEqual(len(declared), 1, declared)
        ids = ({item.strip() for item in declared[0].split(':', 1)[1].split(',') if item.strip()}
               if declared else set())
        registry = json.loads((ROOT / 'docs/testing/scenario-registry.json').read_text())
        import fnmatch
        expected = set()
        for scenario in registry['scenarios']:
            for pattern in scenario.get('change_patterns', []):
                if any(fnmatch.fnmatch(path, pattern) for path in changed):
                    expected.add(scenario['id'])
                    break
        if expected:
            self.assertEqual(len(declared), 1, declared)
        self.assertTrue(expected <= ids, f'missing {sorted(expected - ids)} for {changed}')


if __name__ == '__main__':
    unittest.main()
