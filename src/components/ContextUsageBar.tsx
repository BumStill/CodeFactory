// SPDX-License-Identifier: Apache-2.0
//
// Combined live readout for the chat footer: token totals on the left,
// context-window usage on the right, all in one slim row. Previously these
// were two separate rows with emoji-prefixed labels; the user asked for a
// tighter, professional look — single row, no colored icons.

import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "../lib/tauri";
import { useChatStore } from "../stores/chat";

interface UsageSummary {
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cached_tokens: number;
  requests: number;
  actual_cost_usd: number | null;
  estimated_cost_usd: number | null;
  cost_source: string;
}

interface DailyUsage {
  summary: UsageSummary;
}

export function formatContextTokens(n: number): string {
  if (n >= 1_000_000) {
    const millions = n / 1_000_000;
    const decimals = Number.isInteger(millions * 10) ? 1 : 2;
    return `${millions.toFixed(decimals)}M`;
  }
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

interface Props {
  sessionId?: string | null;
}

export function ContextUsageBar({ sessionId }: Props) {
  const usage = useChatStore((s) => (sessionId ? s.runtime?.[sessionId]?.contextUsage ?? null : null));
  const toast = useChatStore((s) => (sessionId ? s.runtime?.[sessionId]?.compressionToast ?? null : null));

  const [session, setSession] = useState<UsageSummary | null>(null);
  const [today, setToday] = useState<UsageSummary | null>(null);
  const [showToast, setShowToast] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const dashboard = await invoke<DailyUsage>("get_usage_dashboard", {
        rangeDays: 1,
        timezoneOffsetMinutes: -new Date().getTimezoneOffset(),
      });
      setToday(dashboard.summary);
    } catch {
      // DB may not be ready
    }
    if (sessionId) {
      try {
        setSession(await invoke<UsageSummary>("get_session_usage", { sessionId }));
      } catch {
        setSession(null);
      }
    } else {
      setSession(null);
    }
  }, [sessionId]);

  useEffect(() => { refresh(); }, [refresh]);

  useEffect(() => {
    let cancel = false;
    let unlisten: (() => void) | null = null;
    listen<string>("model-usage-recorded", () => {
      if (!cancel) refresh();
    }).then((fn) => {
      if (cancel) fn();
      else unlisten = fn;
    });
    return () => { cancel = true; unlisten?.(); };
  }, [refresh]);

  useEffect(() => {
    if (!toast) return;
    setShowToast(true);
    const t = setTimeout(() => setShowToast(false), 4500);
    return () => clearTimeout(t);
  }, [toast?.id]);

  const sessionTok = session ? session.input_tokens + session.output_tokens : 0;
  const todayTok = today ? today.input_tokens + today.output_tokens : 0;
  const hasUsage = usage && usage.limit > 0;
  const hasTokens = sessionTok > 0 || todayTok > 0;

  // Hide entirely if there's nothing to show.
  if (!hasUsage && !hasTokens && !showToast) return null;

  // Context bar color tones — kept subtle (no fill icon, just text colour
  // shifts) so the row stays minimal.
  let pct = 0;
  let toneText = "text-gray-500";
  let toneBar = "bg-emerald-500";
  if (hasUsage) {
    pct = Math.min(100, (usage!.used / usage!.limit) * 100);
    if (pct < 60) { toneText = "text-emerald-400"; toneBar = "bg-emerald-500"; }
    else if (pct < 85) { toneText = "text-amber-400";  toneBar = "bg-amber-500"; }
    else               { toneText = "text-rose-400";   toneBar = "bg-rose-500"; }
  }

  return (
    <div className="flex items-center gap-4 px-4 py-1 border-t border-border text-[11px] bg-surface-1 shrink-0 select-none">
      {/* Token totals — left side, no emoji */}
      {hasTokens && (
        <div className="flex items-center gap-3 text-gray-500 tabular-nums">
          {sessionTok > 0 && (
            <span title="当前会话">
              <span className="text-gray-600 mr-1">会话</span>
              {formatContextTokens(sessionTok)}
            </span>
          )}
          {todayTok > 0 && (
            <span title="今日合计">
              <span className="text-gray-600 mr-1">今日</span>
              {formatContextTokens(todayTok)}
            </span>
          )}
          {today?.cost_source === "subscription" && <span title="ChatGPT 订阅流量">订阅</span>}
          {today?.cost_source === "local" && <span title="本地模型流量">本地</span>}
          {today?.cost_source === "provider_actual" && (today.actual_cost_usd ?? 0) > 0 && (
            <span title="今日 Provider 实际费用">实际 ${(today.actual_cost_usd ?? 0).toFixed(4)}</span>
          )}
          {(today?.cost_source === "model_price_estimate" || today?.cost_source === "legacy_estimate")
            && (today.estimated_cost_usd ?? 0) > 0 && (
              <span title="今日估算费用">估算 ${(today.estimated_cost_usd ?? 0).toFixed(4)}</span>
            )}
        </div>
      )}

      {/* Compression toast — middle, transient */}
      {showToast && toast && (
        <span
          className="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-800 dark:text-amber-300 text-[10px]"
          title={`释放约 ${formatContextTokens(toast.tokensFreed)} tokens`}
        >
          已压缩 {toast.elidedCount} 条旧结果
        </span>
      )}

      {/* Context window usage — right-aligned */}
      {hasUsage && (
        <div
          className="flex items-center gap-2 ml-auto tabular-nums"
          title={
            usage!.maxLimit > usage!.limit
              ? `当前预算 ${formatContextTokens(usage!.limit)}，内容增长后自动扩展至 ${formatContextTokens(usage!.maxLimit)}`
              : `当前上下文预算 ${formatContextTokens(usage!.limit)}`
          }
        >
          <span className="text-gray-600">上下文</span>
          <span className={toneText}>
            {formatContextTokens(usage!.used)} / {formatContextTokens(usage!.limit)}
          </span>
          <div className="h-1 w-24 rounded-full bg-surface-3 overflow-hidden">
            <div
              className={`h-full ${toneBar} transition-all duration-300`}
              style={{ width: `${pct}%` }}
            />
          </div>
          <span className={`${toneText} w-10 text-right`}>{pct.toFixed(0)}%</span>
        </div>
      )}
    </div>
  );
}
