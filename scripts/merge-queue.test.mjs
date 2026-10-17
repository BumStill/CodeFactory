// U31 merge queue — requirement tests (CF-MQ-R1 … R7).
//
// These are pure-logic tests over synthetic PR fixtures with an injected fake
// GitHub adapter: no network, no real repository. They pin the queue semantics
// the spec requires.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createGhAdapter } from './merge-queue.mjs';

import {
  REQUIRED_CHECKS,
  ARM_LABEL,
  HOLD_LABEL,
  evaluateEligibility,
  orderQueue,
  runQueue,
  versionBumpHandoff,
  verifyRequiredChecks,
} from './merge-queue.mjs';

// ---------------------------------------------------------------------------
// Synthetic fixtures
// ---------------------------------------------------------------------------

const SHA_A = 'a'.repeat(40);
const SHA_B = 'b'.repeat(40);
const SHA_C = 'c'.repeat(40);

function greenChecks(headSha, names = REQUIRED_CHECKS) {
  return names.map((name) => ({
    name,
    status: 'completed',
    conclusion: 'success',
    headSha,
  }));
}

function pr(overrides = {}) {
  const headSha = overrides.headSha ?? SHA_A;
  return {
    number: 1,
    title: 'fix: something',
    headRefName: 'feature/one',
    headSha,
    isDraft: false,
    labels: [ARM_LABEL],
    checks: greenChecks(headSha),
    mergeStateStatus: 'CLEAN',
    readyAt: 1000,
    commitMessages: ['fix: something'],
    ...overrides,
  };
}

// A stateful fake adapter. `behind` seeds which PRs start behind main; a merge
// moves main so the still-queued PRs become behind. `combinedFailures` maps a
// PR number to a check name that fails once the branch is updated onto the
// combined state (the R2 counter-example).
function fakeAdapter(prs, opts = {}) {
  const state = new Map(prs.map((p) => [p.number, structuredClone(p)]));
  const calls = { merge: [], update: [], blocked: [] };
  const behind = new Set(opts.behind ?? []);
  const combinedFailures = opts.combinedFailures ?? {};

  return {
    calls,
    _state: state,
    async listOpenPrs() {
      return [...state.values()].map((p) => structuredClone(p));
    },
    async refreshPr(number) {
      return structuredClone(state.get(number));
    },
    async updateBranch(number) {
      calls.update.push(number);
      const p = state.get(number);
      const failing = combinedFailures[number];
      if (failing) {
        p.checks = greenChecks(p.headSha).map((c) =>
          c.name === failing ? { ...c, conclusion: 'failure' } : c,
        );
      }
      p.mergeStateStatus = 'CLEAN';
      behind.delete(number);
      return p.headSha;
    },
    async merge(number, { expectedHead } = {}) {
      const p = state.get(number);
      if (!p.labels.includes(ARM_LABEL)) {
        return { merged: false, reason: 'not armed for the queue' };
      }
      if (expectedHead !== p.headSha) {
        return { merged: false, reason: 'head moved since the merge was armed' };
      }
      calls.merge.push({ number, expectedHead });
      state.delete(number);
      for (const other of state.values()) other.mergeStateStatus = 'BEHIND';
      return { merged: true, sha: `merge-${number}` };
    },
    async markBlocked(number, reason) {
      calls.blocked.push({ number, reason });
      state.delete(number);
    },
  };
}

// ---------------------------------------------------------------------------
// CF-MQ-R1 / R2 — eligibility and ordering
// ---------------------------------------------------------------------------

test('R1: two green, armed PRs are both eligible and ordered FIFO by readiness', () => {
  const a = pr({ number: 11, readyAt: 2000 });
  const b = pr({ number: 12, readyAt: 1000 });
  const ordered = orderQueue([a, b].filter((p) => evaluateEligibility(p).eligible));
  assert.deepEqual(
    ordered.map((p) => p.number),
    [12, 11],
  );
});

test('R2/R3: a check that ran on an older base does not certify the current head', () => {
  // The GREEN checks are bound to SHA_A but the PR head already moved to SHA_B.
  const stale = pr({ headSha: SHA_B, checks: greenChecks(SHA_A) });
  const verdict = evaluateEligibility(stale);
  assert.equal(verdict.eligible, false);
  assert.match(verdict.reason, /current head/);
});

test('R1: a PR without the delivery arm marker does not enter the queue', () => {
  const verdict = evaluateEligibility(pr({ labels: [] }));
  assert.equal(verdict.eligible, false);
  assert.match(verdict.reason, /armed/);
});

