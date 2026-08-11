// SPDX-License-Identifier: Apache-2.0
//
// Compact checkpoint trigger + on-demand drawer. The backend still captures
// every snapshot; the UI deduplicates identical SHAs and prioritises snapshots
// that would actually change files when restored.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  Check,
  FileEdit,
  FileMinus,
  FilePlus,
  FileText,
  GitBranch,
  History,
  RotateCcw,
  X,
} from "lucide-react";
import { invoke } from "../lib/tauri";
import type { CheckpointFileChange, CheckpointInfo } from "../lib/tauri";

interface Props {
  sessionId: string | null;
  /** Render inside the owning Git auxiliary pane instead of opening a second drawer. */
  embedded?: boolean;
  /** Whether the owning embedded pane is narrower than 640px. */
  narrow?: boolean;
}

const RECENT_LIMIT = 3;
const CANDIDATE_LIMIT = 12;

export function CheckpointsPanel({ sessionId, embedded = false, narrow = embedded }: Props) {
  const [checkpoints, setCheckpoints] = useState<CheckpointInfo[]>([]);
  const [changes, setChanges] = useState<Record<string, CheckpointFileChange[]>>({});
  const [open, setOpen] = useState(false);
  const [showAllChanged, setShowAllChanged] = useState(false);
  const [showUnchanged, setShowUnchanged] = useState(false);
  const [confirming, setConfirming] = useState<CheckpointInfo | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const drawerCloseRef = useRef<HTMLButtonElement>(null);

  const refresh = useCallback(async () => {
    if (!sessionId) {
      setCheckpoints([]);
      setChanges({});
      return;
    }
    try {
      const list = await invoke<CheckpointInfo[]>("list_checkpoints", { sessionId });
      // Recovery is an emergency affordance, not a full snapshot archive.
      // Bound diff work so old conversations cannot fan out dozens of git calls.
      const unique = dedupeBySnapshot(list).slice(0, CANDIDATE_LIMIT);
      const results = await Promise.all(unique.map(async (checkpoint) => {
        try {
          const files = await invoke<CheckpointFileChange[]>("checkpoint_changeset", { checkpointId: checkpoint.id });
          return [checkpoint.id, files] as const;
        } catch {
          return [checkpoint.id, [{ path: "", status: "modified" } as CheckpointFileChange]] as const;
        }
      }));
      setCheckpoints(unique);
      setChanges(Object.fromEntries(results));
    } catch {
      setCheckpoints([]);
      setChanges({});
    }
  }, [sessionId]);

  useEffect(() => { void refresh(); }, [refresh]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    listen<string>("checkpoint-created", (event) => {
      if (!cancelled && event.payload === sessionId) void refresh();
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [sessionId, refresh]);

  useEffect(() => {
    if (!open) return;
    const returnTarget = triggerRef.current;
    drawerCloseRef.current?.focus();
    return () => {
      if (returnTarget?.isConnected) returnTarget.focus();
    };
  }, [open]);


  const changed = useMemo(
    () => checkpoints.filter((checkpoint) => (changes[checkpoint.id]?.length ?? 0) > 0),
    [checkpoints, changes],
  );
  const unchanged = useMemo(
    () => checkpoints.filter((checkpoint) => changes[checkpoint.id]?.length === 0),
    [checkpoints, changes],
  );
  const visibleChanged = showAllChanged ? changed : changed.slice(0, RECENT_LIMIT);
  const changesLoaded = checkpoints.length > 0 && checkpoints.every((checkpoint) => changes[checkpoint.id] !== undefined);

  if (!sessionId || !changesLoaded || changed.length === 0) return null;

  return (
    <>
      <button
        ref={triggerRef}
        onClick={() => setOpen(true)}
        aria-label={`恢复 ${changed.length}`}
        title="查看可恢复快照"
        className={embedded
          ? `inline-flex items-center gap-1 rounded border border-border bg-surface-2 px-2 text-[11px] text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200 ${narrow ? "h-11" : "h-9"}`
          : "inline-flex h-9 items-center gap-1 rounded border border-border bg-surface-2 px-2 text-[11px] text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"}
      >
        <History size={11} />
        <span>恢复</span>
        <span className="tabular-nums text-gray-600">{changed.length}</span>
      </button>

      {open && (
        <div
          className={embedded ? "absolute inset-0 z-30 bg-surface-1" : "fixed inset-0 z-40 bg-black/30"}
          onClick={() => setOpen(false)}
        >
          <aside
            role="dialog"
            aria-modal={embedded ? "false" : "true"}
            aria-label="检查点抽屉"
            className={embedded
              ? "flex h-full w-full flex-col bg-surface-1"
              : "absolute inset-y-0 right-0 flex w-[min(420px,92vw)] flex-col border-l border-border bg-surface-1 shadow-2xl"}
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => trapDialogKeyboard(event, () => setOpen(false))}
          >
            <header className="flex items-start gap-3 border-b border-border px-4 py-3">
              <GitBranch size={15} className="mt-0.5 text-accent" />
              <div className="min-w-0 flex-1">
                <h2 className="text-sm font-semibold text-gray-100">检查点</h2>
                <p className="mt-0.5 text-[11px] text-gray-600">
                  自动快照不会移动 HEAD；恢复结果会作为普通工作区修改供你审查。
                </p>
              </div>
              <button
                ref={drawerCloseRef}
                onClick={() => setOpen(false)}
                aria-label="关闭检查点"
                data-auxiliary-initial-focus={embedded ? true : undefined}
                className={embedded
                  ? `inline-flex shrink-0 items-center justify-center rounded text-gray-600 hover:bg-surface-3 hover:text-gray-200 ${narrow ? "h-11 w-11" : "h-9 w-9"}`
                  : "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded text-gray-600 hover:bg-surface-3 hover:text-gray-200"}
              >
                <X size={14} />
              </button>
            </header>

            <div className="min-h-0 flex-1 overflow-y-auto p-3">
              {checkpoints.length === 0 ? (
                <p className="rounded border border-dashed border-border px-3 py-8 text-center text-xs text-gray-600">
                  暂无检查点。Git 项目发送下一条消息前会自动创建。
                </p>
              ) : (
                <>
                  <section>
                    <div className="mb-2 flex items-center gap-2 px-1 text-[11px] font-semibold uppercase tracking-wider text-gray-500">
                      可恢复变更
                      <span className="ml-auto tabular-nums text-gray-600">{changed.length}</span>
                    </div>
                    {changed.length === 0 ? (
                      <p className="px-2 py-4 text-xs text-gray-600">当前检查点都与工作区一致。</p>
                    ) : (
                      <ul className="space-y-1">
                        {visibleChanged.map((checkpoint) => (
                          <CheckpointRow
                            key={checkpoint.id}
                            checkpoint={checkpoint}
                            fileCount={changes[checkpoint.id]?.filter((file) => file.path).length ?? 0}
                            narrow={narrow}
                            onRequestRevert={setConfirming}
                          />
                        ))}
                      </ul>
                    )}
                    {!showAllChanged && changed.length > RECENT_LIMIT && (
                      <button
                        onClick={() => setShowAllChanged(true)}
                        aria-label={`查看最近 ${changed.length} 个有效检查点`}
                        className="mt-2 w-full rounded px-2 py-1.5 text-[11px] text-gray-500 hover:bg-surface-2 hover:text-gray-200"
                      >
                        查看最近 {changed.length} 个有效检查点
                      </button>
                    )}
                  </section>

                  {unchanged.length > 0 && (
                    <section className="mt-4 border-t border-border pt-3">
                      <button
                        onClick={() => setShowUnchanged((value) => !value)}
                        aria-label={`${showUnchanged ? "收起" : "查看"} ${unchanged.length} 个无差异检查点`}
                        className="flex w-full items-center gap-2 rounded px-1 py-1 text-[11px] font-semibold uppercase tracking-wider text-gray-600 hover:text-gray-300"
                      >
                        无文件差异
                        <span className="ml-auto tabular-nums">{unchanged.length}</span>
                      </button>
                      {showUnchanged && (
                        <ul className="mt-1 space-y-1 opacity-60">
                          {unchanged.map((checkpoint) => (
                            <CheckpointRow
                              key={checkpoint.id}
                              checkpoint={checkpoint}
                              fileCount={0}
                              narrow={narrow}
                              onRequestRevert={setConfirming}
                            />
                          ))}
                        </ul>
                      )}
                    </section>
                  )}
                </>
              )}
            </div>
          </aside>
        </div>
      )}

      {confirming && (
        <RevertConfirmModal
          embedded={embedded}
          narrow={narrow}
          checkpoint={confirming}
          initialChanges={changes[confirming.id]}
          onCancel={() => setConfirming(null)}
          onDone={() => {
            setConfirming(null);
            void refresh();
          }}
        />
      )}
    </>
  );
}

export function dedupeBySnapshot(checkpoints: CheckpointInfo[]): CheckpointInfo[] {
  const seen = new Set<string>();
  return checkpoints.filter((checkpoint) => {
    if (seen.has(checkpoint.git_sha)) return false;
    seen.add(checkpoint.git_sha);
    return true;
  });
}

function CheckpointRow({
  checkpoint,
  fileCount,
  narrow,
  onRequestRevert,
}: {
  checkpoint: CheckpointInfo;
  fileCount: number;
  narrow: boolean;
  onRequestRevert: (checkpoint: CheckpointInfo) => void;
}) {
  const when = new Date(checkpoint.created_at);
  const label = checkpoint.label.length > 70
    ? `${checkpoint.label.slice(0, 70)}…`
    : checkpoint.label || "(空消息)";

  return (
    <li className="group flex items-center gap-2 rounded border border-transparent px-2 py-2 text-[11px] hover:border-border hover:bg-surface-2">
      <span className="w-12 shrink-0 truncate font-mono text-gray-600" title={checkpoint.git_sha}>
        {checkpoint.git_sha.slice(0, 7)}
      </span>
      <div className="min-w-0 flex-1">
        <div className={checkpoint.reverted ? "truncate text-gray-600 line-through" : "truncate text-gray-300"} title={checkpoint.label}>
          {label}
        </div>
        <div className="mt-0.5 text-[11px] text-gray-600">
          {fileCount > 0 ? `${fileCount} 个文件变化` : "无文件差异"}
        </div>
      </div>
      <span className="shrink-0 text-[11px] text-gray-600" title={when.toLocaleString()}>
        {when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
      </span>
      {checkpoint.reverted ? (
        <span className="inline-flex shrink-0 items-center gap-0.5 text-[11px] text-status-success">
          <Check size={10} /> 已恢复
        </span>
      ) : (
        <button
          onClick={() => onRequestRevert(checkpoint)}
          aria-label={`恢复检查点 ${checkpoint.label}`}
          className={`inline-flex shrink-0 items-center gap-0.5 rounded px-1.5 text-[11px] text-gray-500 transition-colors hover:bg-status-warning-soft hover:text-status-warning ${narrow ? "h-11" : "h-9"}`}
        >
          <RotateCcw size={10} /> 恢复
        </button>
      )}
    </li>
  );
}

function RevertConfirmModal({
  embedded,
  narrow,
  checkpoint,
  initialChanges,
  onCancel,
  onDone,
}: {
  embedded: boolean;
  narrow: boolean;
  checkpoint: CheckpointInfo;
  initialChanges?: CheckpointFileChange[];
  onCancel: () => void;
  onDone: () => void;
}) {
  const [fileChanges, setFileChanges] = useState<CheckpointFileChange[] | null>(initialChanges ?? null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const cancelButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const returnTarget = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelButtonRef.current?.focus();
    return () => {
      if (returnTarget?.isConnected) returnTarget.focus();
    };
  }, []);

  useEffect(() => {
    if (initialChanges) return;
    invoke<CheckpointFileChange[]>("checkpoint_changeset", { checkpointId: checkpoint.id })
      .then(setFileChanges)
      .catch((cause) => setError(String(cause)));
  }, [checkpoint.id, initialChanges]);

  const restore = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke("revert_checkpoint", { checkpointId: checkpoint.id });
      onDone();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className={`${embedded ? "absolute" : "fixed"} inset-0 z-50 flex items-center justify-center bg-black/60 p-3`}
      onClick={() => {
        if (!busy) onCancel();
      }}
    >
      <div
        role="dialog"
        aria-modal={embedded ? "false" : "true"}
        aria-label="恢复检查点"
        className="flex max-h-[80vh] w-[min(560px,92vw)] flex-col rounded-xl border border-border bg-surface-2 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => trapDialogKeyboard(event, () => {
          if (!busy) onCancel();
        })}
      >
        <header className="flex items-start justify-between gap-3 border-b border-border px-4 py-3">
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-gray-100">恢复到检查点</h2>
            <p className="mt-0.5 truncate text-[11px] text-gray-500" title={checkpoint.label}>
              {checkpoint.git_sha.slice(0, 7)} · {checkpoint.label || "(空)"}
            </p>
          </div>
          <button
            ref={cancelButtonRef}
            onClick={onCancel}
            aria-label="取消恢复"
            className={`inline-flex shrink-0 items-center justify-center rounded text-gray-600 hover:bg-surface-3 hover:text-gray-300 ${narrow ? "h-11 w-11" : "h-9 w-9"}`}
          >
            <X size={14} />
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          {error ? (
            <div className="flex items-start gap-2 rounded border border-status-danger/40 bg-status-danger-soft p-2 text-xs text-status-danger">
              <AlertCircle size={12} className="mt-0.5 shrink-0" />
              <span className="flex-1 break-words">{error}</span>
            </div>
          ) : fileChanges === null ? (
            <div className="text-xs text-gray-500">正在计算差异…</div>
          ) : fileChanges.length === 0 ? (
            <div className="text-xs italic text-gray-500">工作区已与此检查点一致，不会有任何更改。</div>
          ) : (
            <>
              <p className="mb-2 text-[11px] text-gray-500">
                以下文件将恢复到快照状态；当前修改会被覆盖：
              </p>
              <ul className="space-y-0.5">
                {fileChanges.map((file) => (
                  <li key={file.path} className="flex items-center gap-2 font-mono text-[11px]">
                    <StatusIcon status={file.status} />
                    <span className="min-w-0 flex-1 truncate text-gray-300" title={file.path}>{file.path}</span>
                    <span className="text-[11px] uppercase tracking-wide text-gray-600">{statusLabel(file.status)}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>

        <footer className="flex justify-end gap-2 border-t border-border px-4 py-3">
          <button onClick={onCancel} className={`${narrow ? "h-11" : "h-9"} rounded px-3 text-xs text-gray-400 hover:bg-surface-3 hover:text-gray-200`}>
            取消
          </button>
          <button
            onClick={() => void restore()}
            disabled={busy || fileChanges === null || fileChanges.length === 0}
            aria-label="确认恢复"
            className={`${narrow ? "h-11" : "h-9"} inline-flex items-center gap-1.5 rounded border border-status-warning/40 bg-status-warning-soft px-3 text-xs text-status-warning transition-colors hover:brightness-95 disabled:cursor-not-allowed disabled:opacity-40`}
          >
            <RotateCcw size={11} />
            {busy ? "正在恢复…" : "恢复"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function StatusIcon({ status }: { status: CheckpointFileChange["status"] }) {
  switch (status) {
    case "added": return <FilePlus size={11} className="shrink-0 text-status-info" />;
    case "deleted": return <FileMinus size={11} className="shrink-0 text-gray-500" />;
    case "modified": return <FileEdit size={11} className="shrink-0 text-accent" />;
    default: return <FileText size={11} className="shrink-0 text-gray-500" />;
  }
}

function statusLabel(status: CheckpointFileChange["status"]): string {
  switch (status) {
    case "added": return "新增";
    case "deleted": return "删除";
    case "modified": return "修改";
    case "renamed": return "重命名";
    case "typechange": return "类型变化";
  }
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
