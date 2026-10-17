#!/usr/bin/env node
// U31 — Repository merge queue.
//
// GitHub's native merge queue is unavailable to this personal-account repo
// (`repository.mergeQueue` returns null; owner.type == "User"), so this module
// is the equivalent: it lets green, armed PRs enter a queue and merges them one
// at a time, in order, verifying each against the combined state (latest main +
// the PRs ahead of it). The ruleset's strict up-to-date requirement is kept, and
// the queue performs the catch-up itself — nobody updates a branch by hand.
//
// Design notes:
// * The pure logic (`evaluateEligibility`, `orderQueue`, `runQueue`) takes an
//   injected GitHub adapter, so it is fully unit-testable with synthetic PR
//   fixtures and no network. `createGhAdapter` is the only networked part.
// * "Armed" is load-bearing for R6: the queue never merges a PR that the
//   delivery tool has not armed. A stopped task clears the arm marker, so
//   nothing merges in the background after the session ends.
//
// See docs/specs/feature-specs/merge-queue.md.
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';

// The exact required checks of ruleset `main-pr-and-ci-gate` (id 20222077).
// Kept in sync with `.github/rulesets/main.json`; the queue refuses to run if
// the live ruleset diverges (fail closed), so a candidate cannot drop a check.
export const REQUIRED_CHECKS = Object.freeze([
  'agent-bridge-linux',
  'check-frontend',
  'check-rust',
  'governance-baseline',
  'remote-real-app-gui',
  'scenario-gate-pr',
]);

export const ARM_LABEL = 'merge-queue: armed';
export const HOLD_LABEL = 'merge-queue: hold';
export const BLOCKED_LABEL_PREFIX = 'merge-queue: blocked';

// M101 — which head the queue blocked, so a later push is re-evaluated instead
// of being refused forever (2026-10-17: #693 stayed blocked after a green fix).
// The record lives in a hidden marker inside the blocking PR comment: a label
// description is repository-global and cannot hold a per-PR SHA, and the queue
// is stateless between runs, so a local state file would not survive.
const BLOCKED_MARKER_RE =
  /<!--\s*merge-queue:blocked\s+head=([0-9a-f]{7,40})\s+at=([^\s]+)\s*-->/i;

/** Render the machine-readable marker written at the top of a blocking comment. */
export function blockedMarker(headSha, date = new Date()) {
  return `<!-- merge-queue:blocked head=${headSha} at=${date.toISOString()} -->`;
}

/** Parse the marker back out of a PR comment body; null when absent. */
export function parseBlockedMarker(body) {
  if (typeof body !== 'string') return null;
  const match = BLOCKED_MARKER_RE.exec(body);
  if (!match) return null;
  const at = Date.parse(match[2]);
  return { headSha: match[1].toLowerCase(), at: Number.isNaN(at) ? null : at };
}

function shortSha(sha) {
  return typeof sha === 'string' && sha.length > 8 ? sha.slice(0, 8) : String(sha ?? '');
}

function toMillis(value) {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string') {
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? null : parsed;
  }
  return null;
}

/** Highest-order readiness key: when the PR became green+armed. */
function readyAtOf(pr) {
  const value = pr.readyAt;
  if (typeof value === 'number') return value;
  if (typeof value === 'string') {
    const parsed = Date.parse(value);
    if (!Number.isNaN(parsed)) return parsed;
  }
  return Number.MAX_SAFE_INTEGER;
}

/**
 * Decide whether a PR may enter the queue right now.
 *
 * `terminal: true` means the PR cannot merge as-is and must be dropped from the
 * queue (a hard conflict, or a required check that failed on the current head).
 * `terminal: false` means it is merely not ready yet (pending/stale checks,
 * draft, held, unarmed) and should be reconsidered on a later pass.
 * @returns {{eligible: boolean, reason: string, terminal: boolean}}
 */
