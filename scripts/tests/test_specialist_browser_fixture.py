"""Execute the browser init callback without launching Chromium; not UI evidence."""
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]

class BrowserFixtureContract(unittest.TestCase):
    def test_native_observation_list_has_epoch_and_records(self):
        script = r'''
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync('scripts/verify-specialistbots-headless.mjs','utf8');
const start = source.indexOf('await page.addInitScript(');
const end = source.indexOf('const fixture =', start);
assert(start >= 0 && end > start);
const window = {};
const page = {addInitScript: fn => fn()};
(async () => {
  await vm.runInNewContext('(async()=>{' + source.slice(start,end) + '})()', {window,page});
  const result = await window.__TAURI_INTERNALS__.invoke('list_specialist_observations',{project:'/synthetic'});
  assert(Number.isSafeInteger(result.epoch) && result.epoch >= 0, 'native status requires an epoch');
  assert(Array.isArray(result.records), 'native status requires records');
})().catch(error => {console.error(error);process.exitCode=1;});
'''
        result = subprocess.run(['node','-e',script],cwd=ROOT,capture_output=True,text=True)
        self.assertEqual(result.returncode,0,result.stderr)
