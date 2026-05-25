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

interface CostSummary {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

interface Props {
  sessionId?: string | null;
}

export function ContextUsageBar({ sessionId }: Props) {
  const usage = useChatStore((s) => s.contextUsage);
  const toast = useChatStore((s) => s.compressionToast);

  const [session, setSession] = useState<CostSummary | null>(null);
  const [today, setToday] = useState<CostSummary | null>(null);
  const [monthly, setMonthly] = useState<CostSummary | null>(null);
  const [showToast, setShowToast] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [t, m] = await Promise.all([
        invoke<CostSummary>("get_today_cost"),
        invoke<CostSummary>("get_monthly_cost"),
      ]);
      setToday(t);
      setMonthly(m);
    } catch {
      // DB may not be ready
    }
    if (sessionId) {
      try {
        setSession(await invoke<CostSummary>("get_session_cost", { sessionId }));
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
    listen<string>("token-usage-recorded", () => {
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
  const monthCost = monthly?.cost_usd ?? 0;
  const hasUsage = usage && usage.limit > 0;
  const hasTokens = sessionTok > 0 || todayTok > 0 || monthCost > 0.0001;

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
            <span title="This session">
              <span className="text-gray-600 mr-1">session</span>
              {fmtTokens(sessionTok)}
            </span>
          )}
          {todayTok > 0 && (
            <span title="Today's total">
              <span className="text-gray-600 mr-1">today</span>
              {fmtTokens(todayTok)}
            </span>
          )}
          {monthCost > 0.0001 && (
            <span title="This month (estimated)">
              <span className="text-gray-600 mr-1">month</span>
              ${monthCost.toFixed(4)}
            </span>
          )}
        </div>
      )}

      {/* Compression toast — middle, transient */}
      {showToast && toast && (
        <span
          className="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-800 dark:text-amber-300 text-[10px]"
          title={`Freed ~${fmtTokens(toast.tokensFreed)} tokens`}
        >
          compressed {toast.elidedCount} old result{toast.elidedCount === 1 ? "" : "s"}
        </span>
      )}

      {/* Context window usage — right-aligned */}
      {hasUsage && (
        <div className="flex items-center gap-2 ml-auto tabular-nums">
          <span className="text-gray-600">context</span>
          <span className={toneText}>
            {fmtTokens(usage!.used)} / {fmtTokens(usage!.limit)}
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
