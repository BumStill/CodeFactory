// SPDX-License-Identifier: Apache-2.0
import { useEffect } from "react";
import { ArrowDown, ArrowUp, Circle, GitBranch } from "lucide-react";
import { useGitStore } from "../stores/git";

interface Props {
  cwd: string | null;
  onOpenChanges: () => void;
  detailsId?: string;
  detailsOpen?: boolean;
}

/**
 * One readable local-worktree summary. Remote delivery is deliberately kept in
 * WorkspaceDeliveryStatus so returning to main never erases the session PR.
 */
export function GitStatusBar({ cwd, onOpenChanges, detailsId, detailsOpen = false }: Props) {
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
    return (
      <button
        type="button"
        aria-label="本地 Git；当前目录不是 Git 仓库"
        aria-controls={detailsId}
        aria-expanded={detailsOpen}
        data-status-tone="neutral"
        onClick={onOpenChanges}
        className="inline-flex h-11 w-11 items-center justify-center rounded-lg text-gray-600 hover:bg-surface-3 hover:text-gray-300 lg:h-9 lg:w-9"
        title="当前目录不是 Git 仓库"
      >
        <GitBranch size={14} aria-hidden="true" />
      </button>
    );
  }

  const dirty = (status?.staged.length ?? 0) + (status?.unstaged.length ?? 0) + (status?.untracked.length ?? 0);
  const branch = status?.branch ?? "…";
  const syncLabel = !status ? "正在读取" : status.ahead === 0 && status.behind === 0 ? "已同步" : null;
  const statusTone = dirty > 0 || (status?.behind ?? 0) > 0 ? "warning" : "neutral";
  const syncStatus = syncLabel ?? [status?.ahead ? `领先 ${status.ahead}` : null, status?.behind ? `落后 ${status.behind}` : null].filter(Boolean).join("；");
  const accessibleStatus = !status
    ? "本地 Git；正在读取"
    : dirty > 0
      ? `本地 Git；分支 ${branch}；${dirty} 个本地变更；${syncStatus}`
      : `本地 Git；分支 ${branch}；${syncStatus}；无本地变更`;

  return (
    <button
      type="button"
      aria-label={accessibleStatus}
      aria-controls={detailsId}
      aria-expanded={detailsOpen}
      data-status-tone={statusTone}
      onClick={onOpenChanges}
      className={`inline-flex h-11 shrink-0 items-center justify-center gap-1 rounded-lg px-2 text-caption transition-colors hover:bg-surface-3 lg:h-9 ${
        statusTone === "warning" ? "text-status-warning" : "text-gray-500 hover:text-gray-200"
      }`}
      title={accessibleStatus}
    >
      <GitBranch size={14} aria-hidden="true" className={refreshing ? "animate-pulse motion-reduce:animate-none" : ""} />
      {dirty > 0 && (
        <span className="inline-flex items-center gap-1 whitespace-nowrap tabular-nums" aria-hidden="true">
          <Circle size={6} className="fill-current" />
          {dirty}
        </span>
      )}
      {status && status.ahead > 0 && <span aria-hidden="true" className="inline-flex items-center gap-0.5 whitespace-nowrap"><ArrowUp size={10} />{status.ahead}</span>}
      {status && status.behind > 0 && <span aria-hidden="true" className="inline-flex items-center gap-0.5 whitespace-nowrap"><ArrowDown size={10} />落后 {status.behind}</span>}
    </button>
  );
}
