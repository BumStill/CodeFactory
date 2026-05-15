// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useState } from "react";
import { X, FileText, Plus, Check, RefreshCw, GitCommit as GitCommitIcon } from "lucide-react";
import { useGitStore } from "../stores/git";
import type { FileChange } from "../lib/tauri";
import { DiffViewer } from "./DiffViewer";

interface Props {
  onClose: () => void;
}

type Group = "staged" | "unstaged" | "untracked";

interface Row {
  path: string;
  status: string;
  group: Group;
}

export function GitChangesPanel({ onClose }: Props) {
  const { status, refreshStatus, stageFiles, commit, getFileDiff } = useGitStore();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [activeFile, setActiveFile] = useState<{ path: string; staged: boolean } | null>(null);
  const [diff, setDiff] = useState<string>("");
  const [diffLoading, setDiffLoading] = useState(false);
  const [showCommitModal, setShowCommitModal] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [committing, setCommitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

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

  return (
    <div className="fixed right-0 top-0 bottom-0 z-40 w-[640px] max-w-[80vw] bg-surface-1 border-l border-border shadow-2xl flex flex-col">
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <FileText size={14} className="text-gray-400" />
        <span className="text-xs font-semibold text-gray-200 flex-1">Changes</span>
        <button
          onClick={() => refreshStatus()}
          className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors"
          title="Refresh"
        >
          <RefreshCw size={12} />
        </button>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors"
          title="Close"
        >
          <X size={14} />
        </button>
      </div>

      {/* Action bar */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-border bg-surface-2 shrink-0">
        <button
          onClick={handleStageAll}
          disabled={rows.length === 0}
          className="px-2 py-1 text-[11px] rounded bg-surface-3 hover:bg-surface-4 text-gray-300 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          Stage all
        </button>
        <button
          onClick={handleStageSelected}
          disabled={selected.size === 0}
          className="px-2 py-1 text-[11px] rounded bg-surface-3 hover:bg-surface-4 text-gray-300 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          Stage selected ({selected.size})
        </button>
        <span className="flex-1" />
        <button
          onClick={() => setShowCommitModal(true)}
          disabled={stagedCount === 0}
          className="flex items-center gap-1 px-2 py-1 text-[11px] rounded bg-accent hover:bg-accent-hover text-white disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          <GitCommitIcon size={11} />
          Commit ({stagedCount})
        </button>
      </div>

      {error && (
        <div className="px-3 py-1.5 text-[11px] text-red-400 border-b border-border bg-red-950/20 shrink-0">
          {error}
        </div>
      )}

      {/* Body: list + diff */}
      <div className="flex-1 flex min-h-0">
        {/* Files list */}
        <div className="w-[260px] border-r border-border overflow-y-auto shrink-0">
          {rows.length === 0 && (
            <div className="px-3 py-4 text-[11px] text-gray-700 text-center">
              No changes
            </div>
          )}
          <FileGroup
            label="Staged"
            files={status?.staged ?? []}
            group="staged"
            selected={selected}
            activeFile={activeFile}
            onToggle={toggleSelect}
            onSelect={(path, staged) => setActiveFile({ path, staged })}
          />
          <FileGroup
            label="Unstaged"
            files={status?.unstaged ?? []}
            group="unstaged"
            selected={selected}
            activeFile={activeFile}
            onToggle={toggleSelect}
            onSelect={(path, staged) => setActiveFile({ path, staged })}
          />
          {status && status.untracked.length > 0 && (
            <div>
              <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-gray-600 bg-surface-2 sticky top-0">
                Untracked ({status.untracked.length})
              </div>
              {status.untracked.map((p) => (
                <FileRow
                  key={`u-${p}`}
                  path={p}
                  status="added"
                  group="untracked"
                  selected={selected.has(p)}
                  active={activeFile?.path === p && !activeFile.staged}
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
            <div className="text-[11px] text-gray-700 text-center pt-8">
              Select a file to view diff
            </div>
          )}
          {activeFile && diffLoading && (
            <div className="text-[11px] text-gray-600 text-center pt-8">Loading…</div>
          )}
          {activeFile && !diffLoading && (
            <>
              <div className="text-[11px] text-gray-500 mb-2 truncate">
                {activeFile.path}{" "}
                <span className="text-gray-700">({activeFile.staged ? "staged" : "unstaged"})</span>
              </div>
              {diff.trim().length === 0 ? (
                <div className="text-[11px] text-gray-700">
                  No diff available (file may be binary or fully added).
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
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
          onClick={() => !committing && setShowCommitModal(false)}
        >
          <div
            className="w-[480px] rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4"
            onClick={(e) => e.stopPropagation()}
          >
            <h2 className="text-sm font-semibold text-gray-200 flex items-center gap-2">
              <GitCommitIcon size={14} /> Commit {stagedCount} file{stagedCount === 1 ? "" : "s"}
            </h2>
            <textarea
              value={commitMsg}
              onChange={(e) => setCommitMsg(e.target.value)}
              placeholder="Commit message…"
              rows={5}
              className="w-full bg-surface-3 border border-border rounded px-3 py-2 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none font-mono"
              autoFocus
            />
            {error && <div className="text-[11px] text-red-400">{error}</div>}
            <div className="flex justify-end gap-2">
              <button
                disabled={committing}
                onClick={() => setShowCommitModal(false)}
                className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors disabled:opacity-40"
              >
                Cancel
              </button>
              <button
                disabled={committing || !commitMsg.trim()}
                onClick={handleCommit}
                className="flex items-center gap-1 px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              >
                <Check size={12} />
                {committing ? "Committing…" : "Commit"}
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
  onToggle: (path: string) => void;
  onSelect: (path: string, staged: boolean) => void;
}

function FileGroup({ label, files, group, selected, activeFile, onToggle, onSelect }: GroupProps) {
  if (files.length === 0) return null;
  const staged = group === "staged";
  return (
    <div>
      <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-gray-600 bg-surface-2 sticky top-0">
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
  onToggle: () => void;
  onClick: () => void;
}

function FileRow({ path, status, group, selected, active, onToggle, onClick }: RowProps) {
  const statusBadge = badge(status);
  return (
    <div
      className={`flex items-center gap-1 px-2 py-1 cursor-pointer transition-colors ${
        active ? "bg-surface-3" : "hover:bg-surface-2"
      }`}
      onClick={onClick}
    >
      {group !== "staged" && (
        <input
          type="checkbox"
          checked={selected}
          onChange={onToggle}
          onClick={(e) => e.stopPropagation()}
          className="shrink-0"
          title="Select for staging"
        />
      )}
      {group === "staged" && <Plus size={10} className="text-green-400 shrink-0" />}
      <span
        className={`text-[10px] font-mono shrink-0 w-3 text-center ${statusBadge.color}`}
        title={status}
      >
        {statusBadge.letter}
      </span>
      <span className="text-[11px] text-gray-300 truncate flex-1" title={path}>
        {path}
      </span>
    </div>
  );
}

function badge(status: string): { letter: string; color: string } {
  switch (status) {
    case "modified":
      return { letter: "M", color: "text-yellow-400" };
    case "added":
      return { letter: "A", color: "text-green-400" };
    case "deleted":
      return { letter: "D", color: "text-red-400" };
    case "renamed":
      return { letter: "R", color: "text-blue-400" };
    case "typechange":
      return { letter: "T", color: "text-purple-400" };
    default:
      return { letter: "?", color: "text-gray-500" };
  }
}
