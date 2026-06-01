// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { GitBranch as GitBranchIcon, ArrowUp, ArrowDown, Circle, History, GitCommitHorizontal, GitPullRequest } from "lucide-react";
import { useGitStore } from "../stores/git";

interface Props {
  cwd: string | null;
  onOpenChanges: () => void;
  onOpenHistory: () => void;
  onOpenRemote?: () => void;
}

export function GitStatusBar({ cwd, onOpenChanges, onOpenHistory, onOpenRemote }: Props) {
  const { status, branches, refreshing, lastRefresh, setCwd, refreshStatus, refreshBranches, checkout } = useGitStore();
  const [branchPickerOpen, setBranchPickerOpen] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Sync cwd with store
  useEffect(() => {
    setCwd(cwd);
  }, [cwd, setCwd]);

  // Initial load + poll while document is visible
  useEffect(() => {
    if (!cwd) return;
    refreshStatus();
    const id = setInterval(() => {
      if (document.visibilityState === "visible") {
        refreshStatus();
      }
    }, 5000);
    return () => clearInterval(id);
  }, [cwd, refreshStatus]);

  // Close dropdown on outside click
  useEffect(() => {
    if (!branchPickerOpen) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setBranchPickerOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [branchPickerOpen]);

  if (!cwd) return null;
  if (status && !status.is_repo) {
    return (
      <div className="flex items-center gap-2 px-3 py-1 border-t border-border bg-surface-1 text-xs text-gray-700 select-none shrink-0">
        <GitBranchIcon size={11} />
        <span>不是 git 仓库</span>
      </div>
    );
  }

  const dirty =
    (status?.staged.length ?? 0) +
    (status?.unstaged.length ?? 0) +
    (status?.untracked.length ?? 0);

  const handleOpenBranchPicker = async () => {
    if (!branchPickerOpen) {
      await refreshBranches();
    }
    setBranchPickerOpen((v) => !v);
    setSwitchError(null);
  };

  const handleCheckout = async (target: string) => {
    setSwitchError(null);
    try {
      await checkout(target);
      setBranchPickerOpen(false);
    } catch (e) {
      setSwitchError(String(e));
    }
  };

  return (
    <div className="flex items-center gap-3 px-3 py-1 border-t border-border bg-surface-1 text-xs text-gray-500 shrink-0 select-none relative">
      {/* Branch */}
      <div className="relative" ref={dropdownRef}>
        <button
          onClick={handleOpenBranchPicker}
          className="flex items-center gap-1 hover:text-gray-300 transition-colors"
          title="切换分支"
        >
          <GitBranchIcon size={11} />
          <span className="truncate max-w-[180px]">{status?.branch ?? "…"}</span>
        </button>
        {branchPickerOpen && (
          <div className="absolute bottom-full mb-1 left-0 z-30 w-64 rounded border border-border bg-surface-2 shadow-2xl py-1 max-h-72 overflow-y-auto">
            {switchError && (
              <div className="px-2 py-1 text-[11px] text-red-400 border-b border-border">
                {switchError}
              </div>
            )}
            {branches.length === 0 && (
              <div className="px-2 py-1 text-[11px] text-gray-600">无分支</div>
            )}
            {branches.map((b) => (
              <button
                key={`${b.is_remote ? "r" : "l"}:${b.name}`}
                onClick={() => handleCheckout(b.name)}
                className={`w-full text-left px-2 py-1 text-xs flex items-center gap-1.5 hover:bg-surface-3 transition-colors ${
                  b.is_current ? "text-accent" : b.is_remote ? "text-gray-600" : "text-gray-300"
                }`}
                title={b.upstream ?? b.name}
              >
                <GitBranchIcon size={10} />
                <span className="flex-1 truncate">{b.name}</span>
                {b.is_current && <span className="text-[10px]">当前</span>}
                {b.is_remote && !b.is_current && <span className="text-[10px]">远程</span>}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Ahead/behind */}
      {status && (status.ahead > 0 || status.behind > 0) && (
        <span className="flex items-center gap-1 text-gray-500" title="领先 / 落后于上游">
          {status.ahead > 0 && (
            <span className="flex items-center gap-0.5">
              <ArrowUp size={10} />
              {status.ahead}
            </span>
          )}
          {status.behind > 0 && (
            <span className="flex items-center gap-0.5">
              <ArrowDown size={10} />
              {status.behind}
            </span>
          )}
        </span>
      )}

      {/* Upstream */}
      {status?.upstream && (
        <span className="text-gray-700 truncate max-w-[160px]" title={status.upstream}>
          {status.upstream}
        </span>
      )}

      {/* Dirty count */}
      <button
        onClick={onOpenChanges}
        className="flex items-center gap-1 hover:text-gray-300 transition-colors"
        title="显示变更"
      >
        {dirty > 0 ? (
          <>
            <Circle size={8} className="fill-red-400 text-red-400" />
            <span>{dirty}</span>
          </>
        ) : (
          <>
            <Circle size={8} className="text-gray-700" />
            <span className="text-gray-700">干净</span>
          </>
        )}
      </button>

      {/* History */}
      <button
        onClick={onOpenHistory}
        className="flex items-center gap-1 hover:text-gray-300 transition-colors"
        title="显示提交历史"
      >
        <History size={11} />
      </button>

      {/* Remote */}
      {onOpenRemote && (
        <button
          onClick={onOpenRemote}
          className="flex items-center gap-1 hover:text-gray-300 transition-colors"
          title="远程仓库（问题与拉取请求）"
        >
          <GitPullRequest size={11} />
          <span className="text-[10px]">远程仓库</span>
        </button>
      )}

      <span className="flex-1" />

      {/* Refreshed at */}
      <span className="text-gray-700 text-[10px] flex items-center gap-1">
        {refreshing ? (
          <GitCommitHorizontal size={10} className="animate-pulse" />
        ) : null}
        {lastRefresh ? formatRelative(lastRefresh) : ""}
      </span>
    </div>
  );
}

function formatRelative(ts: number): string {
  const diff = Math.max(0, Date.now() - ts);
  const sec = Math.floor(diff / 1000);
  if (sec < 5) return "刚刚";
  if (sec < 60) return `${sec} 秒前`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  return `${hr} 小时前`;
}
