// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Loader2 } from "lucide-react";
import { invoke } from "../lib/tauri";
import { useSettingsStore } from "../stores/settings";
import { formatUsageTokens } from "./TokenUsageHeatmap";
import { TokenUsageTrend } from "./TokenUsageTrend";
import { type UsageDashboard, usageCostLabel } from "./UsageDashboardSection";

interface Props {
  anonymous: boolean;
  onOpenUsage?: () => void;
}

export function WelcomeUsageCard({ anonymous, onOpenUsage }: Props) {
  const settings = useSettingsStore((state) => state.settings);
  const [dashboard, setDashboard] = useState<UsageDashboard | null>(null);
  const [failed, setFailed] = useState(false);
  const timezoneOffsetMinutes = -new Date().getTimezoneOffset();

  const reload = useCallback(async () => {
    if (anonymous) {
      setDashboard(null);
      setFailed(false);
      return;
    }
    try {
      setDashboard(await invoke<UsageDashboard>("get_usage_dashboard", {
        rangeDays: 28,
        timezoneOffsetMinutes,
      }));
      setFailed(false);
    } catch {
      setFailed(true);
    }
  }, [anonymous, timezoneOffsetMinutes]);

  useEffect(() => { void reload(); }, [reload]);
  useEffect(() => {
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("model-usage-recorded", () => { if (!disposed) void reload(); }).then((unlisten) => {
      if (disposed) unlisten(); else stop = unlisten;
    });
    return () => { disposed = true; stop?.(); };
  }, [reload]);

  const today = dashboard?.heatmap[dashboard.heatmap.length - 1];
  const todayTokens = today?.total_tokens ?? 0;
  const priorCompleteDays = dashboard?.heatmap
    .slice(-8, -1)
    .filter((day) => day.status === "recorded" && day.total_tokens != null) ?? [];
  const sevenDayAverage = priorCompleteDays.length > 0
    ? priorCompleteDays.reduce((sum, day) => sum + (day.total_tokens ?? 0), 0) / priorCompleteDays.length
    : null;
  const dailyLimit = settings?.usage_budget?.daily_token_limit ?? 0;
  const budgetRatio = dailyLimit > 0 ? todayTokens / dailyLimit : null;
  const costLabel = dashboard ? usageCostLabel(dashboard.summary) : null;

  return (
    <section role="region" aria-label="今日用量与过去 4 周趋势" className="rounded-xl border border-border bg-surface-1 p-4">
      <div className="mb-3 flex items-center justify-between gap-3">
        <h2 className="text-xs font-medium text-gray-300">今日用量</h2>
        <button
          type="button"
          aria-label="查看用量详情"
          onClick={onOpenUsage}
          className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-accent transition-colors hover:bg-accent/10 focus:outline-none focus:ring-2 focus:ring-accent/60"
        >
          查看详情<ArrowRight size={11} />
        </button>
      </div>

      {anonymous ? (
        <p className="text-xs text-amber-700 dark:text-amber-300">匿名会话本次临时用量，不计入今日统计</p>
      ) : dashboard ? (
        <div className="grid gap-4 min-[580px]:grid-cols-[minmax(150px,0.75fr)_minmax(280px,1.5fr)] min-[580px]:items-end min-[580px]:gap-6">
          <div className="min-w-0">
            <div className="flex items-baseline gap-1.5">
              <span className="font-mono text-2xl font-semibold tracking-tight text-gray-100">{formatUsageTokens(todayTokens)}</span>
              <span className="text-[10px] uppercase tracking-wide text-gray-400">Tokens</span>
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-2 text-[11px] text-gray-400">
              <span>{today?.requests ?? 0} 次请求</span>
              {costLabel && costLabel !== "费用不可用" && <><span aria-hidden>·</span><span>{costLabel}</span></>}
            </div>
            {budgetRatio != null ? (
              <div className="mt-1 text-[10px] text-gray-400">已使用日预算 {Math.round(budgetRatio * 100)}%</div>
            ) : sevenDayAverage != null ? (
              <div className="mt-1 text-[10px] text-gray-400">近 7 个完整日均值 {formatUsageTokens(Math.round(sevenDayAverage))}</div>
            ) : null}
          </div>
          <div className="min-w-0">
            <div className="mb-1.5 flex items-center justify-between text-[10px] text-gray-400">
              <span>过去 4 周</span>
              <span aria-hidden>较低 · 较高</span>
            </div>
            <TokenUsageTrend
              days={dashboard.heatmap}
              ariaLabel="过去 4 周 Token 趋势"
              dailyBudgetLimit={dailyLimit}
            />
          </div>
        </div>
      ) : failed ? (
        <p className="text-xs text-gray-400">用量统计暂不可用</p>
      ) : (
        <p className="inline-flex items-center gap-1.5 text-xs text-gray-400"><Loader2 size={11} className="animate-spin" />正在读取本机用量</p>
      )}
    </section>
  );
}
