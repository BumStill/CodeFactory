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

    def test_released_features_refresh_t1(self):
        """T1: capabilities shipped since the last README refresh must be visible."""
        text = (ROOT / 'README.md').read_text()
        features = text.split('## Features', 1)[1].split('## Install', 1)[0]
        for phrase in (
            'page by page',                    # R1 vision bridge renders .pptx/.pdf pages
            'resumes the interrupted turn',    # R2 mid-turn restart resume
            'hard ceiling',                    # R3 subscription quota is a hard cap
            'survives a restart',              # R3 the wait itself outlives a restart
            'session-owned git',               # R4 trusted mode allows local commits
            'segment by segment',              # R4 compound commands judged per segment
            'generated from the change set',   # R5 PR metadata auto-generated
            'no longer ends a session',        # R5 system wakes keep the wait alive
            'incident',                        # R6 incident workspace rescue
            'delivered after a restart',       # R7 durable wake delivery
        ):
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
