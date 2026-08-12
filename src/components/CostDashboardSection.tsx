// SPDX-License-Identifier: Apache-2.0
//
// Cost dashboard — shows token + dollar spend across all sessions, with
// scope toggle (today / month / all) and a per-model breakdown.
//
// Lives on the Profile page so it's discoverable from the same place as
// preferences and learning log — the "how am I spending my AI budget"
// question naturally sits next to the "how is AI learning about me"
// question.
//
// Token economy: every call here reads from cost_entries (already
// recorded by the agent loop). Zero extra model calls.

import { useEffect, useState } from "react";
import { DollarSign, Activity, Loader2 } from "lucide-react";
import { invoke } from "../lib/tauri";

interface CostSummary {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

interface CostByModel {
  model: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  calls: number;
}

interface RecentCostEntry {
  id: string;
  session_id: string;
  model: string;
  endpoint: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  created_at: string;
}

type Scope = "today" | "month" | "all";

function fmtUsd(n: number): string {
  if (n === 0) return "$0";
  if (n < 0.01) return `<$0.01`;
  if (n < 1) return `$${n.toFixed(3)}`;
  return `$${n.toFixed(2)}`;
}

function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

export function CostDashboardSection() {
  const [scope, setScope] = useState<Scope>("today");
  const [summary, setSummary] = useState<CostSummary | null>(null);
  const [byModel, setByModel] = useState<CostByModel[]>([]);
  const [recent, setRecent] = useState<RecentCostEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const reload = async () => {
      setLoading(true);
      setError(null);
      try {
        const summaryCmd =
          scope === "today" ? "get_today_cost" :
          scope === "month" ? "get_monthly_cost" :
          // "all" → sum of all model rows
          null;
        const [sumResult, models, recentList] = await Promise.all([
          summaryCmd
            ? invoke<CostSummary>(summaryCmd)
            : Promise.resolve(null),
          invoke<CostByModel[]>("get_costs_by_model", { scope }),
          invoke<RecentCostEntry[]>("list_recent_cost_entries", { limit: 20 }),
        ]);
        if (cancelled) return;

        // For "all", derive summary from per-model rows so we don't add
        // a fourth backend command just for one number.
        const finalSummary: CostSummary = sumResult ?? models.reduce(
          (acc, m) => ({
            input_tokens: acc.input_tokens + m.input_tokens,
            output_tokens: acc.output_tokens + m.output_tokens,
            cost_usd: acc.cost_usd + m.cost_usd,
          }),
          { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
        );

        setSummary(finalSummary);
        setByModel(models);
        setRecent(recentList);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void reload();
    return () => { cancelled = true; };
  }, [scope]);

  const totalCallsAllScopes = byModel.reduce((s, m) => s + m.calls, 0);

  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-label font-semibold text-gray-400 flex items-center gap-1.5">
          <DollarSign size={12} className="text-accent" />
          成本透视
        </h2>
        <div className="flex items-center rounded border border-border overflow-hidden text-caption">
          {(["today", "month", "all"] as Scope[]).map((s) => (
            <button
              key={s}
              onClick={() => setScope(s)}
              className={`px-2 py-0.5 transition-colors ${
                scope === s
                  ? "bg-surface-3 text-accent"
                  : "text-gray-500 hover:text-gray-300 hover:bg-surface-3"
              }`}
            >
              {s === "today" ? "今天" : s === "month" ? "本月" : "全部"}
            </button>
          ))}
        </div>
      </div>

      {error && <p className="text-label text-red-700 dark:text-red-300 mb-2">{error}</p>}

      {/* Headline row */}
      <div className="grid grid-cols-3 gap-3 mb-4">
        <div className="rounded-lg border border-border bg-surface-1 p-3">
          <div className="text-caption text-gray-500 mb-0.5">花费</div>
          <div className="text-title font-semibold text-gray-200 font-mono">
            {summary ? fmtUsd(summary.cost_usd) : <Loader2 size={12} className="animate-spin inline" />}
          </div>
        </div>
        <div className="rounded-lg border border-border bg-surface-1 p-3">
          <div className="text-caption text-gray-500 mb-0.5">输入 Token</div>
          <div className="text-title font-semibold text-gray-200 font-mono">
            {summary ? fmtTokens(summary.input_tokens) : "—"}
          </div>
        </div>
        <div className="rounded-lg border border-border bg-surface-1 p-3">
          <div className="text-caption text-gray-500 mb-0.5">输出 Token</div>
          <div className="text-title font-semibold text-gray-200 font-mono">
            {summary ? fmtTokens(summary.output_tokens) : "—"}
          </div>
        </div>
      </div>

      {/* By model */}
      <div className="mb-4">
        <h3 className="text-caption font-medium text-gray-400 mb-1.5">按模型</h3>
        {loading && byModel.length === 0 ? (
          <p className="text-label text-gray-500 text-center py-4 flex items-center justify-center gap-2">
            <Loader2 size={11} className="animate-spin" /> 加载中
          </p>
        ) : byModel.length === 0 ? (
          <p className="text-label text-gray-500 text-center py-4">
            {scope === "today" ? "今天" : scope === "month" ? "本月" : ""}还没有消费记录
          </p>
        ) : (
          <div className="rounded-lg border border-border bg-surface-1 divide-y divide-border">
            {byModel.map((m) => {
              const pct = summary && summary.cost_usd > 0
                ? (m.cost_usd / summary.cost_usd) * 100
                : 0;
              return (
                <div key={m.model} className="px-3 py-2">
                  <div className="flex items-center gap-2 mb-1">
                    <span className="text-label text-gray-300 font-mono truncate flex-1" title={m.model}>
                      {m.model}
                    </span>
                    <span className="text-label text-gray-200 font-mono">{fmtUsd(m.cost_usd)}</span>
                  </div>
                  <div className="flex items-center gap-2 text-caption text-gray-500">
                    <div className="flex-1 h-1 rounded bg-surface-3 overflow-hidden">
                      <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
                    </div>
                    <span className="font-mono">
                      {fmtTokens(m.input_tokens)} in · {fmtTokens(m.output_tokens)} out · {m.calls} 次
                    </span>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Recent activity */}
      <div>
        <h3 className="text-caption font-medium text-gray-400 mb-1.5 flex items-center gap-1">
          <Activity size={10} /> 最近活动
          {totalCallsAllScopes > 0 && (
            <span className="text-gray-600 font-normal">· 共 {totalCallsAllScopes} 次调用</span>
          )}
        </h3>
        {recent.length === 0 ? (
          <p className="text-label text-gray-500 text-center py-3">无</p>
        ) : (
          <details className="rounded-lg border border-border bg-surface-1">
            <summary className="px-3 py-2 text-caption text-gray-500 cursor-pointer hover:text-gray-300 select-none">
              展开最近 {recent.length} 条
            </summary>
            <ul className="border-t border-border divide-y divide-border max-h-60 overflow-y-auto">
              {recent.map((r) => (
                <li key={r.id} className="px-3 py-1.5 flex items-center gap-2 text-caption">
                  <span className="text-caption text-gray-600 font-mono shrink-0">
                    {r.created_at.slice(5, 16).replace("T", " ")}
                  </span>
                  <span className="text-gray-300 font-mono truncate flex-1" title={r.model}>
                    {r.model}
                  </span>
                  <span className="text-gray-500 font-mono shrink-0">
                    {fmtTokens(r.input_tokens + r.output_tokens)} tok
                  </span>
                  <span className="text-gray-200 font-mono shrink-0 w-14 text-right">
                    {fmtUsd(r.cost_usd)}
                  </span>
                </li>
              ))}
            </ul>
          </details>
        )}
      </div>

      <p className="mt-2 text-caption text-gray-600">
        定价基于默认估算（input $1/M · output $3/M）。后续支持按模型自定义定价。
      </p>
    </section>
  );
}
