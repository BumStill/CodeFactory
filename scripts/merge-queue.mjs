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
import { appendFileSync, existsSync } from 'node:fs';

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
  /<!--\s*merge-queue:blocked\s+head=([0-9a-f]{7,40})\s+at=([^\s]+)(?:\s+check=([^\s]+))?\s*-->/i;

/**
 * Render the machine-readable marker written at the top of a blocking comment.
 *
 * M127: the marker also records WHICH required check failed, because that is
 * the only thing that can distinguish "this head failed for a reason a later
 * run can fix" (a re-run, or a PR-body contract that needs no commit) from
 * "this head is genuinely broken".
 */
export function blockedMarker(headSha, date = new Date(), checkName = '') {
  const name = typeof checkName === 'string' ? checkName.trim() : '';
  const check = name ? ` check=${encodeURIComponent(name)}` : '';
  return `<!-- merge-queue:blocked head=${headSha} at=${date.toISOString()}${check} -->`;
}

// ---------------------------------------------------------------------------
// M126 — what wakes the queue
// ---------------------------------------------------------------------------
// 2026-10-20 (#768): the queue was driven by `schedule`, `workflow_run` and a
// `pull_request_target: [labeled]` trigger filtered to `merge-queue: armed`.
// Two real gaps let a green, armed PR (merge state CLEAN) sit for 20+ minutes:
//
//   (a) the queue's own `merge-queue: blocked` label was lifted after a green
//       head, which emits `unlabeled` — no trigger listened to it, so the
//       block removal woke nothing and the queue only ran when a human
//       dispatched it by hand;
//   (b) arming an already-armed PR produces no event at all (`gh pr edit
//       --add-label` is a no-op on a present label), so "arm once CI went
//       green" could stay asleep until the next cron tick.
//
// The decision lives HERE, not in the workflow's `if:`, so a unit test pins it
// and the workflow cannot drift away from the tested behaviour.
export const WAKE_LABELS = Object.freeze([ARM_LABEL, BLOCKED_LABEL_PREFIX]);

/**
 * Should this event drain the queue?
 *
 * Only `pull_request_target` events are filtered: they are the ones that fire
 * per label change. Everything else (schedule / workflow_run / dispatch) is a
 * deliberate queue sweep and always runs. The trigger stays
 * `pull_request_target` — the workflow checks out the DEFAULT branch and never
 * the PR head, so a candidate PR still cannot run its own queue logic.
 */
export function resolveWake(eventName, labelName = '') {
  const event = typeof eventName === 'string' ? eventName.trim() : '';
  if (event !== 'pull_request_target') {
    return { run: true, reason: `${event || 'unspecified event'} always drains the queue` };
  }
  const label = typeof labelName === 'string' ? labelName.trim() : '';
  if (!label) return { run: false, reason: 'pull_request_target without a label name' };
  const hits = WAKE_LABELS.some(
    (name) => label === name || (name === BLOCKED_LABEL_PREFIX && label.startsWith(BLOCKED_LABEL_PREFIX)),
  );
  if (!hits) return { run: false, reason: `label "${label}" is not a queue wake label` };
  return { run: true, reason: `queue label "${label}" changed` };
}

