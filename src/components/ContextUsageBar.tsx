// SPDX-License-Identifier: Apache-2.0
//
// Combined live readout for the chat composer header: token totals on the
// left, context-window usage on the right, all in one slim row sitting
// directly above the message input. Previously these were two separate rows
// with emoji-prefixed labels; the user asked for a tighter, professional
// look — single row, no colored icons, placed above the input box so it
// stops stealing vertical space from the conversation area.

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
  onOpenUsage?: (scope: "session" | "today") => void;
}

export type ContextUsageTone = "progress" | "warning" | "danger";

interface ContextUsagePresentation {
  tone: ContextUsageTone;
  label: string;
  textClass: string;
  barClass: string;
}

export function contextUsagePresentation(percent: number): ContextUsagePresentation {
  if (percent >= 85) {
    return {
      tone: "danger",
      label: "上下文紧张",
      textClass: "text-status-danger",
      barClass: "bg-status-danger",
    };
  }
  if (percent >= 70) {
    return {
      tone: "warning",
      label: "上下文偏高",
      textClass: "text-status-warning",
      barClass: "bg-status-warning",
    };
  }
  return {
    tone: "progress",
    label: "上下文充足",
    textClass: "text-status-progress",
    barClass: "bg-status-progress",
  };
}

export function ContextUsageBar({ sessionId, onOpenUsage }: Props) {
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

  let pct = 0;
  if (hasUsage) {
    pct = Math.min(100, (usage!.used / usage!.limit) * 100);
  }
  const presentation = contextUsagePresentation(pct);

  const usageValue = (
    scope: "session" | "today",
    label: string,
    value: number,
    title: string,
  ) => {
    const content = (
      <>
        <span className="mr-1 text-gray-600">{label}</span>
        {formatContextTokens(value)}
      </>
    );
    if (!onOpenUsage) return <span title={title}>{content}</span>;
    return (
      <button
        type="button"
        className="-my-1 inline-flex min-h-7 items-center rounded-md px-1.5 transition-colors hover:bg-surface-3 hover:text-gray-300"
        title={`${title}，打开用量详情`}
        onClick={() => onOpenUsage(scope)}
      >
        {content}
      </button>
    );
  };

  return (
    <div
      data-testid="context-usage-bar"
      className="flex min-h-8 shrink-0 select-none items-center gap-3 border-b border-border/60 px-4 py-1 text-xs text-gray-500"
    >
      {/* Token totals — left side, no emoji */}
      {hasTokens && (
        <div className="flex items-center gap-1 tabular-nums">
          {sessionTok > 0 && usageValue("session", "会话", sessionTok, "当前会话")}
          {todayTok > 0 && usageValue("today", "今日", todayTok, "今日合计")}
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

      {/* Compression toast — transient, follows the tokens */}
      {showToast && toast && (
        <span
          className="rounded-md bg-status-warning-soft px-1.5 py-0.5 text-[11px] text-status-warning"
          title={`释放约 ${formatContextTokens(toast.tokensFreed)} tokens`}
        >
          已压缩 {toast.elidedCount} 条旧结果
        </span>
      )}

      {/* Context window usage — right-aligned */}
      {hasUsage && (
        <div
          className="ml-auto flex items-center gap-2 tabular-nums"
          data-context-tone={presentation.tone}
          title={
            usage!.maxLimit > usage!.limit
              ? `当前预算 ${formatContextTokens(usage!.limit)}，内容增长后自动扩展至 ${formatContextTokens(usage!.maxLimit)}`
              : `当前上下文预算 ${formatContextTokens(usage!.limit)}`
          }
        >
          <span className="text-gray-600">{presentation.label}</span>
          <span className={presentation.textClass}>
            {formatContextTokens(usage!.used)} / {formatContextTokens(usage!.limit)}
          </span>
          <div
            className="h-1.5 w-24 overflow-hidden rounded-full bg-surface-4"
            role="meter"
            aria-label="上下文占用"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(pct)}
            aria-valuetext={`${presentation.label}，已使用 ${pct.toFixed(0)}%`}
          >
            <div
              className={`h-full rounded-full ${presentation.barClass} transition-all duration-300 motion-reduce:transition-none`}
              style={{ width: `${pct}%` }}
            />
          </div>
          <span className={`${presentation.textClass} w-9 text-right`}>{pct.toFixed(0)}%</span>
        </div>
      )}
    </div>
  );
}
