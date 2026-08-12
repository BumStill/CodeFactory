// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
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
  ringClass: string;
}

export function contextUsagePresentation(percent: number): ContextUsagePresentation {
  if (percent >= 90) {
    return {
      tone: "danger",
      label: "接近上限",
      textClass: "text-status-danger",
      ringClass: "stroke-status-danger",
    };
  }
  if (percent >= 75) {
    return {
      tone: "warning",
      label: "上下文偏高",
      textClass: "text-status-warning",
      ringClass: "stroke-status-warning",
    };
  }
  return {
    tone: "progress",
    label: "上下文",
    textClass: "text-status-progress",
    ringClass: "stroke-status-progress",
  };
}

function costDescription(summary: UsageSummary | null): string {
  if (!summary) return "暂无费用记录";
  if (summary.cost_source === "subscription") return "ChatGPT 订阅流量";
  if (summary.cost_source === "local") return "本地模型流量";
  if (summary.cost_source === "provider_actual") {
    return summary.actual_cost_usd == null
      ? "Provider 实际费用待同步"
      : `Provider 实际费用 $${summary.actual_cost_usd.toFixed(4)}`;
  }
  if (summary.cost_source === "model_price_estimate" || summary.cost_source === "legacy_estimate") {
    return summary.estimated_cost_usd == null
      ? "估算费用待同步"
      : `估算费用 $${summary.estimated_cost_usd.toFixed(4)}`;
  }
  return "暂无费用记录";
}

/**
 * Compact context-window control for the composer. The always-visible surface
 * is deliberately limited to the current context pressure; cumulative token
 * and cost telemetry remains available from the keyboard-openable detail.
 */
