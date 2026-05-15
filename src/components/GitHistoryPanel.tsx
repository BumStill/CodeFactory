// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { X, History, RefreshCw, ChevronRight, ChevronDown } from "lucide-react";
import { useGitStore } from "../stores/git";

interface Props {
  onClose: () => void;
}

export function GitHistoryPanel({ onClose }: Props) {
  const { commits, refreshCommits } = useGitStore();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    setLoading(true);
    refreshCommits(50).finally(() => setLoading(false));
  }, [refreshCommits]);

  const toggle = (hash: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(hash)) next.delete(hash);
      else next.add(hash);
      return next;
    });
  };

  const handleRefresh = async () => {
    setLoading(true);
    try {
      await refreshCommits(50);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed right-0 top-0 bottom-0 z-40 w-[480px] max-w-[70vw] bg-surface-1 border-l border-border shadow-2xl flex flex-col">
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <History size={14} className="text-gray-400" />
        <span className="text-xs font-semibold text-gray-200 flex-1">History</span>
        <button
          onClick={handleRefresh}
          className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors"
          title="Refresh"
        >
          <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
        </button>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors"
          title="Close"
        >
          <X size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto">
        {commits.length === 0 && !loading && (
          <div className="text-[11px] text-gray-700 text-center pt-8">No commits</div>
        )}
        {loading && commits.length === 0 && (
          <div className="text-[11px] text-gray-600 text-center pt-8">Loading…</div>
        )}
        {commits.map((c) => {
          const isExpanded = expanded.has(c.hash);
          return (
            <div key={c.hash} className="border-b border-border">
              <button
                onClick={() => toggle(c.hash)}
                className="w-full text-left px-3 py-2 hover:bg-surface-2 transition-colors flex items-start gap-2"
              >
                {isExpanded ? (
                  <ChevronDown size={12} className="text-gray-600 shrink-0 mt-0.5" />
                ) : (
                  <ChevronRight size={12} className="text-gray-600 shrink-0 mt-0.5" />
                )}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 text-[11px]">
                    <span className="font-mono text-amber-400 shrink-0">{c.short_hash}</span>
                    <span className="text-gray-200 truncate flex-1">{c.message}</span>
                  </div>
                  <div className="flex items-center gap-2 text-[10px] text-gray-600 mt-0.5">
                    <span className="truncate">{c.author}</span>
                    <span className="text-gray-700">·</span>
                    <span>{formatRelative(c.timestamp)}</span>
                  </div>
                </div>
              </button>
              {isExpanded && (
                <div className="px-3 pb-2 pt-0 ml-5">
                  <pre className="text-[11px] text-gray-400 whitespace-pre-wrap break-words font-mono bg-surface-2 rounded p-2 border border-border">
                    {c.message_body || c.message}
                  </pre>
                  <div className="text-[10px] text-gray-700 mt-1.5">
                    <span className="font-mono">{c.hash}</span>
                    <span className="mx-1">·</span>
                    <span>{c.email}</span>
                    <span className="mx-1">·</span>
                    <span>{new Date(c.timestamp * 1000).toLocaleString()}</span>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function formatRelative(ts: number): string {
  // ts is unix epoch in seconds
  const diff = Math.max(0, Date.now() / 1000 - ts);
  if (diff < 60) return "just now";
  const min = Math.floor(diff / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day}d ago`;
  const month = Math.floor(day / 30);
  if (month < 12) return `${month}mo ago`;
  const year = Math.floor(day / 365);
  return `${year}y ago`;
}