test('R5: a hold label keeps a PR out of the queue without deadlocking', () => {
  const verdict = evaluateEligibility(pr({ labels: [ARM_LABEL, HOLD_LABEL] }));
  assert.equal(verdict.eligible, false);
  assert.match(verdict.reason, /hold/i);
});

// ---------------------------------------------------------------------------
// CF-MQ-R3 — the queue refuses to run on a weakened ruleset (fail closed)
// ---------------------------------------------------------------------------

function rulesetWith(contexts = REQUIRED_CHECKS, strict = true) {
  return {
    rules: [
      { type: 'deletion' },
      {
        type: 'required_status_checks',
        parameters: {
          strict_required_status_checks_policy: strict,
          required_status_checks: contexts.map((context) => ({ context, integration_id: 15368 })),
        },
      },
    ],
  };
}

test('R3: the live ruleset with all six checks and strict mode passes the guard', () => {
  assert.equal(verifyRequiredChecks(rulesetWith()).ok, true);
});

test('R3: a ruleset missing a required check fails the guard (a candidate cannot drop a check)', () => {
  const weakened = REQUIRED_CHECKS.filter((c) => c !== 'scenario-gate-pr');
  const verdict = verifyRequiredChecks(rulesetWith(weakened));
  assert.equal(verdict.ok, false);
  assert.deepEqual(verdict.missing, ['scenario-gate-pr']);
});

test('R3: disabling the strict up-to-date policy fails the guard (R2 would be weakened)', () => {
  const verdict = verifyRequiredChecks(rulesetWith(REQUIRED_CHECKS, false));
  assert.equal(verdict.ok, false);
  assert.equal(verdict.strict, false);
});

test('R2: a PR that conflicts with the PR ahead is dropped, not merged', async () => {
  const conflicting = pr({ number: 91, readyAt: 1, mergeStateStatus: 'DIRTY' });
  const adapter = fakeAdapter([conflicting, pr({ number: 92, readyAt: 2 })]);
  const outcomes = await runQueue(adapter, {});
  assert.equal(outcomes.find((o) => o.number === 91).status, 'dropped');
  assert.deepEqual(
    outcomes.filter((o) => o.status === 'merged').map((o) => o.number),
    [92],
  );
});

// ---------------------------------------------------------------------------
// CF-MQ-R1 — end-to-end ordering (two PRs green at the same time)
// ---------------------------------------------------------------------------

test('R1: two green PRs merge in order with zero manual catch-up', async () => {
  const adapter = fakeAdapter(
    [pr({ number: 21, readyAt: 1 }), pr({ number: 22, readyAt: 2 })],
    { behind: [22] }, // 22 is behind main; the queue must catch it up itself
  );
  const outcomes = await runQueue(adapter, {});
  assert.deepEqual(
    outcomes.filter((o) => o.status === 'merged').map((o) => o.number),
    [21, 22],
  );
  // 22 required a catch-up the queue performed itself, with no human step.
  assert.deepEqual(adapter.calls.update, [22]);
});

test('R1: three PRs in a row all merge in order', async () => {
  const adapter = fakeAdapter([
    pr({ number: 31, readyAt: 1 }),
    pr({ number: 32, readyAt: 2 }),
    pr({ number: 33, readyAt: 3 }),
  ]);
  const outcomes = await runQueue(adapter, {});
  assert.deepEqual(
    outcomes.filter((o) => o.status === 'merged').map((o) => o.number),
    [31, 32, 33],
  );
});

// ---------------------------------------------------------------------------
// CF-MQ-R2 — counter-example: the PR behind fails on the combined state
// ---------------------------------------------------------------------------

test('R2: the PR behind is verified on the combined state and is not merged when it fails', async () => {
  const adapter = fakeAdapter(
    [pr({ number: 41, readyAt: 1 }), pr({ number: 42, readyAt: 2 })],
    { behind: [42], combinedFailures: { 42: 'check-rust' } },
  );
  const outcomes = await runQueue(adapter, {});
  const merged = outcomes.filter((o) => o.status === 'merged').map((o) => o.number);
  assert.deepEqual(merged, [41]); // 42 never merged
  const dropped = outcomes.find((o) => o.number === 42 && o.status === 'dropped');
  assert.ok(dropped, '42 must be dropped');
  assert.match(dropped.reason, /check-rust/);
});

// ---------------------------------------------------------------------------
// CF-MQ-R4 — a failing PR leaves with a plain reason; the rest continue
// ---------------------------------------------------------------------------

