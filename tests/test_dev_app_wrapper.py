import json
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


class DevAppWrapperTests(unittest.TestCase):
    def test_wrapper_uses_a_separate_tauri_identifier(self) -> None:
        config_path = REPO_ROOT / "src-tauri" / "tauri.dev.conf.json"
        config = json.loads(config_path.read_text(encoding="utf-8"))
        self.assertEqual(config["identifier"], "com.codefactory.dev")

        wrapper = (REPO_ROOT / "scripts" / "install-dev-app-wrapper.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            'TAURI_CONFIG="${CODEFACTORY_DEV_TAURI_CONFIG:-src-tauri/tauri.dev.conf.json}"',
            wrapper,
        )
        self.assertIn('--config "$TAURI_CONFIG"', wrapper)


if __name__ == "__main__":
    unittest.main()