export function evaluateEligibility(pr, { requiredChecks = REQUIRED_CHECKS } = {}) {
  if (pr.isDraft) {
    return { eligible: false, reason: 'still a draft', terminal: false };
  }
  const labels = pr.labels ?? [];
  if (!labels.includes(ARM_LABEL)) {
    return {
      eligible: false,
      reason: 'not armed; the delivery tool must arm it for the queue',
      terminal: false,
    };
  }
  if (labels.includes(HOLD_LABEL)) {
    return { eligible: false, reason: 'on hold (`merge-queue: hold`)', terminal: false };
  }
  const blockedLabel = labels.find((label) => label.startsWith(BLOCKED_LABEL_PREFIX));
  if (blockedLabel) {
    // M101-R1/R2: the queue records the head it blocked. A different head is a
    // different subject — clear the block and evaluate normally. The same head
    // stays blocked, so a failed head is never retried in a loop.
    const record = pr.blocked && typeof pr.blocked === 'object' ? pr.blocked : null;
    const recordedHead = typeof record?.headSha === 'string' && record.headSha ? record.headSha : null;
    if (recordedHead) {
      if (recordedHead !== pr.headSha) {
        return {
          eligible: true,
          clearedBlock: true,
          reason: `blocked head ${shortSha(recordedHead)} was superseded by ${shortSha(pr.headSha)}; re-evaluating`,
          terminal: false,
        };
      }
      return {
        eligible: false,
        reason: `already blocked by the queue on this head (${shortSha(recordedHead)})`,
        terminal: false,
      };
    }

    // M101-R3: legacy label with no record. Conservative: unblock only when the
    // head's latest commit is strictly newer than the label; if we cannot prove
    // it, keep the block rather than retrying an unknown failed head.
    const labelAt = toMillis(pr.blockedLabelAt) ?? toMillis(record?.at);
    const headAt = toMillis(pr.headCommittedAt);
    if (labelAt !== null && headAt !== null && headAt > labelAt) {
      return {
        eligible: true,
        clearedBlock: true,
        reason: 'the blocked label predates the current head; re-evaluating',
        terminal: false,
      };
    }
    return {
      eligible: false,
      reason: 'already blocked by the queue (no recorded head; keeping the block)',
      terminal: false,
    };
  }

  // Every required check must be a completed success bound to THIS head.
  // A check that ran on an older base cannot certify the current head (R2/R3):
  // it is the difference between "it passed against an old base" and "it passed
  // against the exact content we are about to merge".
  const checks = pr.checks ?? [];
  for (const name of requiredChecks) {
    const onHead = checks.filter((check) => check.name === name);
    if (onHead.length === 0) {
      return { eligible: false, reason: `required check ${name} has not run`, terminal: false };
    }
    if (!onHead.some((check) => check.headSha === pr.headSha)) {
      return {
        eligible: false,
        reason: `required check ${name} is stale; it ran on an older base, not the current head`,
        terminal: false,
      };
    }
    const current = onHead.find((check) => check.headSha === pr.headSha);
    if (current.status !== 'completed') {
      return { eligible: false, reason: `required check ${name} is still running`, terminal: false };
    }
    if (current.conclusion !== 'success') {
      // A failed check on the exact head is a definite failure: drop it (R4).
      return {
        eligible: false,
        reason: `required check ${name} failed on the current head (${current.conclusion})`,
        terminal: true,
      };
    }
  }

  if (pr.mergeStateStatus === 'DIRTY') {
    return {
      eligible: false,
      reason: 'conflicts with main; a human must resolve the merge conflict',
      terminal: true,
    };
  }
  return {
    eligible: true,
    reason: 'green, armed and up to date (or catchable by the queue)',
    terminal: false,
  };
}

/**
 * Fail-closed guard on the live ruleset (R3): the queue must see the exact
 * required-check set and the strict up-to-date policy it is built around. If a
 * candidate branch could drop a check or disable strict mode, the queue refuses
 * to merge anything. This is what stops a PR from certifying itself with a
 * weakened gate.
 */
export function verifyRequiredChecks(ruleset, expected = REQUIRED_CHECKS) {
  const rules = ruleset?.rules ?? [];
  const status = rules.find((rule) => rule.type === 'required_status_checks');
  const params = status?.parameters ?? {};
  const live = new Set((params.required_status_checks ?? []).map((check) => check.context));
  const missing = expected.filter((context) => !live.has(context));
  const extra = [...live].filter((context) => !expected.includes(context));
  const strict = params.strict_required_status_checks_policy === true;
  return { ok: missing.length === 0 && extra.length === 0 && strict, strict, missing, extra };
}

/** FIFO by readiness time; ties broken by PR number so the order is deterministic. */
export function orderQueue(prs) {
  return [...prs].sort((a, b) => readyAtOf(a) - readyAtOf(b) || a.number - b.number);
}

