// SPDX-License-Identifier: Apache-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

// Execute the actual extension capture function, not a second implementation.
const source = readFileSync(new URL('../extension/background.js', import.meta.url), 'utf8');
const capture = source.slice(source.indexOf('async function captureVisibleTab('), source.indexOf('/**\n * Tabs the user has open'));
function run(tab, after = tab) {
  const effects = [];
  let queries = 0;
  const context = vm.createContext({
    setTimeout, chrome: {
      tabs: {
        get: async () => tab,
        query: async () => [queries++ ? after : tab],
        update: async () => { effects.push('activate'); },
        captureVisibleTab: async () => { effects.push('capture'); return 'data:image/png;base64,iVBORw0KGgo='; },
      },
      windows: { update: async () => { effects.push('focus'); } },
    },
  });
  return { result: vm.runInContext(`${capture}; captureVisibleTab(42)`, context), effects };
}
const active = { id: 42, windowId: 7, active: true, url: 'https://synthetic.example/deck' };
test('M194 R5/R6 capture refuses background tabs without activating or focusing', async () => {
  const { result, effects } = run({ ...active, active: false });
  await assert.rejects(result, /TAB_NOT_VISIBLE/);
  assert.deepEqual(effects, []);
});
test('M194 R5/R6 capture refuses non-web tabs', async () => {
  const { result, effects } = run({ ...active, url: 'chrome://settings' });
  await assert.rejects(result, /TAB_NOT_FOUND/);
  assert.deepEqual(effects, []);
});
test('M194 R5 capture returns the named active tab pixels', async () => {
  const { result, effects } = run(active);
  assert.equal((await result).tab_id, 42);
  assert.deepEqual(effects, ['capture']);
});
test('M194 R5 capture discards image if the user switches tabs during capture', async () => {
  const { result } = run(active, { ...active, id: 99 });
  await assert.rejects(result, /TAB_NOT_VISIBLE/);
});