export function ContextUsageBar({ sessionId, onOpenUsage }: Props) {
  const usage = useChatStore((s) => (sessionId ? s.runtime?.[sessionId]?.contextUsage ?? null : null));
  const toast = useChatStore((s) => (sessionId ? s.runtime?.[sessionId]?.compressionToast ?? null : null));

  const [session, setSession] = useState<UsageSummary | null>(null);
  const [today, setToday] = useState<UsageSummary | null>(null);
  const [showToast, setShowToast] = useState(false);
  const [detailOpen, setDetailOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  const refreshGenerationRef = useRef(0);
  const detailId = `context-usage-detail-${useId().replace(/:/g, "")}`;
  const [detailPosition, setDetailPosition] = useState({
    left: 8,
    top: 8,
    width: 352,
    maxHeight: 320,
  });

  const refresh = useCallback(async () => {
    const generation = ++refreshGenerationRef.current;
    const requestedSessionId = sessionId;
    const dashboardRequest = invoke<DailyUsage>("get_usage_dashboard", {
        rangeDays: 1,
        timezoneOffsetMinutes: -new Date().getTimezoneOffset(),
      });
    const sessionRequest = requestedSessionId
      ? invoke<UsageSummary>("get_session_usage", { sessionId: requestedSessionId })
      : Promise.resolve<UsageSummary | null>(null);
    const [dashboardResult, sessionResult] = await Promise.allSettled([
      dashboardRequest,
      sessionRequest,
    ]);
    // A late response from a previous session or an older refresh must never
    // overwrite the newly selected session's cumulative usage.
    if (generation !== refreshGenerationRef.current) return;
    setToday(dashboardResult.status === "fulfilled" ? dashboardResult.value.summary : null);
    setSession(sessionResult.status === "fulfilled" ? sessionResult.value : null);
  }, [sessionId]);

  useEffect(() => {
    setSession(null);
    setDetailOpen(false);
    void refresh();
    return () => {
      refreshGenerationRef.current += 1;
    };
  }, [refresh, sessionId]);

  useEffect(() => {
    let cancel = false;
    let unlisten: (() => void) | null = null;
    listen<string>("model-usage-recorded", () => {
      if (!cancel) void refresh();
    }).then((fn) => {
      if (cancel) fn();
      else unlisten = fn;
    });
    return () => { cancel = true; unlisten?.(); };
  }, [refresh]);

  useEffect(() => {
    if (!toast) return;
    setShowToast(true);
    const timer = setTimeout(() => setShowToast(false), 4500);
    return () => clearTimeout(timer);
  }, [toast?.id]);

  const closeDetail = useCallback((deferFocus = false) => {
    setDetailOpen(false);
    if (deferFocus) {
      queueMicrotask(() => triggerRef.current?.focus());
    } else {
      triggerRef.current?.focus();
    }
  }, []);

  const updateDetailPosition = useCallback(() => {
    if (typeof window === "undefined") return;
    const triggerRect = triggerRef.current?.getBoundingClientRect();
    if (!triggerRect) return;
    const viewportPadding = 8;
    const width = Math.min(352, Math.max(0, window.innerWidth - viewportPadding * 2));
    const maxHeight = Math.max(0, triggerRect.top - viewportPadding * 2);
    const measuredHeight = detailRef.current?.getBoundingClientRect().height || 320;
    const visibleHeight = Math.min(measuredHeight, maxHeight);
    setDetailPosition({
      left: Math.max(
        viewportPadding,
        Math.min(triggerRect.right - width, window.innerWidth - width - viewportPadding),
      ),
      top: Math.max(viewportPadding, triggerRect.top - visibleHeight - viewportPadding),
      width,
      maxHeight,
    });
  }, []);

  useLayoutEffect(() => {
    if (!detailOpen) return;
    updateDetailPosition();
    window.addEventListener("resize", updateDetailPosition);
    window.addEventListener("scroll", updateDetailPosition, true);
    return () => {
      window.removeEventListener("resize", updateDetailPosition);
      window.removeEventListener("scroll", updateDetailPosition, true);
    };
  }, [detailOpen, updateDetailPosition]);

  useEffect(() => {
    if (!detailOpen) return;
    detailRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeDetail();
    };
    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (triggerRef.current?.contains(target) || detailRef.current?.contains(target)) return;
      closeDetail(true);
    };
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("mousedown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("mousedown", onPointerDown);
    };
  }, [closeDetail, detailOpen]);

  const sessionTokens = session ? session.input_tokens + session.output_tokens : 0;
  const todayTokens = today ? today.input_tokens + today.output_tokens : 0;
  const hasUsage = usage != null && usage.limit > 0;
  const hasTokens = sessionTokens > 0 || todayTokens > 0;

  // An active session should still expose an honest unknown state while the
  // provider has not reported its first context sample.
  if (!sessionId && !hasUsage && !hasTokens && !showToast) return null;

  const percent = hasUsage
    ? Math.min(100, Math.max(0, (usage.used / usage.limit) * 100))
    : null;
  const roundedPercent = percent == null ? null : Math.round(percent);
  const presentation = contextUsagePresentation(percent ?? 0);
  const meterText = roundedPercent == null
    ? "上下文占用未知"
    : `${presentation.label}，已使用 ${roundedPercent}%`;
  const currentDescription = hasUsage
    ? `${formatContextTokens(usage.used)} / ${formatContextTokens(usage.limit)}（${roundedPercent}%）`
    : "尚未收到当前会话的上下文数据";
  const remainingDescription = hasUsage
    ? `${formatContextTokens(Math.max(0, usage.limit - usage.used))}${
        usage.maxLimit > usage.limit ? `（预算可扩展至 ${formatContextTokens(usage.maxLimit)}）` : ""
      }`
    : "未知";

  return (
    <div
      data-testid="context-usage-bar"
      className="relative inline-flex min-w-0 items-center gap-1"
      data-context-tone={percent == null ? "unknown" : presentation.tone}
    >
      <button
        ref={triggerRef}
        type="button"
        data-testid="context-usage-ring"
        className={`inline-flex min-h-11 min-w-11 items-center justify-center gap-1.5 rounded-lg px-2 text-label transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/60 lg:min-h-9 lg:min-w-9 ${
          percent == null ? "text-gray-500" : presentation.textClass
        }`}
        aria-label={`打开上下文与用量详情，${meterText}`}
        aria-expanded={detailOpen}
        aria-controls={detailId}
        onClick={() => setDetailOpen((open) => !open)}
      >
        {percent == null ? (
          <svg
            viewBox="0 0 24 24"
            className="h-5 w-5 shrink-0 -rotate-90"
            role="img"
            aria-label="上下文占用未知"
          >
            <circle
              cx="12"
              cy="12"
              r="9"
              pathLength="100"
              fill="none"
              strokeWidth="3"
              className="stroke-gray-500"
              strokeDasharray="3 4"
            />
          </svg>
        ) : (
          <svg
            viewBox="0 0 24 24"
            className="h-5 w-5 shrink-0 -rotate-90"
            role="meter"
            aria-label="上下文占用"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={roundedPercent ?? undefined}
            aria-valuetext={meterText}
          >
            <circle
              cx="12"
              cy="12"
              r="9"
              pathLength="100"
              fill="none"
              strokeWidth="3"
              className="stroke-surface-4"
            />
            <circle
              cx="12"
              cy="12"
              r="9"
              pathLength="100"
              fill="none"
              strokeWidth="3"
              strokeLinecap="round"
              className={`${presentation.ringClass} transition-[stroke-dasharray] duration-300 motion-reduce:transition-none`}
              strokeDasharray={`${percent} 100`}
            />
          </svg>
        )}
        {roundedPercent != null && roundedPercent >= 90 ? (
          <span className="whitespace-nowrap font-medium">接近上限</span>
        ) : roundedPercent != null && roundedPercent >= 75 ? (
          <span className="tabular-nums">{roundedPercent}%</span>
        ) : null}
      </button>

      {showToast && toast && (
        <span
          role="status"
          className="whitespace-nowrap rounded-md bg-status-warning-soft px-1.5 py-0.5 text-caption text-status-warning"
          title={`释放约 ${formatContextTokens(toast.tokensFreed)} tokens`}
        >
          已压缩 {toast.elidedCount} 条
        </span>
      )}

      {detailOpen && typeof document !== "undefined" && createPortal(
        <div
          ref={detailRef}
          id={detailId}
          data-testid="context-usage-detail-portal"
          role="dialog"
          aria-label="上下文与用量详情"
          tabIndex={-1}
          className="fixed z-[110] overflow-y-auto rounded-xl border border-border bg-surface-2 p-3 text-left text-label text-gray-300 shadow-2xl outline-none"
          style={detailPosition}
        >
          <div className="mb-2 flex items-center justify-between gap-3">
            <h3 className="text-body font-semibold text-gray-200">上下文与用量详情</h3>
            <button
              type="button"
              aria-label="关闭上下文与用量详情"
              className="flex h-11 w-11 items-center justify-center rounded-lg text-gray-500 hover:bg-surface-3 hover:text-gray-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/60 lg:h-9 lg:w-9"
              onClick={() => closeDetail()}
            >
              <X size={14} aria-hidden="true" />
            </button>
          </div>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 tabular-nums">
            <dt className="text-gray-500">当前上下文</dt>
            <dd className="text-right">{currentDescription}</dd>
            <dt className="text-gray-500">剩余预算</dt>
            <dd className="text-right">{remainingDescription}</dd>
            <dt className="text-gray-500">会话累计</dt>
            <dd className="text-right">{session ? formatContextTokens(sessionTokens) : "暂无记录"}</dd>
            <dt className="text-gray-500">今日累计</dt>
            <dd className="text-right">{today ? formatContextTokens(todayTokens) : "暂无记录"}</dd>
            <dt className="text-gray-500">费用口径</dt>
            <dd className="text-right">{costDescription(today)}</dd>
            <dt className="text-gray-500">上下文压缩</dt>
            <dd className="text-right">
              {toast
                ? `已释放约 ${formatContextTokens(toast.tokensFreed)}，省略 ${toast.elidedCount} 条旧结果`
                : "本轮暂无压缩"}
            </dd>
          </dl>
          {onOpenUsage && (
            <div className="mt-3 flex flex-wrap justify-end gap-2 border-t border-border/60 pt-2">
              {sessionId && (
                <button
                  type="button"
                  className="min-h-11 rounded-lg px-2 text-gray-400 hover:bg-surface-3 hover:text-gray-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/60 lg:min-h-9"
                  onClick={() => onOpenUsage("session")}
                >
                  查看会话统计
                </button>
              )}
              <button
                type="button"
                className="min-h-11 rounded-lg px-2 text-gray-400 hover:bg-surface-3 hover:text-gray-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/60 lg:min-h-9"
                onClick={() => onOpenUsage("today")}
              >
                查看今日统计
              </button>
            </div>
          )}
        </div>,
        document.body,
      )}
    </div>
  );
}