/** Parse the marker back out of a PR comment body; null when absent. */
export function parseBlockedMarker(body) {
  if (typeof body !== 'string') return null;
  const match = BLOCKED_MARKER_RE.exec(body);
  if (!match) return null;
  const at = Date.parse(match[2]);
  let check = null;
  if (match[3]) {
    try {
      check = decodeURIComponent(match[3]);
    } catch {
      check = match[3];
    }
  }
  return { headSha: match[1].toLowerCase(), at: Number.isNaN(at) ? null : at, check };
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
// ---------------------------------------------------------------------------
// M127 — a re-run on the SAME head supersedes its earlier conclusion
// ---------------------------------------------------------------------------
// 2026-10-20 (#769): `check-frontend` failed because the PR body lacked the
// README-Update contract lines. Fixing the body needs no commit, so the head
// stayed identical while a later run of the same job went green — and the queue
// kept reading the oldest run for that name, saw `failure`, and refused the PR
// forever. GitHub keeps every run for a head, so "which conclusion counts" has
// to be answered explicitly: the newest one.
export function latestCheckOnHead(checks, name, headSha) {
  const onHead = (checks ?? []).filter(
    (check) => check && check.name === name && check.headSha === headSha,
  );
  if (onHead.length === 0) return null;
  const stamp = (check) => {
    const value = toMillis(check?.completedAt) ?? toMillis(check?.startedAt);
    return value === null ? -Infinity : value;
  };
  return onHead.reduce((latest, check) => (stamp(check) >= stamp(latest) ? check : latest));
}

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
      // M127: the block names the check that dropped the PR. If that check is
      // green again on the SAME head, the block is stale — the recovery never
      // needed a commit. A legacy record without a check name keeps the
      // conservative M101 behaviour rather than retrying an unknown failure.
      const recordedCheck =
        typeof record?.check === 'string' && record.check.trim() ? record.check.trim() : null;
      if (recordedCheck) {
        const now = latestCheckOnHead(pr.checks, recordedCheck, pr.headSha);
        if (now && now.status === 'completed' && now.conclusion === 'success') {
          return evaluateEligibility(
            { ...pr, labels: labels.filter((label) => !label.startsWith(BLOCKED_LABEL_PREFIX)) },
            { requiredChecks },
          ).eligible
            ? {
                eligible: true,
                clearedBlock: true,
                reason: `blocked check ${recordedCheck} is green again on the same head (${shortSha(pr.headSha)}); all required checks re-evaluated`,
                terminal: false,
              }
            : { eligible: false, terminal: false, reason: 'recovered check does not certify all required checks; keeping block' };
        }
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
    // M127: the NEWEST run for this head decides, not whichever run the API
    // happened to list first.
    const current = latestCheckOnHead(checks, name, pr.headSha);
    if (current.status !== 'completed') {
      return { eligible: false, reason: `required check ${name} is still running`, terminal: false };
    }
    if (current.conclusion !== 'success') {
      // A failed check on the exact head is a definite failure: drop it (R4).
      return {
        eligible: false,
        failedCheck: name,
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

/** M122: PR intent wins conflicts, except a hold from any source is sticky. */
export function squashMessage(title, prBody, messages) {
  const readTrailers = (text) => text.split(/\r?\n/).map((line) => line.trim())
    .filter((line) => /^(Release-Urgency|Co-Authored-By):\s*\S/i.test(line));
  const commitTrailers = messages.flatMap(readTrailers);
  const bodyTrailers = readTrailers(prBody);
  const all = [...bodyTrailers, ...commitTrailers];
  const urgency = all.filter((line) => /^Release-Urgency:/i.test(line));
  const hold = urgency.find((line) => /^Release-Urgency:\s*hold$/i.test(line));
  const chosen = hold ? [hold] : (bodyTrailers.some((line) => /^Release-Urgency:/i.test(line))
    ? bodyTrailers : commitTrailers).filter((line) => /^Release-Urgency:/i.test(line));
  const trailers = [...new Set([...chosen, ...all.filter((line) => /^Co-Authored-By:/i.test(line))])];
  const subjects = new Set([title.trim(), ...messages.map((message) => message.split(/\r?\n/)[0].trim())]);
  const seenSubjects = new Set([title.trim()]);
  const prose = messages.map((message) => message.split(/\r?\n/).filter((line) => {
    const text = line.trim();
    if (/^(Release-Urgency|Co-Authored-By):/i.test(text) || /^Merge branch ['"]main['"] into /i.test(text)) return false;
    if (subjects.has(text)) {
      if (seenSubjects.has(text)) return false;
      seenSubjects.add(text);
    }
    return true;
  }).join('\n').trim()).filter(Boolean).join('\n\n');
  return { body: [prose, trailers.join('\n')].filter(Boolean).join('\n\n'), trailers };
}

/** M120: one deliberate dispatch per drain, pinned to its final merged head. */
export async function releaseMergedBatch(adapter, merged) {
  if (!merged.length) return;
  const immediate = [];
  try {
    for (const entry of merged) {
      const message = (await adapter.commitMessage(entry.sha)).trimEnd();
      const footer = message.split(/\r?\n\s*\r?\n/).at(-1);
      if (footer.split(/\r?\n/).some((line) => {
        const colon = line.indexOf(':');
        return colon >= 0 && line.slice(0, colon).trim().toLowerCase() === 'release-urgency'
          && line.slice(colon + 1).trim().toLowerCase() === 'immediate';
      })) immediate.push(entry);
    }
    if (!immediate.length) return;
    const head = merged.at(-1).sha;
    const plan = await adapter.releasePlan(head);
    if (plan.hold > 0 || plan.invalid_urgency > 0) {
      for (const entry of immediate) await adapter.comment(entry.number,
        `Merge queue did not dispatch Auto Release: guarded batch at ${head} ` +
        `contains ${plan.hold} hold and ${plan.invalid_urgency} invalid urgency trailer(s). ` +
        'Review the complete batch before explicitly using allow_guarded_batch=true.');
      return;
    }
    await adapter.dispatchRelease(head);
  } catch (error) {
    // Also report scan failures: missing evidence must never silently release.
    for (const entry of immediate.length ? immediate : merged) {
      await adapter.comment(entry.number,
        `Merge queue Auto Release dispatch/check failed: ${String(error.message ?? error)}`);
    }
    throw error; // CLI exits nonzero; never let the per-PR merge catch swallow it.
  }
}

/**
 * Run the queue against an adapter, one merge at a time.
 *
 * The loop re-lists open PRs after every authoritative state change (merge,
 * drop), so the next PR is always evaluated against the freshly moved main.
 * `stopRequested` is consulted before each merge; when it returns true the
 * queue stops and emits a terminal `stopped` outcome (R6).
 */
export async function runQueue(adapter, { stopRequested, maxIterations = 100, initialPrs = null } = {}) {
  const outcomes = [];
  const mergedEntries = [];
  const seen = new Set();
  let iterations = 0;
  // R8(b): the caller may hand over the snapshot it already fetched (the
  // `drain` command plans and merges from ONE listing instead of two).
  let pendingPrs = Array.isArray(initialPrs) ? initialPrs : null;

  while (iterations++ < maxIterations) {
    if (typeof stopRequested === 'function' && stopRequested()) {
      outcomes.push({ status: 'stopped' });
      break;
    }

    const prs = pendingPrs ?? (await adapter.listOpenPrs());
    pendingPrs = null;
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
          await adapter.markBlocked(pr.number, verdict.reason, {
            headSha: pr.headSha,
            checkName: verdict.failedCheck,
          });
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
      title: target.title,
      prBody: target.body,
    });
    if (result.merged) {
      outcomes.push({ number: target.number, status: 'merged', sha: result.sha });
      mergedEntries.push({ number: target.number, sha: result.sha });
      seen.add(target.number);
      continue;
    }
    // Not merged right now (e.g. checks still running): wait, do not spin.
    outcomes.push({ number: target.number, status: 'waiting', reason: result.reason });
    seen.add(target.number);
    } catch (error) {
      if (error.mergedTrailerFailure) throw error;
      seen.add(head.number);
      outcomes.push({ number: head.number, status: 'error', reason: String(error.message ?? error) });
    }
  }
  // M120: merges that landed in THIS drain may authorize one release. This is
  // outside the per-PR catch above, so a dispatch failure fails the queue run.
  await releaseMergedBatch(adapter, mergedEntries);
  return outcomes;
}

/**
 * R8: plan and drain from ONE snapshot.
 *
 * Before this, `.github/workflows/merge-queue.yml` ran `plan` and then `run` as
 * two processes; each re-listed every open PR and re-read the ruleset, and every
 * listing pulled check-runs for PRs that were not even armed. This is the single
 * entry point both the workflow and the CLI now use.
 *
 * Order is deliberate and fail-closed:
 *   (a) one cheap "is anything armed?" listing — exit immediately otherwise;
 *   (d) exactly one ruleset read per run (a drifted ruleset must never run);
 *   (b) one PR listing, shared by the plan and the merge loop;
 *   (c) the adapter only pulls check-runs/comments/events for armed PRs.
 */
export async function drainQueue(adapter, { stopRequested, maxIterations = 100, dryRun = false } = {}) {
  if (typeof adapter.anyArmed === 'function' && !(await adapter.anyArmed())) {
    return { skipped: 'no_armed_pr', plan: [], outcomes: [] };
  }
  const guard = await adapter.rulesetGuard();
  const prs = await adapter.listOpenPrs();
  const plan = orderQueue(prs.filter((pr) => evaluateEligibility(pr).eligible)).map((pr) => ({
    number: pr.number,
    title: pr.title,
    mergeStateStatus: pr.mergeStateStatus,
    action: pr.mergeStateStatus === 'BEHIND' ? 'update_then_merge' : 'merge',
  }));
  if (dryRun) return { guard, plan, outcomes: [] };
  const outcomes = await runQueue(adapter, { stopRequested, maxIterations, initialPrs: prs });
  return { guard, plan, outcomes };
}

// ---------------------------------------------------------------------------
// Networked GitHub adapter (not exercised by unit tests)
// ---------------------------------------------------------------------------

export function createGhAdapter({ repo, run = defaultRun } = {}) {
  async function prView(number) {
    const raw = run([
      'gh', 'pr', 'view', String(number), '--repo', repo, '--json',
      'number,title,body,headRefName,headRefOid,isDraft,labels,mergeStateStatus',
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
      startedAt: check.started_at,
      completedAt: check.completed_at,
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
      body: pr.body,
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
    /// R8(a): the cheap "is anything armed?" probe. One listing call, no
    /// check-runs, no rulesets — used to exit a workflow_run sweep immediately
    /// when the queue has nothing to do.
    async anyArmed() {
      const raw = run([
        'gh', 'pr', 'list', '--repo', repo, '--state', 'open', '--limit', '1',
        '--label', ARM_LABEL, '--json', 'number',
      ]);
      const list = JSON.parse(raw);
      return Array.isArray(list) && list.length > 0;
    },
    async listOpenPrs() {
      const raw = run([
        'gh', 'pr', 'list', '--repo', repo, '--state', 'open', '--limit', '100', '--json',
        'number,title,body,headRefName,headRefOid,isDraft,labels,mergeStateStatus',
      ]);
      const list = JSON.parse(raw);
      const out = [];
      for (const pr of list) {
        const labels = (pr.labels ?? []).map((label) => label.name);
        // R8(c): an unarmed PR can never be eligible (evaluateEligibility
        // rejects it on the label alone), so its check-runs and blocked-head
        // evidence are not worth an API call.
        if (!labels.includes(ARM_LABEL)) {
          out.push(mapPr(pr, []));
          continue;
        }
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
    // M120(a): the merged commit on main is the release decision source.
    async commitMessage(sha) {
      const body = JSON.parse(run(['gh', 'api', `repos/${repo}/commits/${sha}`]));
      return body?.commit?.message ?? '';
    },
    // M120(b): reuse the release planner so the queue and Auto Release can
    // never disagree about hold/invalid guards.
    async releasePlan(sha) {
      // Checkout predates this drain's merges. Fetch their objects and tags
      // before scanning, and resolve the previous tag relative to the final head.
      run(['git', 'fetch', '--tags', 'origin', 'main']);
      const lastTag = run(['git', 'describe', '--tags', '--abbrev=0', sha]).trim();
      const raw = run(['python3', 'tools/release/plan_release.py', '--range', `${lastTag}..${sha}`]);
      return JSON.parse(raw);
    },
    async dispatchRelease(sha) {
      run(['gh', 'workflow', 'run', 'auto-release.yml', '--repo', repo, '--ref', 'main',
        '-f', `expected_head_sha=${sha}`]);
    },
    async comment(number, body) {
      run(['gh', 'pr', 'comment', String(number), '--repo', repo, '--body', body]);
    },
    async updateBranch(number, expectedHead) {
      run([
        'gh', 'api', '-X', 'PUT',
        `repos/${repo}/pulls/${number}/update-branch`,
        '-f', `expected_head_sha=${expectedHead}`,
      ]);
    },
    async merge(number, { expectedHead, title, prBody } = {}) {
      const commits = JSON.parse(run([
        'gh', 'api', `repos/${repo}/pulls/${number}/commits`, '--paginate', '--slurp',
      ])).flat();
      // Reuse the listing snapshot, including PR intent, without another read.
      const metadata = title && prBody !== undefined
        ? { title, body: prBody }
        : JSON.parse(run([
          'gh', 'pr', 'view', String(number), '--repo', repo, '--json', 'title,body',
        ]));
      const subject = title ?? metadata.title ?? 'fix: synthetic change';
      const { body, trailers } = squashMessage(subject, prBody ?? metadata.body ?? '',
        commits.map((commit) => commit.commit.message));
      run([
        'gh', 'pr', 'merge', String(number), '--repo', repo, '--squash',
        '--match-head-commit', expectedHead,
        '--subject', subject, '--body', body,
      ]);
      const after = JSON.parse(
        run(['gh', 'pr', 'view', String(number), '--repo', repo, '--json', 'state,mergeCommit']),
      );
      if (after.state === 'MERGED') {
        if (trailers.length) {
          try {
            const commit = JSON.parse(run(['gh', 'api', `repos/${repo}/commits/${after.mergeCommit?.oid}`]));
            const footer = (commit?.commit?.message ?? '').trimEnd().split(/\r?\n\s*\r?\n/).at(-1);
            const lines = footer.split(/\r?\n/).map((line) => line.trim());
            const missing = trailers.filter((line) => !lines.includes(line));
            if (missing.length) throw new Error(`missing final trailer(s): ${missing.join(', ')}`);
          } catch (cause) {
            const error = new Error(`Merged trailer verification failed: ${cause.message ?? cause}`);
            error.mergedTrailerFailure = true;
            try {
              run(['gh', 'pr', 'comment', String(number), '--repo', repo, '--body', error.message]);
            } finally {
              throw error;
            }
          }
        }
        return { merged: true, sha: after.mergeCommit?.oid ?? '' };
      }
      return { merged: false, reason: `PR #${number} is still ${after.state}` };
    },
    async markBlocked(number, reason, { headSha, checkName } = {}) {
      // --force is idempotent when the fixed label already exists.
      run(['gh', 'label', 'create', BLOCKED_LABEL_PREFIX, '--repo', repo,
        '--description', 'Blocked by merge queue; see PR comment for details', '--force']);
      run(['gh', 'pr', 'edit', String(number), '--repo', repo, '--add-label',
        BLOCKED_LABEL_PREFIX]);
      // M101-R1: record WHICH head was blocked so a later push is re-evaluated
      // instead of being refused forever. The marker is hidden when rendered.
      const marker = headSha ? `${blockedMarker(headSha, new Date(), checkName)}\n` : '';
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

  if (command === 'wake') {
    // M126: the workflow calls this instead of encoding the label filter in its
    // `if:`, so the tested predicate is the one that actually runs.
    const verdict = resolveWake(
      flags.event ?? process.env.GITHUB_EVENT_NAME ?? '',
      flags.label ?? '',
    );
    const outFile = process.env.GITHUB_OUTPUT;
    if (outFile) {
      appendFileSync(outFile, `run=${verdict.run ? 'true' : 'false'}\nreason=${verdict.reason}\n`);
    }
    process.stdout.write(`${JSON.stringify({ command, ...verdict }, null, 2)}\n`);
    return 0;
  }

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

  if (command === 'drain') {
    // R8: the single entry point — one armed-probe, one ruleset read, one PR
    // listing shared by the plan and the merge loop.
    const adapter = createGhAdapter({ repo });
    const result = await drainQueue(adapter, {
      stopRequested: flags.stopFile
        ? () => existsSync(flags.stopFile)
        : undefined,
      maxIterations: flags.limit ? Number(flags.limit) : 100,
      dryRun: flags.dryRun === true,
    });
    process.stdout.write(`${JSON.stringify({ repo, ...result }, null, 2)}\n`);
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
