from pathlib import Path
import importlib.util
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('readme_contract', ROOT / 'tools/governance/validate_readme_contract.py')
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)

class ReadmeReleasedCapabilities(unittest.TestCase):
    def test_released_features_and_boundaries(self):
        text = (ROOT / 'README.md').read_text()
        features = text.split('## Features', 1)[1].split('## Install', 1)[0]
        for phrase in ('--dispatch', 'Unix domain socket', 'Windows', 'delivery_authorized', '80%', '5-hour', 'Estimated', '60 GiB', 'resumable', 'uncommitted', 'release-only', 'hold', 'Stop', 'PR opened', 'system interruptions'):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, features)
    def test_roadmap_has_no_completed_items(self):
        text = (ROOT / 'README.md').read_text()
        roadmap = text.split('## Roadmap', 1)[1].split('## License', 1)[0]
        self.assertNotIn('[x]', roadmap)
        self.assertIn('Linux build', roadmap)
    def test_evergreen_contract(self):
        self.assertEqual(contract.validate_static(ROOT), [])

if __name__ == '__main__':
    unittest.main()