/** True when a PR is the auto-release version bump (chore: bump version …). */
export function versionBumpHandoff(title) {
  const t = (title ?? '').trim();
  if (!/^chore[\s(:!]/.test(t)) return false;
  return /\bbump version to\b/i.test(t) || /^chore:\s*bump version\b/i.test(t);
}

/**
 * Run the queue against an adapter, one merge at a time.
 *
 * The loop re-lists open PRs after every authoritative state change (merge,
 * drop), so the next PR is always evaluated against the freshly moved main.
 * `stopRequested` is consulted before each merge; when it returns true the
 * queue stops and emits a terminal `stopped` outcome (R6).
 */
export async function runQueue(adapter, { stopRequested, maxIterations = 100 } = {}) {
  const outcomes = [];
  const seen = new Set();
  let iterations = 0;

  while (iterations++ < maxIterations) {
    if (typeof stopRequested === 'function' && stopRequested()) {
      outcomes.push({ status: 'stopped' });
      break;
    }

    const prs = await adapter.listOpenPrs();
    if (prs.length === 0) break;

    const eligible = [];
    for (const pr of prs) {
      if (seen.has(pr.number)) continue;
      try {
      if (pr.error) throw new Error(pr.error);
      const verdict = evaluateEligibility(pr);
      if (verdict.eligible) {
        // M101-R2: a block recorded for a head that no longer exists is stale.
        // Remove the label before merging so the next run sees a clean PR.
        if (verdict.clearedBlock) {
          await adapter.unmarkBlocked(pr.number);
          outcomes.push({ number: pr.number, status: 'unblocked', reason: verdict.reason });
        }
        eligible.push(pr);
      } else if (!seen.has(pr.number)) {
        seen.add(pr.number);
        if (verdict.terminal) {
          // A definite failure (hard conflict, or a check failed on this exact
          // head) leaves the queue with a plain-language reason (R4) and the PRs
          // behind it keep going.
          await adapter.markBlocked(pr.number, verdict.reason, { headSha: pr.headSha });
          outcomes.push({ number: pr.number, status: 'dropped', reason: verdict.reason });
        } else {
          outcomes.push({ number: pr.number, status: 'not-eligible', reason: verdict.reason });
        }
      }
      } catch (error) {
        seen.add(pr.number);
        outcomes.push({ number: pr.number, status: 'error', reason: String(error.message ?? error) });
      }
    }

    const ordered = orderQueue(eligible);
    if (ordered.length === 0) break;
    const head = ordered[0];

    let target = head;
    try {
    if (head.mergeStateStatus === 'BEHIND') {
      // The catch-up nobody should do by hand: bring the branch onto latest
      // main, then require the checks to go green on the *combined* head.
      await adapter.updateBranch(head.number, head.headSha);
      target = await adapter.refreshPr(head.number);
      const verdict = evaluateEligibility(target);
      if (!verdict.eligible) {
        const reason = `combined-state verification failed: ${verdict.reason}`;
        await adapter.markBlocked(target.number, reason, { headSha: target.headSha });
        outcomes.push({ number: target.number, status: 'dropped', reason: verdict.reason });
        seen.add(target.number);
        continue;
      }
    }

    const result = await adapter.merge(target.number, {
      expectedHead: target.headSha,
      method: 'squash',
    });
    if (result.merged) {
      outcomes.push({ number: target.number, status: 'merged', sha: result.sha });
      seen.add(target.number);
      continue;
    }
    // Not merged right now (e.g. checks still running): wait, do not spin.
    outcomes.push({ number: target.number, status: 'waiting', reason: result.reason });
    seen.add(target.number);
    } catch (error) {
      seen.add(head.number);
      outcomes.push({ number: head.number, status: 'error', reason: String(error.message ?? error) });
    }
  }
  return outcomes;
}

// ---------------------------------------------------------------------------
// Networked GitHub adapter (not exercised by unit tests)
// ---------------------------------------------------------------------------

export function createGhAdapter({ repo, run = defaultRun } = {}) {
  async function prView(number) {
    const raw = run([
      'gh', 'pr', 'view', String(number), '--repo', repo, '--json',
      'number,title,headRefName,headRefOid,isDraft,labels,mergeStateStatus',
    ]);
    const pr = JSON.parse(raw);
    const checks = await headChecks(pr.headRefOid);
    return mapPr(pr, checks, await blockedFields(pr));
  }

  async function headChecks(headSha) {
    const raw = run(['gh', 'api', `repos/${repo}/commits/${headSha}/check-runs`, '--paginate']);
    const body = JSON.parse(raw);
    return (body.check_runs ?? []).map((check) => ({
      name: check.name,
      status: check.status,
      conclusion: check.conclusion,
      headSha: check.head_sha,
    }));
  }

  // M101: read the blocked-head record lazily — only for PRs that carry the
  // blocked label, so an ordinary queue pass makes no extra API calls.
  async function blockedFields(pr) {
    const names = (pr.labels ?? []).map((label) => label.name);
    if (!names.some((name) => typeof name === 'string' && name.startsWith(BLOCKED_LABEL_PREFIX))) {
      return { blocked: null, blockedLabelAt: null, headCommittedAt: null };
    }
    const comments = JSON.parse(
      run(['gh', 'api', `repos/${repo}/issues/${pr.number}/comments`, '--paginate']),
    );
    const record = [...(comments ?? [])]
      .reverse()
      .map((comment) => parseBlockedMarker(comment.body))
      .find(Boolean) ?? null;
    if (record) return { blocked: record, blockedLabelAt: null, headCommittedAt: null };

    // Legacy label with no marker (M101-R3): compare the label's added time with
    // the head commit time. Missing evidence keeps the block (conservative).
    let blockedLabelAt = null;
    let headCommittedAt = null;
    try {
      const events = JSON.parse(
        run(['gh', 'api', `repos/${repo}/issues/${pr.number}/events`, '--paginate']),
      );
      const times = (events ?? [])
        .filter((event) => event.event === 'labeled'
          && typeof event.label?.name === 'string'
          && event.label.name.startsWith(BLOCKED_LABEL_PREFIX))
        .map((event) => Date.parse(event.created_at))
        .filter((at) => !Number.isNaN(at));
      blockedLabelAt = times.length ? Math.max(...times) : null;
    } catch {
      blockedLabelAt = null;
    }
    try {
      const commit = JSON.parse(run(['gh', 'api', `repos/${repo}/commits/${pr.headRefOid}`]));
      const at = Date.parse(commit?.commit?.committer?.date ?? commit?.commit?.author?.date ?? '');
      headCommittedAt = Number.isNaN(at) ? null : at;
    } catch {
      headCommittedAt = null;
    }
    return { blocked: null, blockedLabelAt, headCommittedAt };
  }

  function mapPr(pr, checks, blocked = {}) {
    return {
      number: pr.number,
      title: pr.title,
      headRefName: pr.headRefName,
      headSha: pr.headRefOid,
      isDraft: pr.isDraft,
      labels: (pr.labels ?? []).map((label) => label.name),
      checks,
      mergeStateStatus: pr.mergeStateStatus,
      readyAt: pr.number,
      blocked: blocked.blocked ?? null,
      blockedLabelAt: blocked.blockedLabelAt ?? null,
      headCommittedAt: blocked.headCommittedAt ?? null,
    };
  }

  return {
    async listOpenPrs() {
      const raw = run([
        'gh', 'pr', 'list', '--repo', repo, '--state', 'open', '--limit', '100', '--json',
        'number,title,headRefName,headRefOid,isDraft,labels,mergeStateStatus',
      ]);
      const list = JSON.parse(raw);
      const out = [];
      for (const pr of list) {
        try {
          const checks = await headChecks(pr.headRefOid);
          out.push(mapPr(pr, checks, await blockedFields(pr)));
        } catch (error) {
          out.push({ ...mapPr(pr, []), error: String(error.message ?? error) });
        }
      }
      return out;
    },
    async refreshPr(number) {
      return prView(number);
    },
    async updateBranch(number, expectedHead) {
      run([
        'gh', 'api', '-X', 'PUT',
        `repos/${repo}/pulls/${number}/update-branch`,
        '-f', `expected_head_sha=${expectedHead}`,
      ]);
    },
    async merge(number, { expectedHead } = {}) {
      const commits = JSON.parse(run([
        'gh', 'api', `repos/${repo}/pulls/${number}/commits`, '--paginate', '--slurp',
      ])).flat();
      // Pin metadata to the same head GitHub is asked to merge. Never execute it.
      const metadata = JSON.parse(run([
        'gh', 'pr', 'view', String(number), '--repo', repo, '--json', 'title',
      ]));
      const messages = commits.map((commit) => commit.commit.message);
      const trailers = [...new Set(messages.flatMap((message) => message.split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => /^(Release-Urgency|Co-Authored-By):\s*\S/i.test(line))))];
      const prose = messages.map((message) => message.split(/\r?\n/)
        .filter((line) => !/^(Release-Urgency|Co-Authored-By):/i.test(line.trim())).join('\n').trim())
        .filter(Boolean).join('\n\n');
      const body = [prose, trailers.join('\n')].filter(Boolean).join('\n\n');
      run([
        'gh', 'pr', 'merge', String(number), '--repo', repo, '--squash',
        '--match-head-commit', expectedHead,
        '--subject', metadata.title ?? 'fix: synthetic change', '--body', body,
      ]);
      const after = JSON.parse(
        run(['gh', 'pr', 'view', String(number), '--repo', repo, '--json', 'state,mergeCommit']),
      );
      if (after.state === 'MERGED') {
        return { merged: true, sha: after.mergeCommit?.oid ?? '' };
      }
      return { merged: false, reason: `PR #${number} is still ${after.state}` };
    },
    async markBlocked(number, reason, { headSha } = {}) {
      // --force is idempotent when the fixed label already exists.
      run(['gh', 'label', 'create', BLOCKED_LABEL_PREFIX, '--repo', repo,
        '--description', 'Blocked by merge queue; see PR comment for details', '--force']);
      run(['gh', 'pr', 'edit', String(number), '--repo', repo, '--add-label',
        BLOCKED_LABEL_PREFIX]);
      // M101-R1: record WHICH head was blocked so a later push is re-evaluated
      // instead of being refused forever. The marker is hidden when rendered.
      const marker = headSha ? `${blockedMarker(headSha)}\n` : '';
      run(['gh', 'pr', 'comment', String(number), '--repo', repo, '--body',
        `${marker}Merge queue dropped this PR: ${reason}`]);
    },
    async unmarkBlocked(number) {
      // M101-R2: the recorded head no longer exists; lift the block and let the
      // normal eligibility path decide again. If the removal fails, only treat
      // it as an error when the label is still there — a concurrent removal is
      // the outcome we wanted anyway.
      try {
        run(['gh', 'pr', 'edit', String(number), '--repo', repo, '--remove-label',
          BLOCKED_LABEL_PREFIX]);
      } catch (error) {
        const raw = run(['gh', 'pr', 'view', String(number), '--repo', repo, '--json', 'labels']);
        const labels = (JSON.parse(raw).labels ?? []).map((label) => label.name);
        if (labels.some((name) => typeof name === 'string' && name.startsWith(BLOCKED_LABEL_PREFIX))) {
          throw error;
        }
      }
    },
    async rulesetGuard() {
      const summaries = JSON.parse(run(['gh', 'api', `repos/${repo}/rulesets`]));
      const match = (summaries ?? []).find((item) => item.name === 'main-pr-and-ci-gate');
      if (!match) {
        throw new Error('merge queue refuses to run: ruleset main-pr-and-ci-gate is missing');
      }
      const full = JSON.parse(run(['gh', 'api', `repos/${repo}/rulesets/${match.id}`]));
      const verdict = verifyRequiredChecks(full);
      if (!verdict.ok) {
        throw new Error(
          `merge queue refuses to run: ruleset drifted (missing=${JSON.stringify(verdict.missing)} ` +
            `extra=${JSON.stringify(verdict.extra)} strict=${verdict.strict})`,
        );
      }
      return verdict;
    },
  };
}

