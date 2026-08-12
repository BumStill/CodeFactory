// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { X, FileText, Plus, Check, RefreshCw, GitCommit as GitCommitIcon, History, GitPullRequest } from "lucide-react";
import { useGitStore } from "../stores/git";
import type { FileChange } from "../lib/tauri";
import { DiffViewer } from "./DiffViewer";
import { CheckpointsPanel } from "./CheckpointsPanel";

interface Props {
  onClose: () => void;
  onOpenHistory: () => void;
  onOpenRemote: () => void;
  sessionId: string | null;
  embedded?: boolean;
}

type Group = "staged" | "unstaged" | "untracked";

interface Row {
  path: string;
  status: string;
  group: Group;
}

export function GitChangesPanel({ onClose, onOpenHistory, onOpenRemote, sessionId, embedded = false }: Props) {
  const { status, branches, refreshStatus, refreshBranches, checkout, stageFiles, commit, getFileDiff } = useGitStore();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [activeFile, setActiveFile] = useState<{ path: string; staged: boolean } | null>(null);
  const [diff, setDiff] = useState<string>("");
  const [diffLoading, setDiffLoading] = useState(false);
  const [showCommitModal, setShowCommitModal] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [committing, setCommitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [switchingBranch, setSwitchingBranch] = useState(false);
  const [isNarrowEmbedded, setIsNarrowEmbedded] = useState(embedded);
  const panelRef = useRef<HTMLDivElement>(null);
  const commitTriggerRef = useRef<HTMLButtonElement>(null);
  const commitMessageRef = useRef<HTMLTextAreaElement>(null);

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
    if (!showCommitModal) return;
    const returnTarget = commitTriggerRef.current;
    const frame = window.requestAnimationFrame(() => commitMessageRef.current?.focus());
    return () => {
      window.cancelAnimationFrame(frame);
      if (returnTarget?.isConnected && !returnTarget.disabled) returnTarget.focus();
    };
  }, [showCommitModal]);

  useEffect(() => {
    void Promise.all([refreshStatus(), refreshBranches()]);
  }, [refreshBranches, refreshStatus]);

  const rows: Row[] = useMemo(() => {
    if (!status) return [];
    const r: Row[] = [];
    for (const f of status.staged) r.push({ path: f.path, status: f.status, group: "staged" });
    for (const f of status.unstaged) r.push({ path: f.path, status: f.status, group: "unstaged" });
    for (const p of status.untracked) r.push({ path: p, status: "added", group: "untracked" });
    return r;
  }, [status]);

  // Load diff when active file changes
  useEffect(() => {
    if (!activeFile) {
      setDiff("");
      return;
    }
    let cancelled = false;
    setDiffLoading(true);
    getFileDiff(activeFile.path, activeFile.staged)
      .then((d) => {
        if (!cancelled) setDiff(d);
      })
      .catch((e) => {
        if (!cancelled) setDiff(`Error loading diff: ${String(e)}`);
      })
      .finally(() => {
        if (!cancelled) setDiffLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [activeFile, getFileDiff]);

  const toggleSelect = (path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const handleStageAll = async () => {
    setError(null);
    try {
      await stageFiles(["."]);
      setSelected(new Set());
    } catch (e) {
      setError(String(e));
    }
  };

  const handleStageSelected = async () => {
    setError(null);
    if (selected.size === 0) return;
    try {
      await stageFiles(Array.from(selected));
      setSelected(new Set());
    } catch (e) {
      setError(String(e));
    }
  };

  const handleCommit = async () => {
    if (!commitMsg.trim()) return;
    setCommitting(true);
    setError(null);
    try {
      await commit(commitMsg);
      setCommitMsg("");
      setShowCommitModal(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setCommitting(false);
    }
  };

  const stagedCount = status?.staged.length ?? 0;

  const handleBranchChange = async (target: string) => {
    if (!target || target === status?.branch) return;
    setSwitchingBranch(true);
    setError(null);
    try { await checkout(target); }
    catch (cause) { setError(String(cause)); }
    finally { setSwitchingBranch(false); }
  };

  return (
    <div
      ref={panelRef}
      data-embedded-layout={embedded ? (isNarrowEmbedded ? "stacked" : "split") : undefined}
      className={embedded
        ? "relative flex min-h-0 h-full w-full flex-col overflow-hidden bg-surface-1"
        : "fixed right-0 top-0 bottom-0 z-40 w-[640px] max-w-[80vw] bg-surface-1 border-l border-border shadow-2xl flex flex-col"}
    >
      {/* Header */}
      <div
        data-testid="git-changes-header"
        className={embedded
          ? `flex items-center gap-2 border-b border-border px-3 py-2 shrink-0 ${isNarrowEmbedded ? "flex-wrap" : "flex-nowrap"}`
          : "flex items-center gap-2 px-3 py-2 border-b border-border shrink-0"}
      >
        <FileText size={14} className="text-gray-400" />
        <span className="text-label font-semibold text-gray-200 flex-1">本地 Git</span>
        <button
          onClick={onOpenHistory}
          className={`inline-flex items-center gap-1 rounded px-2 text-caption text-gray-500 hover:bg-surface-3 hover:text-gray-200 ${embedded && isNarrowEmbedded ? "h-11" : "h-9"}`}
          title="提交历史"
        ><History size={11} />历史</button>
        <button
          onClick={onOpenRemote}
          className={`inline-flex items-center gap-1 rounded px-2 text-caption text-gray-500 hover:bg-surface-3 hover:text-gray-200 ${embedded && isNarrowEmbedded ? "h-11" : "h-9"}`}
          title="远程仓库（问题与拉取请求）"
        ><GitPullRequest size={11} />远程</button>
        <button
          onClick={() => refreshStatus()}
          aria-label="刷新本地 Git"
          className={`inline-flex shrink-0 items-center justify-center rounded text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-300 ${embedded && isNarrowEmbedded ? "h-11 w-11" : "h-9 w-9"}`}
          title="刷新"
        >
          <RefreshCw size={12} />
        </button>
        <button
          onClick={onClose}
          aria-label={embedded ? "关闭本地 Git" : "关闭"}
          data-auxiliary-initial-focus={embedded ? true : undefined}
          className={embedded
            ? `inline-flex shrink-0 items-center justify-center rounded text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-300 ${isNarrowEmbedded ? "h-11 w-11" : "h-9 w-9"}`
            : "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-300"}
          title={embedded ? "关闭本地 Git" : "关闭"}
        >
          <X size={14} />
        </button>
      </div>

      {/* Action bar */}
      <div
        data-testid="git-changes-actions"
        className={embedded
          ? `flex items-center gap-2 border-b border-border bg-surface-2 px-3 py-1.5 shrink-0 ${isNarrowEmbedded ? "flex-wrap" : "flex-nowrap"}`
          : "flex items-center gap-2 px-3 py-1.5 border-b border-border bg-surface-2 shrink-0"}
      >
        <label className="inline-flex min-w-0 max-w-full items-center gap-1 text-caption text-gray-600">
          分支
          <select
            aria-label="切换本地分支"
            value={status?.branch ?? ""}
            disabled={switchingBranch}
            onChange={(event) => void handleBranchChange(event.target.value)}
            className={`w-40 max-w-full rounded border border-border bg-surface-3 px-1.5 text-caption text-gray-300 outline-none focus:border-accent ${embedded && isNarrowEmbedded ? "h-11" : "h-9"}`}
          >
            {branches.filter((branch) => !branch.is_remote || branch.is_current).map((branch) => (
              <option key={`${branch.is_remote ? "r" : "l"}:${branch.name}`} value={branch.name}>{branch.name}</option>
            ))}
          </select>
        </label>
        <button
          onClick={handleStageAll}
          disabled={rows.length === 0}
          className={`${embedded && isNarrowEmbedded ? "h-11" : "h-9"} px-2 text-caption rounded bg-surface-3 hover:bg-surface-4 text-gray-300 disabled:opacity-40 disabled:cursor-not-allowed transition-colors`}
        >
          暂存全部
        </button>
        <button
          onClick={handleStageSelected}
          disabled={selected.size === 0}
          className={`${embedded && isNarrowEmbedded ? "h-11" : "h-9"} px-2 text-caption rounded bg-surface-3 hover:bg-surface-4 text-gray-300 disabled:opacity-40 disabled:cursor-not-allowed transition-colors`}
        >
          暂存选中 ({selected.size})
        </button>
        <span className="flex-1" />
        <CheckpointsPanel sessionId={sessionId} embedded={embedded} narrow={isNarrowEmbedded} />
        <button
          ref={commitTriggerRef}
          onClick={() => setShowCommitModal(true)}
          disabled={stagedCount === 0}
          className={`${embedded && isNarrowEmbedded ? "h-11" : "h-9"} flex items-center gap-1 px-2 text-caption rounded bg-accent hover:bg-accent-hover text-white disabled:opacity-40 disabled:cursor-not-allowed transition-colors`}
        >
          <GitCommitIcon size={11} />
          提交 ({stagedCount})
        </button>
      </div>

      {error && (
        <div className="px-3 py-1.5 text-caption text-status-danger border-b border-border bg-status-danger-soft shrink-0">
          {error}
        </div>
      )}

      {/* Body: list + diff */}
      <div
        data-testid="git-changes-body"
        className={embedded
          ? `flex min-h-0 flex-1 ${isNarrowEmbedded ? "flex-col" : "flex-row"}`
          : "flex-1 flex min-h-0"}
      >
        {/* Files list */}
        <div
          data-testid="git-changes-file-list"
          className={embedded
            ? `shrink-0 overflow-y-auto ${isNarrowEmbedded ? "max-h-[42%] w-full border-b border-border" : "max-h-none w-[260px] border-r border-border"}`
            : "w-[260px] border-r border-border overflow-y-auto shrink-0"}
        >
          {rows.length === 0 && (
            <div className="px-3 py-4 text-center text-caption text-gray-600">
              无变更
            </div>
          )}
          <FileGroup
            label="已暂存"
            files={status?.staged ?? []}
            group="staged"
            selected={selected}
            activeFile={activeFile}
            largeTarget={embedded && isNarrowEmbedded}
            onToggle={toggleSelect}
            onSelect={(path, staged) => setActiveFile({ path, staged })}
          />
          <FileGroup
            label="未暂存"
            files={status?.unstaged ?? []}
            group="unstaged"
            selected={selected}
            activeFile={activeFile}
            largeTarget={embedded && isNarrowEmbedded}
            onToggle={toggleSelect}
            onSelect={(path, staged) => setActiveFile({ path, staged })}
          />
          {status && status.untracked.length > 0 && (
            <div>
              <div className="px-2 py-1 text-caption font-semibold text-gray-600 bg-surface-2 sticky top-0">
                未跟踪 ({status.untracked.length})
              </div>
              {status.untracked.map((p) => (
                <FileRow
                  key={`u-${p}`}
                  path={p}
                  status="added"
                  group="untracked"
                  selected={selected.has(p)}
                  active={activeFile?.path === p && !activeFile.staged}
                  largeTarget={embedded && isNarrowEmbedded}
                  onToggle={() => toggleSelect(p)}
                  onClick={() => setActiveFile({ path: p, staged: false })}
                />
              ))}
            </div>
          )}
        </div>

        {/* Diff view */}
        <div className="flex-1 overflow-y-auto p-2 min-w-0">
          {!activeFile && (
            <div className="pt-8 text-center text-caption text-gray-600">
              选择文件以查看差异
            </div>
          )}
          {activeFile && diffLoading && (
            <div className="text-caption text-gray-600 text-center pt-8">加载中…</div>
          )}
          {activeFile && !diffLoading && (
            <>
              <div className="text-caption text-gray-500 mb-2 truncate">
                {activeFile.path}{" "}
                <span className="text-gray-600">({activeFile.staged ? "已暂存" : "未暂存"})</span>
              </div>
              {diff.trim().length === 0 ? (
                <div className="text-caption text-gray-600">
                  无可用差异（文件可能为二进制或完全新增）。
                </div>
              ) : (
                <DiffViewer output={diff} />
              )}
            </>
          )}
        </div>
      </div>

      {/* Commit modal */}
      {showCommitModal && (
        <div
          className={`${embedded ? "absolute" : "fixed"} inset-0 z-50 flex items-center justify-center bg-black/60 p-3`}
          onClick={() => !committing && setShowCommitModal(false)}
        >
          <div
            role="dialog"
            aria-modal={embedded ? "false" : "true"}
            aria-labelledby="git-commit-dialog-title"
            className="w-[480px] max-w-full rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(event) => trapDialogKeyboard(event, () => {
              if (!committing) setShowCommitModal(false);
            })}
          >
            <h2 id="git-commit-dialog-title" className="text-body font-semibold text-gray-200 flex items-center gap-2">
              <GitCommitIcon size={14} /> 提交 {stagedCount} 个文件
            </h2>
            <textarea
              ref={commitMessageRef}
              value={commitMsg}
              onChange={(e) => setCommitMsg(e.target.value)}
              placeholder="提交信息…"
              rows={5}
              className="w-full bg-surface-3 border border-border rounded px-3 py-2 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none font-mono"
              autoFocus
            />
            {error && <div className="text-caption text-status-danger">{error}</div>}
            <div className="flex justify-end gap-2">
              <button
                disabled={committing}
                onClick={() => setShowCommitModal(false)}
                className={`${embedded && isNarrowEmbedded ? "h-11" : "h-9"} px-3 rounded text-label text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors disabled:opacity-40`}
              >
                取消
              </button>
              <button
                disabled={committing || !commitMsg.trim()}
                onClick={handleCommit}
                className={`${embedded && isNarrowEmbedded ? "h-11" : "h-9"} flex items-center gap-1 px-3 rounded text-label bg-accent hover:bg-accent-hover text-white transition-colors disabled:opacity-40 disabled:cursor-not-allowed`}
              >
                <Check size={12} />
                {committing ? "提交中…" : "提交"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

interface GroupProps {
  label: string;
  files: FileChange[];
  group: Group;
  selected: Set<string>;
  activeFile: { path: string; staged: boolean } | null;
  largeTarget: boolean;
  onToggle: (path: string) => void;
  onSelect: (path: string, staged: boolean) => void;
}

function FileGroup({ label, files, group, selected, activeFile, largeTarget, onToggle, onSelect }: GroupProps) {
  if (files.length === 0) return null;
  const staged = group === "staged";
  return (
    <div>
      <div className="px-2 py-1 text-caption font-semibold text-gray-600 bg-surface-2 sticky top-0">
        {label} ({files.length})
      </div>
      {files.map((f) => (
        <FileRow
          key={`${group}-${f.path}`}
          path={f.path}
          status={f.status}
          group={group}
          selected={selected.has(f.path)}
          active={activeFile?.path === f.path && activeFile.staged === staged}
          largeTarget={largeTarget}
          onToggle={() => onToggle(f.path)}
          onClick={() => onSelect(f.path, staged)}
        />
      ))}
    </div>
  );
}

interface RowProps {
  path: string;
  status: string;
  group: Group;
  selected: boolean;
  active: boolean;
  largeTarget: boolean;
  onToggle: () => void;
  onClick: () => void;
}

function FileRow({ path, status, group, selected, active, largeTarget, onToggle, onClick }: RowProps) {
  const statusBadge = badge(status);
  return (
    <div className={`flex items-center gap-1 px-2 transition-colors ${
        active ? "bg-surface-3" : "hover:bg-surface-2"
      }`}>
      {group !== "staged" && (
        <label className={`inline-flex shrink-0 cursor-pointer items-center justify-center ${largeTarget ? "h-11 w-11" : "h-9 w-9"}`}>
          <input
            type="checkbox"
            checked={selected}
            onChange={onToggle}
            className="shrink-0"
            title="选择暂存"
            aria-label={`选择 ${path} 暂存`}
          />
        </label>
      )}
      <button
        type="button"
        onClick={onClick}
        aria-label={`查看 ${path} 差异`}
        aria-current={active ? "true" : undefined}
        className={`flex min-w-0 flex-1 items-center gap-1 rounded text-left outline-none focus-visible:ring-2 focus-visible:ring-accent/70 ${largeTarget ? "min-h-11" : "min-h-9"}`}
      >
        {group === "staged" && <Plus size={10} className="text-accent shrink-0" />}
        <span
          className={`text-caption font-mono shrink-0 w-3 text-center ${statusBadge.color}`}
          title={status}
        >
          {statusBadge.letter}
        </span>
        <span className="text-caption text-gray-300 truncate flex-1" title={path}>
          {path}
        </span>
      </button>
    </div>
  );
}

function trapDialogKeyboard(event: ReactKeyboardEvent<HTMLElement>, onEscape: () => void) {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    onEscape();
    return;
  }
  if (event.key !== "Tab") return;

  const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(
    'button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
  )).filter((element) => !element.hasAttribute("hidden"));
  if (focusable.length === 0) {
    event.preventDefault();
    return;
  }
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function badge(status: string): { letter: string; color: string } {
  switch (status) {
    case "modified":
      return { letter: "M", color: "text-accent" };
    case "added":
      return { letter: "A", color: "text-status-info" };
    case "deleted":
      return { letter: "D", color: "text-gray-500" };
    case "renamed":
      return { letter: "R", color: "text-status-info" };
    case "typechange":
      return { letter: "T", color: "text-accent" };
    default:
      return { letter: "?", color: "text-gray-500" };
  }
}
