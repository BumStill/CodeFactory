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


if __name__ == '__main__':
    unittest.main()