function defaultRun(args) {
  const result = spawnSync(args[0], args.slice(1), { encoding: 'utf8' });
  if (result.status !== 0) {
    throw new Error(`${args.join(' ')} failed: ${result.stderr || result.stdout}`);
  }
  return result.stdout;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

async function main(argv) {
  const [command = 'status', ...rest] = argv;
  const flags = parseFlags(rest);
  const repo = flags.repo ?? 'BumStill/CodeFactory';

  if (command === 'plan' || command === 'status') {
    const adapter = createGhAdapter({ repo });
    const prs = await adapter.listOpenPrs();
    const plan = orderQueue(
      prs.filter((pr) => evaluateEligibility(pr).eligible),
    ).map((pr) => ({
      number: pr.number,
      title: pr.title,
      mergeStateStatus: pr.mergeStateStatus,
      action: pr.mergeStateStatus === 'BEHIND' ? 'update_then_merge' : 'merge',
    }));
    process.stdout.write(`${JSON.stringify({ repo, command, plan }, null, 2)}\n`);
    return 0;
  }

  if (command === 'run') {
    const adapter = createGhAdapter({ repo });
    await adapter.rulesetGuard();
    const outcomes = await runQueue(adapter, {
      stopRequested: flags.stopFile
        ? () => existsSync(flags.stopFile)
        : undefined,
      maxIterations: flags.limit ? Number(flags.limit) : 100,
    });
    process.stdout.write(`${JSON.stringify({ repo, outcomes }, null, 2)}\n`);
    return 0;
  }

  process.stderr.write(`unknown command: ${command}\n`);
  return 2;
}

function parseFlags(args) {
  const flags = {};
  for (let i = 0; i < args.length; i += 1) {
    if (args[i].startsWith('--')) {
      const key = args[i].slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
      const next = args[i + 1];
      if (next && !next.startsWith('--')) {
        flags[key] = next;
        i += 1;
      } else {
        flags[key] = true;
      }
    }
  }
  return flags;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (error) => {
      process.stderr.write(`${error?.stack ?? error}\n`);
      process.exit(1);
    },
  );
}
