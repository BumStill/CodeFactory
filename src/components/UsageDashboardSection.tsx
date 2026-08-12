// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Activity, Check, Loader2, RefreshCw } from "lucide-react";
import { invoke } from "../lib/tauri";
import { useSettingsStore } from "../stores/settings";
import {
  formatUsageTokens,
  TokenUsageHeatmap,
  UsageHeatmapLegend,
  type UsageHeatmapMetric,
  type UsageHeatmapDay,
} from "./TokenUsageHeatmap";

export interface UsageSummary {
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cached_tokens: number;
  requests: number;
  actual_cost_usd: number | null;
  estimated_cost_usd: number | null;
  cost_source: string;
  data_status?: "complete" | "partial" | "unavailable";
  missing_usage_count?: number;
  source_counts?: Record<string, number>;
}

export interface UsageBreakdown {
  surface: string;
  total_tokens: number;
  requests: number;
}

export interface TopUsageSession {
  session_id: string;
  job_session_id?: string | null;
  title: string;
  surface?: string;
  task_id?: string | null;
  total_tokens: number;
  requests: number;
  share: number;
}

export interface UsageDashboard {
  range_days: number;
  start_utc?: string;
  end_utc?: string;
  data_status?: "complete" | "partial" | "unavailable";
  summary: UsageSummary;
  heatmap: UsageHeatmapDay[];
  breakdowns: UsageBreakdown[];
  top_sessions: TopUsageSession[];
}

interface UsageDayDetail {
  local_date: string;
  start_utc?: string;
  end_utc?: string;
  data_status?: "complete" | "partial" | "unavailable";
  summary: UsageSummary;
  breakdowns: UsageBreakdown[];
  top_sessions: TopUsageSession[];
}

interface UsageBudgetStatus {
  daily: { usage_tokens: number; limit_tokens: number; ratio: number | null };
  monthly: { usage_tokens: number; limit_tokens: number; ratio: number | null };
  new_alert: null | {
    period_kind: "day" | "month";
    threshold: number;
    usage_tokens: number;
    limit_tokens: number;
  };
}

const SURFACE_LABELS: Record<string, string> = {
  interactive: "交互会话",
  autonomous: "自主任务",
  subagent: "子 Agent",
  eval: "Evals",
  session_title: "会话命名",
};

export function usageCostLabel(summary: UsageSummary): string {
  if (summary.cost_source === "subscription") return "订阅流量";
  if (summary.cost_source === "local") return "本地模型";
  if (summary.cost_source === "provider_actual" && summary.actual_cost_usd != null) {
    return `实际费用 $${summary.actual_cost_usd.toFixed(4)}`;
  }
  if ((summary.cost_source === "model_price_estimate" || summary.cost_source === "legacy_estimate")
      && summary.estimated_cost_usd != null) {
    return `估算费用 $${summary.estimated_cost_usd.toFixed(4)}`;
  }
  if (summary.cost_source === "mixed") return "多种计费方式（费用分项）";
  return "费用不可用";
}

function Kpi({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="rounded-lg border border-border bg-surface-1 p-3 min-w-0">
      <div className="text-[10px] font-medium text-gray-500">{label}</div>
      <div className="mt-1 truncate font-mono text-base font-semibold text-gray-200" title={value}>{value}</div>
      {hint && <div className="mt-0.5 truncate text-[10px] text-gray-600" title={hint}>{hint}</div>}
    </div>
  );
}

interface Props {
  onOpenSession?: (sessionId: string) => void;
  onOpenJobLog?: (sessionId: string, taskId: string) => void;
}

