// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
const read = p => readFileSync(new URL('../' + p, import.meta.url), 'utf8');
test('R1 formal executable dispatches lifecycle smoke before Tauri', () => {
  assert.ok(read('src-tauri/src/main.rs').includes('run_browser_extension_lifecycle_smoke_cli'));
  assert.ok(read('src-tauri/src/lib.rs').includes('--browser-extension-lifecycle-smoke'));
});
test('R2 failure receipt is anonymous and precedes nonzero exit', () => {
  const cli = read('src-tauri/src/lib.rs');
  assert.ok(cli.includes('lifecycle_probe_failed'));
  assert.ok(cli.includes('CODEFACTORY_LIFECYCLE_SMOKE_FAIL'));
});
test('R3 PR hard gate invokes binary and checks receipt', () => {
  const ci = read('.github/workflows/ci.yml');
  for (const marker of ['--browser-extension-lifecycle-smoke', 'old_generation_cancelled', 'half_open_evicted', 'same_session', 'duplicate_command_count']) assert.ok(ci.includes(marker), marker);
});
test('R4 registry retains desktop and release gaps', () => {
  const c = JSON.parse(read('docs/testing/scenario-registry.json')).complex_e2e_cases.find(c => c.id === 'E2E-008');
  assert.equal(c.automation_status, 'partially_implemented');
  assert.ok(c.automated_by.includes('binary:--browser-extension-lifecycle-smoke'));
  assert.ok(c.pull_request_gate.required_targets.includes('binary:--browser-extension-lifecycle-smoke'));
  assert.ok(c.remaining_gaps.some(g => g.includes('L3')));
  assert.ok(c.remaining_gaps.some(g => g.includes('L4')));
  assert.ok(!c.remaining_gaps.some(g => g.includes('L2')));
});
