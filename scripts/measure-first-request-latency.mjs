// SPDX-License-Identifier: Apache-2.0
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';

const binary = resolve(process.argv[2] ?? '.codefactory-cache/cargo-target-m4b/debug/codefactory');
const home = mkdtempSync(join(tmpdir(), 'cf-latency-home-'));
try {
  writeFileSync(join(home, 'synthetic-only'), 'synthetic\n');
  const result = spawnSync(binary, ['--measure-first-request-latency'], {
    cwd: tmpdir(), encoding: 'utf8', timeout: 180_000,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home,
      CODEFACTORY_LATENCY_FIXTURE_HOME: home, HTTP_PROXY: '', HTTPS_PROXY: '',
      ALL_PROXY: '', NO_PROXY: '127.0.0.1,localhost', GIT_CONFIG_GLOBAL: '/dev/null',
      GIT_CONFIG_NOSYSTEM: '1' },
  });
  assert.equal(result.status, 0, result.stderr || String(result.error));
  const receipt = JSON.parse(result.stdout);
  assert.equal(receipt.synthetic_only, true);
  assert.equal(receipt.rounds.length, 5);
  for (const round of receipt.rounds) {
    for (const stage of ['admission', 'workspace_checkpoint', 'route_quota', 'quota_refresh_lock', 'context_assembly', 'skills_memory', 'delivery_readiness', 'provider_post', 'admission_to_http']) {
      assert.ok(Number.isFinite(round[stage]) && round[stage] >= 0, `missing/invalid ${stage}`);
    }
    assert.ok(round.admission_to_http < 30_000, 'first request exceeded fixture deadline');
  }
  for (const stage of Object.keys(receipt.median_ms)) {
    const sorted = receipt.rounds.map(round => round[stage]).sort((a,b) => a-b);
    assert.equal(receipt.median_ms[stage], sorted[2]);
  }
  console.log(JSON.stringify(receipt, null, 2));
} finally {
  rmSync(home, { recursive: true, force: true });
}