export function UsageDashboardSection({ onOpenSession, onOpenJobLog }: Props) {
  const { settings, save } = useSettingsStore();
  const [rangeDays, setRangeDays] = useState(365);
  const [summaryRangeDays, setSummaryRangeDays] = useState<1 | 7 | 30>(1);
  const [mapMetric, setMapMetric] = useState<UsageHeatmapMetric>("tokens");
  const [dashboard, setDashboard] = useState<UsageDashboard | null>(null);
  const [summaryDashboard, setSummaryDashboard] = useState<UsageDashboard | null>(null);
  const [selectedDate, setSelectedDate] = useState<string | null>(null);
  const [detail, setDetail] = useState<UsageDayDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [detailLoading, setDetailLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [budgetSaved, setBudgetSaved] = useState(false);
  const [budgetStatus, setBudgetStatus] = useState<UsageBudgetStatus | null>(null);
  const dashboardRequestId = useRef(0);
  const detailRequestId = useRef(0);
  const [budget, setBudget] = useState({
    daily_token_limit: settings?.usage_budget?.daily_token_limit ?? 0,
    monthly_token_limit: settings?.usage_budget?.monthly_token_limit ?? 0,
    alert_thresholds: settings?.usage_budget?.alert_thresholds ?? [0.5, 0.8, 1],
    alerts_enabled: settings?.usage_budget?.alerts_enabled ?? true,
  });
  const timezoneOffsetMinutes = -new Date().getTimezoneOffset();

  const reload = useCallback(async () => {
    const requestId = ++dashboardRequestId.current;
    setLoading(true);
    setError(null);
    try {
      const [next, nextSummary, nextBudgetStatus] = await Promise.all([
        invoke<UsageDashboard>("get_usage_dashboard", {
          rangeDays,
          timezoneOffsetMinutes,
        }),
        invoke<UsageDashboard>("get_usage_dashboard", {
          rangeDays: summaryRangeDays,
          timezoneOffsetMinutes,
        }),
        invoke<UsageBudgetStatus>("get_usage_budget_status", {
          timezoneOffsetMinutes,
        }).catch(() => null),
      ]);
      if (requestId === dashboardRequestId.current) {
        setDashboard(next);
        setSummaryDashboard(nextSummary);
        if (nextBudgetStatus) setBudgetStatus(nextBudgetStatus);
      }
    } catch (cause) {
      if (requestId === dashboardRequestId.current) setError(String(cause));
    } finally {
      if (requestId === dashboardRequestId.current) setLoading(false);
    }
  }, [rangeDays, summaryRangeDays, timezoneOffsetMinutes]);

  useEffect(() => { void reload(); }, [reload]);
  useEffect(() => {
    if (!settings?.usage_budget) return;
    setBudget(settings.usage_budget);
  }, [settings?.usage_budget]);
  useEffect(() => {
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
    let disposed = false;
    const stops = new Set<() => void>();
    for (const eventName of ["model-usage-recorded", "session-title-updated"]) {
      void listen(eventName, () => { if (!disposed) void reload(); }).then((unlisten) => {
        if (disposed) unlisten(); else stops.add(unlisten);
      });
    }
    return () => {
      disposed = true;
      stops.forEach((stop) => stop());
    };
  }, [reload]);

  const selectDay = async (localDate: string | null) => {
    const requestId = ++detailRequestId.current;
    if (localDate == null) {
      setSelectedDate(null);
      setDetail(null);
      setDetailLoading(false);
      return;
    }
    setSelectedDate(localDate);
    setDetail(null);
    const selectedDay = dashboard?.heatmap.find((day) => day.local_date === localDate);
    if (selectedDay?.status === "missing") {
      setDetailLoading(false);
      return;
    }
    setDetailLoading(true);
    try {
      const next = await invoke<UsageDayDetail>("get_usage_day_detail", {
        localDate,
        timezoneOffsetMinutes,
      });
      if (requestId === detailRequestId.current) setDetail(next);
    } catch (cause) {
      if (requestId === detailRequestId.current) setError(String(cause));
    } finally {
      if (requestId === detailRequestId.current) setDetailLoading(false);
    }
  };

  const summary = summaryDashboard?.summary;
  const total = summary ? summary.input_tokens + summary.output_tokens : 0;
  const todayTokens = dashboard && dashboard.heatmap.length > 0
    ? dashboard.heatmap[dashboard.heatmap.length - 1]?.total_tokens ?? 0
    : 0;
  const dailyBudgetRatio = budgetStatus?.daily?.ratio ?? (budget.daily_token_limit > 0
    ? todayTokens / budget.daily_token_limit
    : null);
  const monthlyBudgetRatio = budgetStatus?.monthly?.ratio ?? null;
  const selectedDay = selectedDate
    ? dashboard?.heatmap.find((day) => day.local_date === selectedDate)
    : undefined;
  const saveBudget = async () => {
    if (!settings) return;
    await save({ ...settings, usage_budget: budget });
    await reload();
    setBudgetSaved(true);
    setTimeout(() => setBudgetSaved(false), 1500);
  };

  return (
    <section role="region" aria-label="用量与预算" className="mx-auto w-full max-w-6xl space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex rounded border border-border p-0.5 text-[10px]" aria-label="摘要范围">
            {([1, 7, 30] as const).map((days) => (
              <button
                type="button"
                key={days}
                aria-pressed={summaryRangeDays === days}
                onClick={() => setSummaryRangeDays(days)}
                className={`rounded px-2 py-1 ${summaryRangeDays === days ? "bg-surface-3 text-accent" : "text-gray-500 hover:text-gray-300"}`}
              >
                {days === 1 ? "今天" : `${days} 天`}
              </button>
            ))}
          </div>
          <button type="button" onClick={() => void reload()} className="flex items-center gap-1 rounded border border-border px-2 py-1 text-xs text-gray-500 hover:bg-surface-2 hover:text-gray-300">
            <RefreshCw size={11} className={loading ? "animate-spin" : ""} />刷新
          </button>
        </div>
      </div>

      {error && (
        <div role="alert" className="rounded border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          用量统计暂不可用：{error}
        </div>
      )}
      {dashboard?.data_status === "partial" && (
        <div role="status" className="rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
          部分请求缺少 Provider Usage 或来自历史回填；当前总量可能不完整。
        </div>
      )}
      {budgetStatus?.new_alert && (
        <div role="alert" className="rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
          {budgetStatus.new_alert.period_kind === "day" ? "今日" : "本月"} Token 已达到预算的 {Math.round(budgetStatus.new_alert.threshold * 100)}%。本提醒已记录，本周期不会重复出现。
        </div>
      )}

      <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
        <Kpi label="总 Tokens" value={summary ? formatUsageTokens(total) : "—"} hint={summary ? `输入 ${formatUsageTokens(summary.input_tokens)} · 输出 ${formatUsageTokens(summary.output_tokens)}` : undefined} />
        <Kpi label="模型请求" value={summary ? `${summary.requests} 次` : "—"} />
        <Kpi label="推理 / 缓存" value={summary ? `${formatUsageTokens(summary.reasoning_tokens)} / ${formatUsageTokens(summary.cached_tokens)}` : "—"} />
        <Kpi label="费用语义" value={summary ? usageCostLabel(summary) : "—"} />
      </div>

      <div className="rounded-lg border border-border bg-surface-1 p-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <h3 className="text-xs font-medium text-gray-300">Token 预算</h3>
            <p className="mt-0.5 text-[10px] text-gray-600">预算只提醒，不会停止任务、切换模型或修改权限。</p>
            {dailyBudgetRatio != null && (
              <div className="mt-2">
                <div className="mb-1 flex justify-between text-[10px] text-gray-500">
                  <span>今日 {formatUsageTokens(todayTokens)} / {formatUsageTokens(budget.daily_token_limit)}</span>
                  <span>{Math.round(dailyBudgetRatio * 100)}%</span>
                </div>
                <div className="h-1.5 overflow-hidden rounded-full bg-surface-3">
                  <div
                    className={`h-full transition-all ${dailyBudgetRatio >= 1 ? "bg-red-500" : dailyBudgetRatio >= 0.8 ? "bg-amber-500" : "bg-accent"}`}
                    style={{ width: `${Math.min(100, dailyBudgetRatio * 100)}%` }}
                  />
                </div>
                {budget.alerts_enabled && dailyBudgetRatio >= 0.5 && (
                  <p role="status" className={`mt-1 text-[10px] ${dailyBudgetRatio >= 1 ? "text-red-700 dark:text-red-300" : "text-amber-700 dark:text-amber-300"}`}>
                    {dailyBudgetRatio >= 1 ? "今日 Token 已达到预算" : dailyBudgetRatio >= 0.8 ? "今日 Token 已达到预算的 80%" : "今日 Token 已达到预算的 50%"}
                  </p>
                )}
                {monthlyBudgetRatio != null && (
                  <p className="mt-1 text-[10px] text-gray-500">
                    本月 {formatUsageTokens(budgetStatus?.monthly?.usage_tokens ?? 0)} / {formatUsageTokens(budgetStatus?.monthly?.limit_tokens ?? budget.monthly_token_limit)} · {Math.round(monthlyBudgetRatio * 100)}%
                  </p>
                )}
              </div>
            )}
          </div>
          <div className="grid w-full grid-cols-1 gap-2 sm:w-auto sm:grid-cols-2">
            <label className="text-[10px] text-gray-500">
              每日上限
              <input
                type="number"
                min={0}
                step={1000}
                value={budget.daily_token_limit}
                onChange={(event) => setBudget({ ...budget, daily_token_limit: Math.max(0, Number(event.target.value) || 0) })}
                className="mt-1 w-full rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
              />
            </label>
            <label className="text-[10px] text-gray-500">
              每月上限
              <input
                type="number"
                min={0}
                step={10000}
                value={budget.monthly_token_limit}
                onChange={(event) => setBudget({ ...budget, monthly_token_limit: Math.max(0, Number(event.target.value) || 0) })}
                className="mt-1 w-full rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
              />
            </label>
            <label className="flex items-center gap-1.5 text-[10px] text-gray-500 sm:col-span-2">
              <input type="checkbox" checked={budget.alerts_enabled} onChange={(event) => setBudget({ ...budget, alerts_enabled: event.target.checked })} />
              50% / 80% / 100% 阈值提醒
            </label>
            <button type="button" onClick={() => void saveBudget()} className="flex items-center justify-center gap-1 rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent-hover sm:col-span-2">
              {budgetSaved ? <><Check size={11} />已保存</> : "保存预算"}
            </button>
          </div>
        </div>
      </div>

      <div className="rounded-lg border border-border bg-surface-1 p-3">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <div>
            <h3 className="text-xs font-medium text-gray-300">Token 消耗地图</h3>
            <p className="mt-0.5 text-[10px] text-gray-600">每格一天，深浅按对数尺度；悬浮或聚焦查看精确值。</p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex rounded border border-border p-0.5 text-[10px]" aria-label="地图指标">
              {(["tokens", "budget", "requests"] as UsageHeatmapMetric[]).map((metric) => (
                <button
                  type="button"
                  key={metric}
                  aria-pressed={mapMetric === metric}
                  onClick={() => setMapMetric(metric)}
                  className={`rounded px-2 py-1 ${mapMetric === metric ? "bg-surface-3 text-accent" : "text-gray-500 hover:text-gray-300"}`}
                >
                  {metric === "tokens" ? "Tokens" : metric === "budget" ? "预算占比" : "请求次数"}
                </button>
              ))}
            </div>
            <div className="flex rounded border border-border p-0.5 text-[10px]">
              {([90, 180, 365] as const).map((days) => (
              <button
                type="button"
                key={days}
                aria-label={`近 ${days} 天`}
                aria-pressed={rangeDays === days}
                onClick={() => { setRangeDays(days); void selectDay(null); }}
                className={`rounded px-2 py-1 ${rangeDays === days ? "bg-surface-3 text-accent" : "text-gray-500 hover:text-gray-300"}`}
              >
                {days === 180 ? "半年" : days === 365 ? "一年" : "90 天"}
              </button>
              ))}
            </div>
          </div>
        </div>
        {loading && !dashboard ? (
          <div className="flex h-24 items-center justify-center gap-2 text-xs text-gray-500"><Loader2 size={13} className="animate-spin" />正在读取本机用量</div>
        ) : (
          <TokenUsageHeatmap
            days={dashboard?.heatmap ?? []}
            ariaLabel={`Token 消耗地图，近 ${rangeDays} 天`}
            selectedDate={selectedDate}
            onSelectDate={(date) => void selectDay(date)}
            metric={mapMetric}
            dailyBudgetLimit={budget.daily_token_limit}
          />
        )}
        <div className="mt-3"><UsageHeatmapLegend /></div>
      </div>

      {selectedDate && (
        <section role="region" aria-label={`${selectedDate} 用量明细`} className="rounded-lg border border-border bg-surface-1 p-3">
          <div className="mb-3 flex items-center gap-2">
            <Activity size={12} className="text-accent" />
            <h3 className="text-xs font-medium text-gray-300">{selectedDate} 用量明细</h3>
            {detailLoading && <Loader2 size={11} className="animate-spin text-gray-500" />}
          </div>
          {selectedDay?.status === "missing" && (
            <p role="status" className="mb-3 rounded border border-gray-700 bg-surface-2 px-2.5 py-2 text-xs text-gray-400">
              数据缺失，不等于 0 用量；该日没有可用于下钻的完整计量记录。
            </p>
          )}
          {selectedDay?.status === "partial" && (
            <p role="status" className="mb-3 rounded border border-amber-500/30 bg-amber-500/10 px-2.5 py-2 text-xs text-amber-700 dark:text-amber-300">
              历史回填，数据可能不完整；Token 与请求拆分仅代表当前可恢复记录。
            </p>
          )}
          {detail && (
            <div className="grid gap-4 lg:grid-cols-2">
              <div>
                <div className="mb-2 text-[10px] font-medium text-gray-600">执行入口</div>
                <div className="space-y-1">
                  {detail.breakdowns.length === 0 && <p className="text-xs text-gray-500">当天没有已计量请求</p>}
                  {detail.breakdowns.map((item) => (
                    <div key={item.surface} className="flex items-center justify-between rounded bg-surface-2 px-2.5 py-2 text-xs">
                      <span className="text-gray-300">{SURFACE_LABELS[item.surface] ?? item.surface}</span>
                      <span className="font-mono text-gray-500">{formatUsageTokens(item.total_tokens)} · {item.requests} 次</span>
                    </div>
                  ))}
                </div>
              </div>
              <div>
                <div className="mb-2 text-[10px] font-medium text-gray-600">高消耗会话</div>
                <div className="space-y-1">
                  {detail.top_sessions.length === 0 && <p className="text-xs text-gray-500">无会话记录</p>}
                  {detail.top_sessions.map((session) => (
                    <div key={session.session_id} className="flex flex-wrap items-center gap-2 rounded bg-surface-2 px-2.5 py-2 text-xs">
                      <span className="min-w-0 flex-1 truncate text-gray-300" title={session.title}>{session.title}</span>
                      <span className="font-mono text-[10px] text-gray-500">{formatUsageTokens(session.total_tokens)} · {(session.share * 100).toFixed(0)}%</span>
                      {onOpenSession && (
                        <button type="button" onClick={() => onOpenSession(session.session_id)} className="text-[10px] text-accent hover:underline">
                          查看会话
                        </button>
                      )}
                      {onOpenJobLog && session.task_id && session.job_session_id && (
                        <button
                          type="button"
                          onClick={() => onOpenJobLog(session.job_session_id!, session.task_id!)}
                          className="text-[10px] text-accent hover:underline"
                        >
                          查看作业日志
                        </button>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            </div>
          )}
        </section>
      )}
    </section>
  );
}