test('R4: a PR with a failing required check is dropped with a plain reason and the queue continues', async () => {
  const failing = pr({ number: 51, readyAt: 1 });
  failing.checks = failing.checks.map((c) =>
    c.name === 'scenario-gate-pr' ? { ...c, conclusion: 'failure' } : c,
  );
  const adapter = fakeAdapter([failing, pr({ number: 52, readyAt: 2 })]);
  const outcomes = await runQueue(adapter, {});
  assert.deepEqual(
    outcomes.filter((o) => o.status === 'merged').map((o) => o.number),
    [52],
  );
  const dropped = outcomes.find((o) => o.number === 51);
  assert.equal(dropped.status, 'dropped');
  assert.match(dropped.reason, /scenario-gate-pr/);
});

// ---------------------------------------------------------------------------
// CF-MQ-R5 — release-flow compatibility
// ---------------------------------------------------------------------------

test('R5: the auto-release version-bump PR goes through the queue with no deadlock', async () => {
  const bump = pr({
    number: 61,
    title: 'chore: bump version to 1.82.11',
    readyAt: 2,
    commitMessages: ['chore: bump version to 1.82.11'],
  });
  const adapter = fakeAdapter([pr({ number: 60, readyAt: 1 }), bump], {
    behind: [61],
  });
  const outcomes = await runQueue(adapter, {});
  assert.deepEqual(
    outcomes.filter((o) => o.status === 'merged').map((o) => o.number),
    [60, 61],
  );
  // The merge is bound to the exact head at merge time (expected_head_sha).
  const bumpMerge = adapter.calls.merge.find((m) => m.number === 61);
  assert.equal(bumpMerge.expectedHead, SHA_A);
});

test('R5: versionBumpHandoff identifies an auto-release bump PR without a deadlock judgement', () => {
  assert.equal(versionBumpHandoff('chore: bump version to 1.82.11'), true);
  assert.equal(versionBumpHandoff('fix: a real fix'), false);
});

// ---------------------------------------------------------------------------
// CF-MQ-R6 — a stopped task must not keep merging in the background
// ---------------------------------------------------------------------------

test('R6: a stop request halts the queue before the next merge', async () => {
  let merged = 0;
  const adapter = fakeAdapter([
    pr({ number: 71, readyAt: 1 }),
    pr({ number: 72, readyAt: 2 }),
    pr({ number: 73, readyAt: 3 }),
  ]);
  const realMerge = adapter.merge.bind(adapter);
  adapter.merge = async (n, o) => {
    const r = await realMerge(n, o);
    if (r.merged) merged += 1;
    return r;
  };
  const outcomes = await runQueue(adapter, { stopRequested: () => merged >= 1 });
  assert.equal(merged, 1);
  assert.equal(outcomes.at(-1).status, 'stopped');
});

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// M94 / M78 / squash trailer regressions (synthetic GitHub client only).
// ---------------------------------------------------------------------------
function ghFixture({ failure, messages = ['fix: synthetic change'] } = {}) {
  const calls = [];
  const open = new Map([1, 2].map((number) => [number, {
    number, title: 'fix: synthetic change', headRefName: `synthetic-${number}`,
    headRefOid: String(number).repeat(40), isDraft: false,
    labels: [{ name: ARM_LABEL }], mergeStateStatus: 'CLEAN',
  }]));
  const run = (args) => {
    calls.push(args);
    const number = Number(args[3]);
    if (args[1] === 'label') return '';
    if (args[2] === 'list') return JSON.stringify([...open.values()]);
    if (args[1] === 'api' && args[2].endsWith('/check-runs')) {
      const sha = args[2].split('/').at(-2);
      if (failure === 'checks' && sha === '1'.repeat(40)) throw Error('synthetic checks error');
      return JSON.stringify({ check_runs: greenChecks(sha).map((c) => ({
        ...c, head_sha: sha, conclusion: failure === 'label' && sha === '1'.repeat(40) ? 'failure' : 'success',
      })) });
    }
    if (args[1] === 'api' && args[2].includes('/commits')) {
      return JSON.stringify(messages.map((message) => ({ commit: { message } })));
    }
    if (args[2] === 'edit' && number === 1 && failure === 'label') throw Error('synthetic label error');
    if (args[2] === 'edit' || args[2] === 'comment') return '';
    if (args[2] === 'merge') {
      if (number === 1 && failure === 'merge') throw Error('synthetic merge error');
      open.delete(number);
      return '';
    }
    if (args[2] === 'view' && args.at(-1) === 'title') {
      return JSON.stringify({ title: 'fix: synthetic change' });
    }
    if (args[2] === 'view') return JSON.stringify({ state: 'MERGED', mergeCommit: { oid: SHA_C } });
    throw Error(`Unexpected synthetic gh call: ${args.join(' ')}`);
  };
  return { calls, adapter: createGhAdapter({ repo: 'synthetic/queue', run }) };
}

