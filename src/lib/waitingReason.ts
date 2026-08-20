// SPDX-License-Identifier: Apache-2.0

/// `waiting_reason` carries human text almost everywhere it is written
/// ("验证证据不足", "交付任务已连续运行约 58 分钟"), so the UI prints it
/// verbatim. A few internal codes reach the same field, and printing those
/// verbatim puts framework vocabulary on screen. Anything shaped like an
/// ASCII identifier is treated as such a code: named ones get their sentence,
/// unnamed ones fall back to a generic one rather than leaking.
const WAITING_REASON_LABELS: Record<string, string> = {
  technical_recovery_exhausted: "系统多轮自动恢复没有进展，已登记为系统故障；你不需要补充输入",
  agent_loop_error: "执行过程中断，已停止并把当前结论交还给你",
  context_compaction_exhausted: "对话上下文已压缩到极限，已停止并把当前结论交还给你",
  run_budget_exhausted: "本轮预算已用完，已停止并把当前结论交还给你",
  authorization_required: "需要你先授权才能继续",
  needs_business_decision: "需要你先做一个决定才能继续",
  tool_observation_contract_missing: "该操作缺少可验证的观察方式，系统没有执行",
  browser_observation_contract_required: "浏览器操作缺少可验证的观察方式，系统没有执行",
};

/// Reasons that mean the turn stopped for good. A stopped turn must not keep
/// quoting a remaining time.
const TERMINAL_WAITING_REASONS = new Set([
  "technical_recovery_exhausted",
  "agent_loop_error",
  "context_compaction_exhausted",
  "run_budget_exhausted",
]);

const INTERNAL_CODE = /^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$/;

function isInternalCode(reason: string): boolean {
  return INTERNAL_CODE.test(reason);
}

/// The sentence to show for a waiting reason. Human text passes through
/// untouched; internal codes never reach the screen.
export function humanWaitingReason(
  reason: string | null | undefined,
): string | null {
  const trimmed = reason?.trim();
  if (!trimmed) return null;
  const label = WAITING_REASON_LABELS[trimmed];
  if (label) return label;
  if (isInternalCode(trimmed)) return "执行已停止，当前结论已交还给你";
  return trimmed;
}

/// True when the reason means the turn is over rather than still waiting on
/// something that will finish by itself.
export function isTerminalWaitingReason(
  reason: string | null | undefined,
): boolean {
  const trimmed = reason?.trim();
  if (!trimmed) return false;
  return TERMINAL_WAITING_REASONS.has(trimmed) || isInternalCode(trimmed);
}
