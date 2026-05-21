// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { Gauge, Archive } from "lucide-react";
import { useChatStore } from "../stores/chat";

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

/**
 * Live readout of how full the model's context window is. Reads
 * `contextUsage` from the chat store — populated each turn from the
 * provider's `prompt_tokens`. Color-coded:
 *   - green  < 60 %
 *   - amber  60-85 %
 *   - red    > 85 %
 *
 * Also shows a transient pill whenever the backend just elided old
 * context to fit the next request under the limit.
 */
export function ContextUsageBar() {
  const usage = useChatStore((s) => s.contextUsage);
  const toast = useChatStore((s) => s.compressionToast);

  const [showToast, setShowToast] = useState(false);
  useEffect(() => {
    if (!toast) return;
    setShowToast(true);
    const t = setTimeout(() => setShowToast(false), 4500);
    return () => clearTimeout(t);
  }, [toast?.id]);

  if (!usage || usage.limit === 0) return null;

  const pct = Math.min(100, (usage.used / usage.limit) * 100);
  const tone =
    pct < 60
      ? { bar: "bg-emerald-500", text: "text-emerald-400" }
      : pct < 85
        ? { bar: "bg-amber-500",   text: "text-amber-400" }
        : { bar: "bg-rose-500",    text: "text-rose-400" };

  return (
    <div className="flex items-center gap-3 px-4 py-1 border-t border-border text-[11px] bg-surface-1 shrink-0 select-none">
      <Gauge size={11} className="text-gray-600 shrink-0" />
      <div className="flex items-center gap-2 flex-1 min-w-0">
        <span className="text-gray-500 shrink-0 tabular-nums">
          {fmtTokens(usage.used)} / {fmtTokens(usage.limit)}
        </span>
        <div className="flex-1 h-1 rounded-full bg-surface-3 overflow-hidden max-w-[200px]">
          <div
            className={`h-full ${tone.bar} transition-all duration-300`}
            style={{ width: `${pct}%` }}
          />
        </div>
        <span className={`shrink-0 ${tone.text} tabular-nums`}>
          {pct.toFixed(0)}%
        </span>
      </div>

      {showToast && toast && (
        <span
          className="flex items-center gap-1 px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 text-[10px]"
          title={`Freed ~${fmtTokens(toast.tokensFreed)} tokens`}
        >
          <Archive size={10} />
          compressed {toast.elidedCount} old result{toast.elidedCount === 1 ? "" : "s"}
        </span>
      )}
    </div>
  );
}
