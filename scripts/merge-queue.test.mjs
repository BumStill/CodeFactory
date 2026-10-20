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
  BLOCKED_LABEL_PREFIX,
  WAKE_LABELS,
  resolveWake,
  evaluateEligibility,
  orderQueue,
  parseBlockedMarker,
  runQueue,
  drainQueue,
  versionBumpHandoff,
  verifyRequiredChecks,
  releaseMergedBatch,
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
  const calls = { merge: [], update: [], blocked: [], unblocked: [] };
  const behind = new Set(opts.behind ?? []);
  const combinedFailures = opts.combinedFailures ?? {};

  return {
    calls,
    _state: state,
    async unmarkBlocked(number) {
      calls.unblocked.push(number);
      const p = state.get(number);
      if (p) p.labels = p.labels.filter((label) => label !== BLOCKED_LABEL_PREFIX);
    },
    async listOpenPrs() {
      return [...state.values()].map((p) => structuredClone(p));
    },
    async refreshPr(number) {
      return structuredClone(state.get(number));
    },
    // M120: default merged commits carry no urgency trailer.
    async commitMessage() {
      return 'fix: synthetic change';
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

// M120 — release decisions use the merged commit, never branch metadata.
for (const [name, messages, plan, expected] of [
  ['immediate', ['fix: one\n\nRelease-Urgency: immediate'], {}, 1],
  ['no trailer', ['fix: one'], {}, 0],
  ['hold batch', ['fix: one\n\nRelease-Urgency: immediate'], { hold: 1 }, 0],
  ['two immediate', ['fix: one\n\nRelease-Urgency: immediate', 'fix: two\n\nRelease-Urgency: immediate'], {}, 1],
  ['prose is not a trailer', ['fix: one\n\nRelease-Urgency: immediate\n\nNot a footer'], {}, 0],
  ['invalid urgency guards batch', ['fix: one\n\nRelease-Urgency: immediate'], { invalid_urgency: 1 }, 0],
]) {
  test(`M120-R1: ${name}`, async () => {
    const dispatches = [], comments = [];
    const merged = messages.map((message, i) => ({ number: i + 1, sha: `merged-${i}`, message }));
    const adapter = {
      async commitMessage(sha) { return merged.find((m) => m.sha === sha).message; },
      async releasePlan(sha) { assert.equal(sha, merged.at(-1).sha); return { hold: 0, invalid_urgency: 0, ...plan }; },
      async dispatchRelease(sha) { dispatches.push(sha); },
      async comment(number, body) { comments.push({ number, body }); },
    };
    await releaseMergedBatch(adapter, merged);
    assert.equal(dispatches.length, expected);
    if (expected) assert.equal(dispatches[0], merged.at(-1).sha);
    if (plan.hold || plan.invalid_urgency) assert.match(comments[0].body, /hold|guard/i);
  });
}

test('M120-R2: dispatch error comments on the PR and rejects the queue run', async () => {
  const adapter = fakeAdapter([pr()]);
  const comments = [];
  adapter.commitMessage = async () => 'fix: one\n\nRelease-Urgency: immediate';
  adapter.releasePlan = async () => ({ hold: 0, invalid_urgency: 0 });
  adapter.dispatchRelease = async () => { throw Error('synthetic dispatch denied'); };
  adapter.comment = async (number, body) => comments.push({ number, body });
  await assert.rejects(runQueue(adapter), /synthetic dispatch denied/);
  assert.equal(comments[0].number, 1);
  assert.match(comments[0].body, /dispatch.*failed.*synthetic dispatch denied/is);
});

test('M120 adapter: fetch newly merged objects before scanning a head-relative tag range', async () => {
  const calls = [];
  const adapter = createGhAdapter({ repo: 'synthetic/queue', run(args) {
    calls.push(args);
    if (args[0] === 'git' && args[1] === 'fetch') return '';
    if (args[0] === 'git' && args[1] === 'describe') {
      assert.equal(args.at(-1), SHA_C);
      return 'v1.0.0\n';
    }
    if (args[0] === 'python3') {
      assert.equal(args.at(-1), `v1.0.0..${SHA_C}`);
      return JSON.stringify({ hold: 1, invalid_urgency: 0 });
    }
    throw Error(`unexpected call ${args.join(' ')}`);
  } });
  assert.equal((await adapter.releasePlan(SHA_C)).hold, 1);
  assert.deepEqual(calls[0], ['git', 'fetch', '--tags', 'origin', 'main']);
});

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
function ghFixture({
  failure,
  messages = ['fix: synthetic change'],
  prBody = '',
  mergedMessage,
  blockedNumbers = [],
  commentsFor = () => [],
  eventsFor = () => [],
  headOverrides = {},
  headCommittedAt = '2026-10-17T00:00:00Z',
} = {}) {
  const calls = [];
  const postedComments = [];
  const open = new Map([1, 2].map((number) => [number, {
    number, title: 'fix: synthetic change', headRefName: `synthetic-${number}`,
    headRefOid: headOverrides[number] ?? String(number).repeat(40), isDraft: false,
    labels: [
      { name: ARM_LABEL },
      ...(blockedNumbers.includes(number) ? [{ name: BLOCKED_LABEL_PREFIX }] : []),
    ],
    mergeStateStatus: 'CLEAN',
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
    if (args[1] === 'api' && args[2].endsWith('/comments')) {
      return JSON.stringify(commentsFor(Number(args[2].split('/')[4])).map((body, i) => ({
        body, created_at: `2026-10-17T00:0${i}:00Z`,
      })));
    }
    if (args[1] === 'api' && args[2].endsWith('/events')) {
      const n = Number(args[2].split('/')[4]);
      return JSON.stringify(eventsFor(n).map((at, i) => ({
        event: 'labeled', label: { name: BLOCKED_LABEL_PREFIX }, created_at: at, id: i,
      })));
    }
    if (args[1] === 'api' && /\/commits\/[0-9a-f]{40}$/.test(args[2])) {
      const merge = calls.findLast((a) => a[2] === 'merge');
      const message = mergedMessage ?? (merge
        ? `${merge[merge.indexOf('--subject') + 1]}\n\n${merge[merge.indexOf('--body') + 1]}`
        : 'fix: synthetic change');
      return JSON.stringify({ commit: { message, committer: { date: headCommittedAt } } });
    }
    if (args[1] === 'api' && args[2].includes('/commits')) {
      return JSON.stringify(messages.map((message) => ({ commit: { message } })));
    }
    if (args[2] === 'edit' && number === 1 && failure === 'label') throw Error('synthetic label error');
    if (args[2] === 'edit' || args[2] === 'comment') {
      if (args[2] === 'comment') postedComments.push(args.at(-1));
      return '';
    }
    if (args[2] === 'merge') {
      if (number === 1 && failure === 'merge') throw Error('synthetic merge error');
      open.delete(number);
      return '';
    }
    if (args[2] === 'view' && /^(title|title,body)$/.test(args.at(-1))) {
      return JSON.stringify({ title: 'fix: synthetic change', body: prBody });
    }
    if (args[2] === 'view') return JSON.stringify({ state: 'MERGED', mergeCommit: { oid: SHA_C } });
    throw Error(`Unexpected synthetic gh call: ${args.join(' ')}`);
  };
  return { calls, postedComments, adapter: createGhAdapter({ repo: 'synthetic/queue', run }) };
}

for (const [name, prBody, messages, expected] of [
  ['body immediate', 'Release-Urgency: immediate', ['fix: synthetic change'], 'immediate'],
  ['body hold', 'Release-Urgency: hold', ['fix: synthetic change\n\nRelease-Urgency: immediate'], 'hold'],
  ['commit hold', 'Release-Urgency: immediate', ['fix: synthetic change\n\nRelease-Urgency: hold'], 'hold'],
  ['body conflict wins', 'Release-Urgency: invalid', ['fix: synthetic change\n\nRelease-Urgency: immediate'], 'invalid'],
  ['no urgency', '', ['fix: synthetic change'], null],
]) {
  test(`M122 R1/R4: ${name}`, async () => {
    const { adapter, calls } = ghFixture({ prBody, messages });
    await adapter.merge(1, { expectedHead: SHA_A, title: 'fix: synthetic change' });
    const merge = calls.find((a) => a[2] === 'merge');
    const body = merge[merge.indexOf('--body') + 1];
    const footer = body.trim().split(/\n\s*\n/).at(-1);
    if (expected) assert.match(footer, new RegExp(`^Release-Urgency: ${expected}$`, 'm'));
    else assert.doesNotMatch(body, /Release-Urgency:/);
    if (expected === 'immediate') {
      const dispatched = [];
      adapter.releasePlan = async () => ({ hold: 0, invalid_urgency: 0 });
      adapter.dispatchRelease = async (sha) => dispatched.push(sha);
      await releaseMergedBatch(adapter, [{ number: 1, sha: SHA_C }]);
      assert.deepEqual(dispatched, [SHA_C]);
    }
  });
}

test('M122 R2/R3: clean prose and one final footer', async () => {
  const { adapter, calls } = ghFixture({
    prBody: 'Release-Urgency: immediate',
    messages: ["fix: synthetic change\n\nfix: synthetic change\n\nMerge branch 'main' into synthetic\n\nUseful detail\n\nCo-Authored-By: Synthetic <synthetic@example.invalid>"],
  });
  await adapter.merge(1, { expectedHead: SHA_A });
  const merge = calls.find((a) => a[2] === 'merge');
  const body = merge[merge.indexOf('--body') + 1];
  assert.doesNotMatch(body, /fix: synthetic change|Merge branch/);
  assert.match(body, /Useful detail/);
  const footer = body.trim().split(/\n\s*\n/).at(-1);
  assert.match(footer, /^Release-Urgency: immediate$/m);
  assert.match(footer, /^Co-Authored-By: Synthetic/m);
});

test('M122 R2: lost merged footer comments and fails the queue run', async () => {
  const { adapter, postedComments } = ghFixture({
    prBody: 'Release-Urgency: immediate', mergedMessage: 'fix: synthetic change',
  });
  await assert.rejects(runQueue(adapter), /trailer/i);
  assert.ok(postedComments.some((body) => /trailer/i.test(body)));
});

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
  // M126: `unlabeled` is part of the trigger because lifting `merge-queue:
  // blocked` is what makes a blocked, green PR mergeable again (2026-10-20 #768
  // sat idle 20+ min on exactly that event). Which labels drain is decided by
  // the unit-tested `wake` predicate below, not by an untested workflow `if:`.
  assert.match(workflow, /pull_request_target:\s*\n\s*types: \[labeled, unlabeled\]/);
  assert.match(workflow, /node scripts\/merge-queue\.mjs wake/);
  assert.match(workflow, /if: steps\.wake\.outputs\.run == 'true'/);
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

// ---------------------------------------------------------------------------
// M101 — a blocked PR is re-evaluated once its head moves (2026-10-17: #693
// stayed `not-eligible: already blocked` for 20 minutes after the conflict fix
// pushed a new, fully green head).
// ---------------------------------------------------------------------------

const BLOCKED = BLOCKED_LABEL_PREFIX;

test('M101-R2: a new head clears the queue block and the PR is evaluated again', async () => {
  const blocked = pr({
    number: 101,
    headSha: SHA_B,
    labels: [ARM_LABEL, BLOCKED],
    blocked: { headSha: SHA_A, at: 1000 },
  });
  const verdict = evaluateEligibility(blocked);
  assert.equal(verdict.eligible, true);
  assert.equal(verdict.clearedBlock, true);
  assert.match(verdict.reason, /SHA_A|aaaaaaaa|superseded|re-evaluat/i);

  const adapter = fakeAdapter([blocked]);
  const outcomes = await runQueue(adapter, {});
  assert.deepEqual(adapter.calls.unblocked, [101], 'the stale block must be removed exactly once');
  assert.ok(outcomes.some((o) => o.number === 101 && o.status === 'unblocked'));
  assert.deepEqual(
    outcomes.filter((o) => o.status === 'merged').map((o) => o.number),
    [101],
  );
});

test('M101-R2: the same blocked head stays blocked and is never retried', async () => {
  const blocked = pr({
    number: 102,
    headSha: SHA_A,
    labels: [ARM_LABEL, BLOCKED],
    blocked: { headSha: SHA_A, at: 1000 },
  });
  const verdict = evaluateEligibility(blocked);
  assert.equal(verdict.eligible, false);
  assert.equal(verdict.clearedBlock ?? false, false);
  assert.match(verdict.reason, /already blocked/i);

  const adapter = fakeAdapter([blocked]);
  const outcomes = await runQueue(adapter, {});
  assert.deepEqual(adapter.calls.merge, [], 'the same failed head must not be retried');
  assert.deepEqual(adapter.calls.blocked, [], 'the queue must not block the same head twice');
  assert.deepEqual(adapter.calls.unblocked, []);
  assert.equal(outcomes.find((o) => o.number === 102).status, 'not-eligible');
});

test('M101-R3: a legacy blocked label without a record is cleared only when the head is newer than the label', () => {
  const legacy = (extra) => pr({
    number: 103,
    headSha: SHA_B,
    labels: [ARM_LABEL, BLOCKED],
    blocked: null,
    ...extra,
  });

  const newer = evaluateEligibility(legacy({ blockedLabelAt: 1000, headCommittedAt: 2000 }));
  assert.equal(newer.eligible, true);
  assert.equal(newer.clearedBlock, true);

  const older = evaluateEligibility(legacy({ blockedLabelAt: 2000, headCommittedAt: 1000 }));
  assert.equal(older.eligible, false);
  assert.match(older.reason, /already blocked/i);

  const unknown = evaluateEligibility(legacy({ blockedLabelAt: null, headCommittedAt: null }));
  assert.equal(unknown.eligible, false, 'no timestamps must stay blocked (conservative)');
});

test('M101-R1: blocking a PR records the exact blocked head in a machine-readable marker', async () => {
  const { adapter, postedComments } = ghFixture();
  await adapter.markBlocked(1, 'conflicts with main', { headSha: SHA_A });
  const body = postedComments.at(-1);
  assert.match(body, /conflicts with main/, 'the human-readable reason is preserved');
  const record = parseBlockedMarker(body);
  assert.ok(record, 'the comment must carry a parseable marker');
  assert.equal(record.headSha, SHA_A);
  assert.equal(typeof record.at, 'number');
});

test('M101-R2: the adapter reads the recorded head back and unblocks a PR whose head moved', async () => {
  const recorded = '1'.repeat(40);
  const current = '2'.repeat(40);
  const { adapter, calls } = ghFixture({
    blockedNumbers: [1],
    headOverrides: { 1: current },
    commentsFor: (n) => (n === 1
      ? [`<!-- merge-queue:blocked head=${recorded} at=2026-10-17T00:00:00Z -->\nMerge queue dropped this PR: conflicts with main`]
      : []),
    eventsFor: () => [],
  });

  const outcomes = await runQueue(adapter, { maxIterations: 4 });
  assert.ok(outcomes.some((o) => o.number === 1 && o.status === 'unblocked'));
  assert.ok(outcomes.some((o) => o.number === 1 && o.status === 'merged'));
  const unblock = calls.find((a) => a[2] === 'edit' && a.includes('--remove-label'));
  assert.ok(unblock, 'the blocked label must be removed');
  assert.equal(unblock.at(-1), BLOCKED);
});

test('M101-R3: a legacy blocked label is unblocked when the head commit is newer than the label event', async () => {
  const { adapter, calls } = ghFixture({
    blockedNumbers: [1],
    commentsFor: () => [],
    eventsFor: () => ['2026-10-16T00:00:00Z'],
    headCommittedAt: '2026-10-17T00:00:00Z',
  });

  const outcomes = await runQueue(adapter, { maxIterations: 4 });
  assert.ok(outcomes.some((o) => o.number === 1 && o.status === 'unblocked'));
  assert.ok(calls.some((a) => a[2] === 'edit' && a.includes('--remove-label')));
});

test('M101-R3: a legacy blocked label stays blocked when the head is not newer than the label', async () => {
  const { adapter, calls } = ghFixture({
    blockedNumbers: [1],
    commentsFor: () => [],
    eventsFor: () => ['2026-10-18T00:00:00Z'],
    headCommittedAt: '2026-10-17T00:00:00Z',
  });

  const outcomes = await runQueue(adapter, { maxIterations: 4 });
  assert.equal(outcomes.find((o) => o.number === 1).status, 'not-eligible');
  assert.ok(!calls.some((a) => a[2] === 'edit' && a.includes('--remove-label')));
  assert.ok(!calls.some((a) => a[2] === 'merge' && a[3] === '1'));
});

// ---------------------------------------------------------------------------
// M114 / R8 — merging must not spend the shared user API budget
// ---------------------------------------------------------------------------

test('M114-R8a: drain exits after ONE listing when no PR is armed', async () => {
  const calls = [];
  const adapter = createGhAdapter({
    repo: 'synthetic/queue',
    run: (args) => {
      calls.push(args.join(' '));
      if (args[1] === 'pr' && args[2] === 'list') return '[]';
      throw new Error(`unexpected call: ${args.join(' ')}`);
    },
  });
  const result = await drainQueue(adapter, {});
  assert.deepEqual(result, { skipped: 'no_armed_pr', plan: [], outcomes: [] });
  assert.equal(calls.length, 1, 'the empty sweep must cost exactly one call');
  assert.ok(calls[0].includes('--label merge-queue: armed'));
});

test('M114-R8c: an unarmed open PR costs no check-runs or blocked-evidence calls', async () => {
  const calls = [];
  const adapter = createGhAdapter({
    repo: 'synthetic/queue',
    run: (args) => {
      calls.push(args.join(' '));
      if (args[1] === 'pr' && args[2] === 'list') {
        return JSON.stringify([
          { number: 1, title: 'plain', headRefName: 'b1', headRefOid: 's1', isDraft: false, labels: [{ name: 'some-other-label' }], mergeStateStatus: 'CLEAN' },
          { number: 2, title: 'armed', headRefName: 'b2', headRefOid: 's2', isDraft: false, labels: [{ name: ARM_LABEL }], mergeStateStatus: 'CLEAN' },
        ]);
      }
      if (Array.isArray(args) && args[1] === 'api' && String(args[2]).includes('/check-runs')) {
        return JSON.stringify({ check_runs: [] });
      }
      if (args[1] === 'api' && String(args[2]).includes('/rules/branches/')) {
        throw new Error('synthetic rules are unavailable');
      }
      throw new Error(`unexpected call: ${args.join(' ')}`);
    },
  });
  const prs = await adapter.listOpenPrs();
  assert.equal(prs.length, 2);
  const checkCalls = calls.filter((call) => call.includes('/check-runs'));
  assert.equal(checkCalls.length, 1, 'only the armed PR may cost a check-runs call');
  assert.ok(checkCalls[0].includes('s2'));
  assert.ok(!calls.some((call) => call.includes('/comments') || call.includes('/events')));
});

test('M114-R8b: the merge loop consumes the shared snapshot instead of listing again', async () => {
  let listCalls = 0;
  const adapter = {
    async listOpenPrs() {
      listCalls += 1;
      return [];
    },
  };
  const outcomes = await runQueue(adapter, {
    initialPrs: [
      { number: 3, title: 'draft', headRefName: 'b3', headSha: 's3', isDraft: true, labels: [], checks: [], mergeStateStatus: 'CLEAN' },
    ],
    maxIterations: 2,
  });
  assert.equal(listCalls, 0, 'plan and merge must share one listing');
  assert.equal(outcomes[0].status, 'not-eligible');
});

test('M114-R8b: merging reuses the listed title instead of another gh pr view', async () => {
  const calls = [];
  const adapter = createGhAdapter({
    repo: 'synthetic/queue',
    run: (args) => {
      calls.push(args.join(' '));
      if (args[1] === 'api' && String(args[2]).includes('/pulls/2/commits')) {
        return JSON.stringify([[{ commit: { message: 'fix: synthetic merge' } }]]);
      }
      if (args[1] === 'pr' && args[2] === 'merge') return '';
      if (args[1] === 'pr' && args[2] === 'view') {
        return JSON.stringify({ state: 'MERGED', mergeCommit: { oid: 'synthetic-sha' } });
      }
      throw new Error(`unexpected call: ${args.join(' ')}`);
    },
  });
  const result = await adapter.merge(2, { expectedHead: 's2', title: 'fix: synthetic merge', prBody: '' });
  assert.equal(result.merged, true);
  assert.ok(
    !calls.some((call) => call.includes('--json title')),
    'the title already came from the shared listing',
  );
});

// ---------------------------------------------------------------------------
// M126 — every signal that can make an armed PR mergeable must wake the queue.
// 2026-10-20 (#768): the queue's own `merge-queue: blocked` label was lifted on
// a CLEAN, armed PR and nothing woke the queue for 20+ minutes, because the
// only label trigger was `labeled` filtered to `merge-queue: armed`.
// ---------------------------------------------------------------------------
test('M126: arming wakes the queue', () => {
  assert.deepEqual(resolveWake('pull_request_target', ARM_LABEL), {
    run: true,
    reason: `queue label "${ARM_LABEL}" changed`,
  });
});

test('M126: clearing the queue block wakes the queue', () => {
  assert.equal(resolveWake('pull_request_target', BLOCKED_LABEL_PREFIX).run, true);
  assert.equal(resolveWake('pull_request_target', `${BLOCKED_LABEL_PREFIX} head=abc`).run, true);
  // Removing the block is an `unlabeled` event, the case #768 missed.
  assert.equal(WAKE_LABELS.includes(BLOCKED_LABEL_PREFIX), true);
});

test('M126: unrelated labels do not drain the queue', () => {
  assert.equal(resolveWake('pull_request_target', 'type: chore').run, false);
  assert.equal(resolveWake('pull_request_target', HOLD_LABEL).run, false);
  assert.deepEqual(resolveWake('pull_request_target', ''), {
    run: false,
    reason: 'pull_request_target without a label name',
  });
});

test('M126: scheduled and CI-completion sweeps always drain', () => {
  for (const event of ['schedule', 'workflow_run', 'workflow_dispatch']) {
    assert.equal(resolveWake(event, '').run, true, event);
  }
});

test('M126: the workflow runs the tested predicate instead of its own label filter', async () => {
  const { readFile } = await import('node:fs/promises');
  const workflow = await readFile(
    new URL('../.github/workflows/merge-queue.yml', import.meta.url),
    'utf8',
  );
  assert.match(workflow, /types: \[labeled, unlabeled\]/);
  assert.match(workflow, /node scripts\/merge-queue\.mjs wake/);
  assert.match(workflow, /if: steps\.wake\.outputs\.run == 'true'/);
  const jobGuard = workflow.split('drain-queue:')[1].split('steps:')[0];
  assert.ok(!/label\.name/.test(jobGuard), 'the job guard must not hardcode a label filter');
  // Trust root unchanged: the queue runs default-branch tooling, never PR code.
  assert.match(workflow, /ref: \$\{\{ github\.event\.repository\.default_branch \}\}/);
});

test('M126: the wake CLI publishes the GITHUB_OUTPUT contract', async () => {
  const { mkdtempSync, readFileSync, rmSync } = await import('node:fs');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { execFileSync } = await import('node:child_process');
  const dir = mkdtempSync(join(tmpdir(), 'm126-'));
  const out = join(dir, 'github_output');
  const repoRoot = new URL('..', import.meta.url);
  const run = (label) =>
    execFileSync(
      process.execPath,
      ['scripts/merge-queue.mjs', 'wake', '--event', 'pull_request_target', '--label', label],
      { env: { ...process.env, GITHUB_OUTPUT: out }, cwd: repoRoot },
    );
  try {
    run(`${BLOCKED_LABEL_PREFIX} head=abc`);
    assert.match(readFileSync(out, 'utf8'), /^run=true$/m);
    run('docs');
    assert.match(readFileSync(out, 'utf8'), /^run=false$/m);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