for (const failure of ['label', 'merge', 'checks']) {
  test(`M94: synthetic gh ${failure} error on first PR does not stop second PR`, async () => {
    const { adapter, calls } = ghFixture({ failure });
    const outcomes = await runQueue(adapter, { maxIterations: 4 });
    assert.ok(outcomes.some((o) => o.number === 2 && o.status === 'merged'));
    assert.ok(outcomes.some((o) => o.number === 1 && o.status === 'error' && /synthetic/.test(o.reason)));
    assert.ok(calls.filter((a) => a[2] === 'merge' && a[3] === '1').length <= 1);
  });
}

test('M94: fixed blocked label is ensured and detailed reason is commented', async () => {
  const { adapter, calls } = ghFixture();
  await adapter.markBlocked(1, 'required check synthetic failed');
  assert.ok(calls.some((a) => a[1] === 'label' && a.includes('merge-queue: blocked')));
  const edit = calls.find((a) => a[2] === 'edit');
  assert.equal(edit.at(-1), 'merge-queue: blocked');
  assert.match(calls.find((a) => a[2] === 'comment').at(-1), /required check synthetic failed/);
});

test('M78: arming triggers only trusted default-branch queue tooling', () => {
  const workflow = readFileSync(new URL('../.github/workflows/merge-queue.yml', import.meta.url), 'utf8');
  assert.match(workflow, /pull_request_target:\s*\n\s*types: \[labeled\]/);
  assert.match(workflow, /if:.*github.event_name != 'pull_request_target'.*github.event.label.name == 'merge-queue: armed'/);
  assert.match(workflow, /ref: \$\{\{ github.event.repository.default_branch \}\}/);
  assert.doesNotMatch(workflow, /ref:.*(?:head|pull_request)/);
  assert.match(workflow, /schedule:/);
  assert.match(workflow, /workflow_run:/);
});

for (const [name, messages] of [
  ['single', ['fix: single\n\nRelease-Urgency: immediate\nCo-Authored-By: Synthetic <synthetic@example.invalid>']],
  ['multiple', ['fix: first\n\nRelease-Urgency: immediate', 'fix: next\n\nCo-Authored-By: Synthetic <synthetic@example.invalid>']],
  ['non-final paragraph', ['fix: footer misplaced\n\nRelease-Urgency: immediate\nCo-Authored-By: Synthetic <synthetic@example.invalid>\n\nLater prose']],
]) {
  test(`squash: ${name} trailers are final and recognized by plan_release`, async () => {
    const { adapter, calls } = ghFixture({ messages });
    await adapter.merge(1, { expectedHead: SHA_A });
    const merge = calls.find((a) => a[2] === 'merge');
    assert.ok(merge.includes('--body'));
    assert.equal(merge[merge.indexOf('--subject') + 1], 'fix: synthetic change');
    const body = merge[merge.indexOf('--body') + 1];
    const footer = body.trim().split(/\n\s*\n/).at(-1);
    assert.match(footer, /^Release-Urgency: immediate/m);
    assert.match(footer, /^Co-Authored-By: Synthetic/m);
    const dir = mkdtempSync(join(tmpdir(), 'queue-trailers-'));
    const git = (...args) => execFileSync('git', ['-C', dir, ...args], { encoding: 'utf8' });
    try {
      git('init', '-q');
      git('config', 'user.name', 'Synthetic');
      git('config', 'user.email', 'synthetic@example.invalid');
      git('commit', '--allow-empty', '-qm', 'chore: initial');
      git('commit', '--allow-empty', '-qm', `fix: synthetic change\n\n${body}`);
      const plan = JSON.parse(execFileSync('python3', [
        decodeURIComponent(new URL('../tools/release/plan_release.py', import.meta.url).pathname),
        '--repo', dir, '--range', 'HEAD~1..HEAD',
      ], { encoding: 'utf8' }));
      assert.equal(plan.immediate, 1);
      assert.equal(plan.slot, 'patch');
      assert.equal(plan.skip, false);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });
}

// Observation — the run is traceable
// ---------------------------------------------------------------------------

test('Observation: the run emits one traceable outcome per PR touched', async () => {
  const adapter = fakeAdapter(
    [pr({ number: 81, readyAt: 1 }), pr({ number: 82, readyAt: 2 })],
    { behind: [82], combinedFailures: { 82: 'check-frontend' } },
  );
  const outcomes = await runQueue(adapter, {});
  assert.equal(outcomes.filter((o) => o.number === 81)[0].status, 'merged');
  assert.equal(outcomes.filter((o) => o.number === 82)[0].status, 'dropped');
});
