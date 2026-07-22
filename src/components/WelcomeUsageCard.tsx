// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Loader2 } from "lucide-react";
import { invoke } from "../lib/tauri";
import { useSettingsStore } from "../stores/settings";
import { formatUsageTokens, TokenUsageHeatmap } from "./TokenUsageHeatmap";
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

  return (
    <section role="region" aria-label="最近 28 天 Token 用量" className="rounded-lg border border-border bg-surface-1 p-3">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="text-[10px] uppercase tracking-wider text-gray-600">最近 28 天 Token 用量</div>
          {anonymous ? (
            <span className="text-xs text-amber-700 dark:text-amber-300">匿名会话本次临时用量，不计入今日统计</span>
          ) : dashboard ? (
            <div className="mt-0.5 space-y-1">
              <div className="text-[10px] font-medium text-gray-500">今日用量</div>
              <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <span className="font-mono text-sm font-semibold text-gray-200">{formatUsageTokens(todayTokens)}</span>
                <span className="text-[10px] text-gray-500">{today?.requests ?? 0} 次请求</span>
                <span className="text-[10px] text-gray-500">{usageCostLabel(dashboard.summary)}</span>
              </div>
              {budgetRatio != null ? (
                <div className="text-[10px] text-gray-500">已使用日预算 {Math.round(budgetRatio * 100)}%</div>
              ) : sevenDayAverage != null ? (
                <div className="text-[10px] text-gray-500">最近 7 个完整日均值 {formatUsageTokens(Math.round(sevenDayAverage))}</div>
              ) : null}
            </div>
          ) : failed ? (
            <span className="text-xs text-gray-500">用量统计暂不可用</span>
          ) : (
            <span className="inline-flex items-center gap-1 text-xs text-gray-500"><Loader2 size={10} className="animate-spin" />正在读取</span>
          )}
        </div>
        <button type="button" aria-label="查看用量详情" onClick={onOpenUsage} className="flex items-center gap-1 text-xs text-accent hover:underline">
          查看详情<ArrowRight size={11} />
        </button>
      </div>
      {!anonymous && dashboard && (
        <div>
          <div className="mb-1 text-[10px] text-gray-600">最近 28 天</div>
          <TokenUsageHeatmap days={dashboard.heatmap} ariaLabel="最近 28 天 Token 消耗" compact />
        </div>
      )}
    </section>
  );
}
