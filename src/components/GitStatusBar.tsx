// SPDX-License-Identifier: Apache-2.0
import { useEffect } from "react";
import { ArrowDown, ArrowUp, Circle, GitBranch } from "lucide-react";
import { useGitStore } from "../stores/git";

interface Props {
  cwd: string | null;
  onOpenChanges: () => void;
}

/**
 * One readable local-worktree summary. Remote delivery is deliberately kept in
 * WorkspaceDeliveryStatus so returning to main never erases the session PR.
 */
export function GitStatusBar({ cwd, onOpenChanges }: Props) {
  const { status, refreshing, setCwd, refreshStatus } = useGitStore();

  useEffect(() => { setCwd(cwd); }, [cwd, setCwd]);
  useEffect(() => {
    if (!cwd) return;
    void refreshStatus();
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void refreshStatus();
    }, 5000);
    return () => window.clearInterval(id);
  }, [cwd, refreshStatus]);

  if (!cwd) return null;
  if (status && !status.is_repo) {
    return <span className="text-[11px] text-gray-600">不是 Git 仓库</span>;
  }

  const dirty = (status?.staged.length ?? 0) + (status?.unstaged.length ?? 0) + (status?.untracked.length ?? 0);
  const branch = status?.branch ?? "…";
  const syncLabel = !status
    ? "正在读取"
    : status.ahead === 0 && status.behind === 0
      ? "已同步"
      : null;

  return (
    <button
      type="button"
      aria-label="本地工作树"
      onClick={onOpenChanges}
      className="inline-flex h-7 max-w-[270px] shrink-0 items-center gap-1.5 rounded-md border border-border bg-surface-2 px-2 text-[11px] text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
      title="查看本地变更、提交历史、分支和恢复点"
    >
      <GitBranch size={12} className={refreshing ? "animate-pulse" : ""} />
      <span className="max-w-[110px] truncate font-medium text-gray-300">{branch}</span><span aria-hidden="true"> · </span>
      {dirty > 0 && (
        <span className="inline-flex items-center gap-1 whitespace-nowrap">
          <Circle size={7} className="fill-amber-500 text-amber-500" />
          {dirty} 个本地变更
        </span>
      )}
      {syncLabel && <span className="whitespace-nowrap text-gray-600">{syncLabel}</span>}
      {status && status.ahead > 0 && <span className="inline-flex items-center gap-0.5 whitespace-nowrap"><ArrowUp size={10} />领先 {status.ahead}</span>}
      {status && status.behind > 0 && <span className="inline-flex items-center gap-0.5 whitespace-nowrap text-amber-600 dark:text-amber-400"><ArrowDown size={10} />落后 {status.behind}</span>}
    </button>
  );
}
