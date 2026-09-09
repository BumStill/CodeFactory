"""Fail-first delegation checks; behavioral isolation tests live in Rust."""

import pathlib
import json
import subprocess
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]


class DesktopIsolationContractTests(unittest.TestCase):
    def test_nonprotected_cli_smokes_reject_world_requests_before_runtime(self):
        source = (ROOT / "src-tauri/src/lib.rs").read_text()
        for name in ["history_session", "delivery_recovery", "managed_workspace_cleanup", "evolution", "browser_session", "update_upgrade", "browser_chrome_attach", "headless"]:
            body = source.split(f"pub fn run_{name}_smoke_cli() -> bool {{", 1)[1].split("\n}", 1)[0]
            with self.subTest(smoke=name):
                self.assertIn("desktop_context::reject_world_request_for_cli();", body)
                self.assertLess(body.index("return false;"), body.index("desktop_context::reject_world_request_for_cli();"))
                self.assertLess(body.index("desktop_context::reject_world_request_for_cli();"), body.index("tokio::runtime::Builder"))

    def test_context_is_fixed_before_any_builder_or_plugin(self):
        source = (ROOT / "src-tauri/src/lib.rs").read_text()
        run = source.split("pub fn run() {", 1)[1]
        self.assertTrue(run.lstrip().startswith("let context = tauri::generate_context!();"))
        self.assertLess(run.index("desktop_context::initialize"), run.index("tauri::Builder"))
        self.assertLess(run.index("desktop_context::run_synthetic"), run.index(".plugin("))
        self.assertIn(".run(context)", run)
        self.assertEqual(run.count("generate_context!"), 1)

    def test_secret_guards_wrap_all_real_store_entry_points(self):
        source = (ROOT / "src-tauri/src/secrets.rs").read_text()
        for name, guard in [("get_key", "read_secret"), ("set_key", "write_secret"), ("delete_key", "write_secret")]:
            body = source.split(f"pub fn {name}(", 1)[1].split("\n}", 1)[0]
            self.assertIn(f"desktop_context::{guard}", body)
            self.assertNotIn("fallback_key(", body)
            self.assertNotIn("_os_key(", body)
            self.assertNotIn("legacy_map(", body)

    def test_synthetic_builder_has_no_normal_setup_or_plugins(self):
        source = (ROOT / "src-tauri/src/desktop_context.rs").read_text()
        run = source.split("pub(crate) fn run_synthetic(", 1)[1].split("\n#[cfg(test)]", 1)[0]
        self.assertNotIn(".plugin(", run)
        self.assertNotIn("BRIDGE", run)
        self.assertNotIn("AppState", run)
        self.assertIn(".incognito(true)", run)
        self.assertIn(".on_navigation(", run)
        self.assertIn(".invoke_handler(", run)
        self.assertIn("harden_context", run)
        self.assertIn(".on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)", run)
        self.assertIn(".on_download(|_, _| false)", run)
        self.assertIn(".initialization_script(SYNTHETIC_INPUT_GUARD)", run)

    def test_dom_guard_blocks_paste_drop_and_file_picker_without_reading_data(self):
        source = (ROOT / "src-tauri/src/desktop_context.rs").read_text()
        script = source.split('const SYNTHETIC_INPUT_GUARD: &str = r#"', 1)[1].split('"#;', 1)[0]
        check = """
const vm = require('node:vm');
const assert = require('node:assert/strict');
const listeners = new Map();
vm.runInNewContext(SCRIPT, {window: {addEventListener(name, fn, capture) {
  assert.equal(capture, true); listeners.set(name, fn);
}}});
function fire(name, extra = {}) {
  let denied = 0;
  const event = { preventDefault() { denied++; }, stopImmediatePropagation() { denied++; }, ...extra };
  Object.defineProperty(event, 'clipboardData', {get() {throw Error('must not read clipboard');}});
  listeners.get(name)(event);
  return denied;
}
for (const name of ['paste', 'drop', 'dragenter', 'dragover']) assert.equal(fire(name), 2);
assert.equal(fire('beforeinput', {inputType: 'insertFromPaste'}), 2);
assert.equal(fire('beforeinput', {inputType: 'insertFromDrop'}), 2);
assert.equal(fire('beforeinput', {inputType: 'insertText'}), 0);
assert.equal(fire('click', {target: {closest: () => true}}), 2);
assert.equal(fire('click', {target: {closest: () => null}}), 0);
""".replace("SCRIPT", json.dumps(script))
        result = subprocess.run(["node", "-e", check], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_settings_dispatch_precedes_all_normal_migration_probes(self):
        source = (ROOT / "src-tauri/src/config/settings.rs").read_text()
        load = source.split("pub fn load() -> Settings {", 1)[1].split("\npub fn default_git_remote_token_ref", 1)[0]
        self.assertLess(load.index("DesktopContext::Synthetic"), load.index("let new_path"))
        self.assertLess(load.index("return load_synthetic"), load.index("release_config_path().exists()"))
        self.assertLess(load.index("DesktopContext::Rejected"), load.index("legacy_config_path().exists()"))


if __name__ == "__main__":
    unittest.main()
