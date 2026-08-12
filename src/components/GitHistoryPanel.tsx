// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { X, History, RefreshCw, ChevronLeft, ChevronRight, ChevronDown } from "lucide-react";
import { useGitStore } from "../stores/git";

interface Props {
  onClose: () => void;
  embedded?: boolean;
}

export function GitHistoryPanel({ onClose, embedded = false }: Props) {
  const { commits, refreshCommits } = useGitStore();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [isNarrowEmbedded, setIsNarrowEmbedded] = useState(embedded);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!embedded) {
      setIsNarrowEmbedded(false);
      return;
    }
    const panel = panelRef.current;
    if (!panel) return;
    const update = () => setIsNarrowEmbedded(panel.getBoundingClientRect().width < 640);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(panel);
    return () => observer.disconnect();
  }, [embedded]);

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
    <div
      ref={panelRef}
      data-embedded-layout={embedded ? (isNarrowEmbedded ? "narrow" : "wide") : undefined}
      className={embedded
        ? "flex min-h-0 h-full w-full flex-col overflow-hidden bg-surface-1"
        : "fixed right-0 top-0 bottom-0 z-40 w-[480px] max-w-[70vw] bg-surface-1 border-l border-border shadow-2xl flex flex-col"}
    >
      {/* Header */}
      <div className={`flex items-center gap-2 px-3 py-2 border-b border-border shrink-0 ${embedded && isNarrowEmbedded ? "flex-wrap" : "flex-nowrap"}`}>
        <History size={14} className="text-gray-400" />
        <span className="text-label font-semibold text-gray-200 flex-1">历史记录</span>
        <button
          onClick={handleRefresh}
          aria-label="刷新提交历史"
          className={`inline-flex shrink-0 items-center justify-center rounded text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-300 ${embedded && isNarrowEmbedded ? "h-11 w-11" : "h-9 w-9"}`}
          title="刷新"
        >
          <RefreshCw size={14} className={loading ? "animate-spin motion-reduce:animate-none" : ""} />
        </button>
        <button
          onClick={onClose}
          aria-label={embedded ? "返回本地 Git" : "关闭"}
          data-auxiliary-initial-focus={embedded ? true : undefined}
          className={embedded
            ? `inline-flex shrink-0 items-center justify-center rounded text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-300 ${isNarrowEmbedded ? "h-11 w-11" : "h-9 w-9"}`
            : "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-300"}
          title={embedded ? "返回本地 Git" : "关闭"}
        >
          {embedded ? <ChevronLeft size={14} /> : <X size={14} />}
        </button>
      </div>

      <div className="flex-1 overflow-y-auto">
        {commits.length === 0 && !loading && (
          <div className="pt-8 text-center text-caption text-gray-600">无提交</div>
        )}
        {loading && commits.length === 0 && (
          <div className="text-caption text-gray-600 text-center pt-8">加载中…</div>
        )}
        {commits.map((c) => {
          const isExpanded = expanded.has(c.hash);
          const commitId = `git-history-commit-${c.hash}`;
          const detailsId = `git-history-details-${c.hash}`;
          return (
            <div key={c.hash} className="border-b border-border">
              <button
                id={commitId}
                onClick={() => toggle(c.hash)}
                aria-label={`${c.short_hash} ${c.message}`}
                aria-expanded={isExpanded}
                aria-controls={detailsId}
                className="w-full text-left px-3 py-2 hover:bg-surface-2 transition-colors flex items-start gap-2"
              >
                {isExpanded ? (
                  <ChevronDown size={14} className="text-gray-600 shrink-0 mt-0.5" />
                ) : (
                  <ChevronRight size={14} className="text-gray-600 shrink-0 mt-0.5" />
                )}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 text-caption">
                    <span className="shrink-0 font-mono text-accent">{c.short_hash}</span>
                    <span className="text-gray-200 truncate flex-1">{c.message}</span>
                  </div>
                  <div className="flex items-center gap-2 text-caption text-gray-600 mt-0.5">
                    <span className="truncate">{c.author}</span>
                    <span className="text-gray-600">·</span>
                    <span>{formatRelative(c.timestamp)}</span>
                  </div>
                </div>
              </button>
              <div
                id={detailsId}
                role="region"
                aria-labelledby={commitId}
                hidden={!isExpanded}
                className="px-3 pb-2 pt-0 ml-5"
              >
                <pre className="text-caption text-gray-400 whitespace-pre-wrap break-words font-mono bg-surface-2 rounded p-2 border border-border">
                  {c.message_body || c.message}
                </pre>
                <div className="mt-1.5 text-caption text-gray-600">
                  <span className="font-mono">{c.hash}</span>
                  <span className="mx-1">·</span>
                  <span>{c.email}</span>
                  <span className="mx-1">·</span>
                  <span>{new Date(c.timestamp * 1000).toLocaleString()}</span>
                </div>
              </div>
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
  if (diff < 60) return "刚刚";
  const min = Math.floor(diff / 60);
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day} 天前`;
  const month = Math.floor(day / 30);
  if (month < 12) return `${month} 个月前`;
  const year = Math.floor(day / 365);
  return `${year} 年前`;
}
