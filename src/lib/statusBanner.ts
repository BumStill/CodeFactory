// SPDX-License-Identifier: Apache-2.0

import { formatDuration } from "./duration";
import { systemOwnsObjective } from "./turnOwnership";
import { humanWaitingReason, isTerminalWaitingReason } from "./waitingReason";

/**
 * The session's top status banner tells the user three things and nothing
 * more: what the system is doing right now, whether they need to act, and — if
 * it is waiting — roughly how long until it continues. Internal control-loop
 * state (owners such as `objective-supervisor:chat`, phases, failure codes)
 * never reaches this view.
 *
 * CF-RSB-R1 / R2 / R3 / R4
 */

export type StatusBannerTone = "progress" | "warning";

export interface StatusBannerView {
  tone: StatusBannerTone;
  /** What the system is doing, or what the user must do. Plain language only. */
  text: string;
  /** Extra plain context (e.g. the human waiting reason), or null. */
  detail: string | null;
  /** Truthful "how long until it continues" hint, or null when unknown. */
  waitHint: string | null;
  /** Human elapsed time (seconds / minutes), never a raw millisecond value. */
  elapsed: string;
}

export interface StatusBannerActivity {
  label?: string | null;
  waitingReason?: string | null;
  objectiveStatus?: string | null;
  recoveryOwner?: string | null;
  nextObservationAt?: number | null;
  terminalReason?: string | null;
}

export interface StatusBannerInput {
  activity?: StatusBannerActivity | null;
  startedAt: number;
  nowMs: number;
  /**
   * CF-HDE-R11（M53）：这个会话此刻是不是**真的**在执行，来自
   * `lib/sessionTurnState.ts` 那一个真实来源。为 true 时，一条陈旧的活动投影
   * （还写着"等待重试"、或者已经带 terminalReason）不能把横幅变成等待／消失——
   * 真机上模型正在连续调用，顶部却一直显示"等待自动重试"。
   */
  systemRunning?: boolean;
}

/**
 * Internal control-loop vocabulary that must never reach the banner: ASCII
 * identifiers (`objective-supervisor:chat`, `objective_failed`,
 * `technical_recovery_exhausted`), the framework's English terms, and the
 * Chinese terms the loop uses for itself ("恢复/退避/观察/监督/补救/目标").
 */
const INTERNAL_ASCII_TOKENS =
  /[a-z][a-z0-9]*(?:[._:-][a-z0-9]+)+|\b(?:objective|supervisor|remediation|generation|recovery|route|backoff)\b/i;

const INTERNAL_ZH_TERMS = [
  "恢复",
  "补救",
  "监督",
  "目标",
  "退避",
  "观察",
  "内部",
] as const;

/** True when `text` carries internal control-loop vocabulary. */
export function containsInternalVocabulary(text: string | null | undefined): boolean {
  if (!text) return false;
  if (INTERNAL_ASCII_TOKENS.test(text)) return true;
  return INTERNAL_ZH_TERMS.some((term) => text.includes(term));
}

/** Anything closer than this reads as "right now", not as a countdown. */
const IMMINENT_MS = 1_000;

/**
 * Roughly how long until the system tries again, in plain words.
 *
 * Imminent or overdue → "马上重试" (never "0ms" / a negative time).
 * Unknown → null (show no time rather than invent one).
 * Durations use human units: seconds / minutes / hours.
 */
export function formatRetryHint(
  nextObservationAt: number | null | undefined,
  nowMs: number,
): string | null {
  if (nextObservationAt == null || !Number.isFinite(nextObservationAt)) return null;
  const remaining = nextObservationAt - nowMs;
  if (remaining <= IMMINENT_MS) return "马上重试";

  const seconds = Math.round(remaining / 1_000);
  if (seconds < 60) return `约 ${seconds} 秒后重试`;
  const minutes = Math.round(remaining / 60_000);
  if (minutes < 60) return `约 ${minutes} 分钟后重试`;
  const hours = Math.round(minutes / 60);
  return `约 ${hours} 小时后重试`;
}

/** Elapsed time in human units — a sub-second turn reads "不到 1 秒". */
export function formatHumanElapsed(ms: number): string {
  const safe = Number.isFinite(ms) ? Math.max(0, ms) : 0;
  if (safe < 1_000) return "不到 1 秒";
  return formatDuration(safe);
}

/** Reasons that mean the next move is the user's, not the system's. */
const USER_ACTION_REASONS = new Set([
  "authorization_required",
  "needs_business_decision",
]);

const USER_ACTION_PHRASE = /需要你|请你|需要先/;

function isUserActionReason(reason: string | null | undefined): boolean {
  const trimmed = reason?.trim();
  if (!trimmed) return false;
  return USER_ACTION_REASONS.has(trimmed) || USER_ACTION_PHRASE.test(trimmed);
}

function plainOrNull(text: string | null | undefined): string | null {
  const trimmed = text?.trim();
  if (!trimmed) return null;
  return containsInternalVocabulary(trimmed) ? null : trimmed;
}

/**
 * Build the single banner view for a turn that has no structured plan.
 * Returns null when there is nothing truthful to say — i.e. the task has
 * ended (completed / failed / cancelled / settled), so no banner and no stop
 * button are shown.
 */
export function statusBannerView(input: StatusBannerInput): StatusBannerView | null {
  const activity = input.activity ?? null;
  const running = input.systemRunning === true;

  // Terminal: the system has stopped and nothing is still running.
  if (activity?.terminalReason && !running) return null;
  const objectiveStatus = activity?.objectiveStatus ?? null;
  if (objectiveStatus && !systemOwnsObjective(objectiveStatus) && !running) return null;
  const reason = activity?.waitingReason ?? null;
  // A user-action reason (authorization, a decision) must survive the terminal
  // check: those codes are internal-shaped, and a generic code-shaped guard
  // would otherwise treat them as a dead turn.
  const userAction = isUserActionReason(reason);
  if (!userAction && isTerminalWaitingReason(reason)) return null;

  const elapsed = formatHumanElapsed(input.nowMs - input.startedAt);
  const systemOwned = systemOwnsObjective(objectiveStatus);
  const userReason = userAction ? humanWaitingReason(reason) : null;
  const label = plainOrNull(activity?.label);
  const detailPlain = userAction ? null : plainOrNull(humanWaitingReason(reason));

  const waiting =
    !running &&
    !userAction &&
    (objectiveStatus === "waiting_system" || (systemOwned && activity?.nextObservationAt != null));

  // What the system is doing right now.
  const text = userReason ?? (waiting ? "系统正在等待自动重试" : label ?? "正在执行");

  // Extra plain context: the label while waiting, otherwise the human reason.
  const detail = userAction
    ? null
    : waiting
      ? label ?? detailPlain
      : detailPlain && detailPlain !== text
        ? detailPlain
        : null;

  const waitHint =
    systemOwned && !userAction && !running
      ? formatRetryHint(activity?.nextObservationAt, input.nowMs)
      : null;

  const tone: StatusBannerTone =
    userAction || humanWaitingReason(reason) != null ? "warning" : "progress";

  return {
    tone,
    text,
    detail: detail && detail !== text ? detail : null,
    waitHint,
    elapsed,
  };
}
