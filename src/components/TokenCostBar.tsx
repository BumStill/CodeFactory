// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "../lib/tauri";

interface CostSummary {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return n.toString();
}

interface Props {
  sessionId?: string | null;
}

export function TokenCostBar({ sessionId }: Props) {
  const [session, setSession] = useState<CostSummary | null>(null);
  const [today, setToday] = useState<CostSummary | null>(null);
  const [monthly, setMonthly] = useState<CostSummary | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [t, m] = await Promise.all([
        invoke<CostSummary>("get_today_cost"),
        invoke<CostSummary>("get_monthly_cost"),
      ]);
      setToday(t);
      setMonthly(m);
    } catch {
      // DB may not be ready yet — ignore
    }

    if (sessionId) {
      try {
        const s = await invoke<CostSummary>("get_session_cost", { sessionId });
        setSession(s);
      } catch {
        setSession(null);
      }
    } else {
      setSession(null);
    }
  }, [sessionId]);

  // Refresh on mount + whenever session changes
  useEffect(() => {
    refresh();
  }, [refresh]);

  // Listen for the backend event fired after each AI response
  useEffect(() => {
    let cancel = false;
    let unlisten: (() => void) | null = null;

    listen<string>("token-usage-recorded", () => {
      if (!cancel) refresh();
    }).then((fn) => {
      if (cancel) fn();
      else unlisten = fn;
    });

    return () => {
      cancel = true;
      unlisten?.();
    };
  }, [refresh]);

  const sessionTok = session ? session.input_tokens + session.output_tokens : 0;
  const todayTok = today ? today.input_tokens + today.output_tokens : 0;
  const monthCost = monthly?.cost_usd ?? 0;

  // Hide bar entirely if there's no data yet
  if (sessionTok === 0 && todayTok === 0 && monthCost === 0) return null;

  return (
    <div className="flex items-center gap-4 px-4 py-1 border-t border-border text-[11px] text-gray-600 bg-surface-1 shrink-0 select-none">
      {sessionTok > 0 && (
        <span title="当前会话">
          📊 {fmtTokens(sessionTok)} tok
        </span>
      )}
      {todayTok > 0 && (
        <span title="今日合计">
          📅 {fmtTokens(todayTok)} 今日
        </span>
      )}
      {monthCost > 0.0001 && (
        <span title="本月(估算)">
          💰 ${monthCost.toFixed(4)}/mo
        </span>
      )}
    </div>
  );
}
