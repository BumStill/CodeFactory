// SPDX-License-Identifier: Apache-2.0
//
// Per-session git checkpoints panel — surfaces every auto-snapshot the
// backend captured before sending a user message, and offers one-click
// revert with a pre-confirm file-list dialog so the user can see exactly
// what will change before agreeing to it.
//
// This is the user-facing half of the "AI 放手干，错了便宜回滚" principle:
// trusting the agent to act, with a cheap reversal path when it goes wrong.

import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown, ChevronRight, RotateCcw, GitBranch, Check, AlertCircle,
  X, FileText, FileMinus, FilePlus, FileEdit,
} from "lucide-react";
import { invoke } from "../lib/tauri";
import type { CheckpointInfo, CheckpointFileChange } from "../lib/tauri";

interface Props {
  sessionId: string | null;
}

export function CheckpointsPanel({ sessionId }: Props) {
  const [checkpoints, setCheckpoints] = useState<CheckpointInfo[]>([]);
  const [expanded, setExpanded] = useState(true);
  const [confirming, setConfirming] = useState<CheckpointInfo | null>(null);

  const refresh = useCallback(async () => {
    if (!sessionId) {
      setCheckpoints([]);
      return;
    }
    try {
      const list = await invoke<CheckpointInfo[]>("list_checkpoints", { sessionId });
      setCheckpoints(list);
    } catch {
      setCheckpoints([]);
    }
  }, [sessionId]);

  useEffect(() => { void refresh(); }, [refresh]);

  // The backend fires `checkpoint-created` after every successful auto-snapshot;
  // refresh the list so the new entry appears without a manual reload.
  useEffect(() => {
    let cancel = false;
    let un: (() => void) | null = null;
    listen<string>("checkpoint-created", (e) => {
      if (!cancel && e.payload === sessionId) {
        void refresh();
      }
    }).then((fn) => { if (cancel) fn(); else un = fn; });
    return () => { cancel = true; un?.(); };
  }, [sessionId, refresh]);

  if (!sessionId) return null;

  return (
    <>
      <div className="border-t border-border bg-surface-1">
        <button
          onClick={() => setExpanded(!expanded)}
          className="w-full flex items-center gap-2 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-gray-500 hover:text-gray-300 transition-colors"
        >
          {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
          <GitBranch size={11} />
          <span>Checkpoints</span>
          {checkpoints.length > 0 && (
            <span className="ml-auto text-[10px] text-gray-600 tabular-nums">
              {checkpoints.length}
            </span>
          )}
        </button>

        {expanded && (
          <div className="px-1 pb-2 max-h-[28vh] overflow-y-auto">
            {checkpoints.length === 0 ? (
              <div className="px-3 py-2 text-[11px] text-gray-600 italic">
                No checkpoints yet. One will appear after the next message
                if this folder is a git repo.
              </div>
            ) : (
              <ul className="space-y-0.5">
                {checkpoints.map((cp) => (
                  <CheckpointRow key={cp.id} cp={cp} onRequestRevert={setConfirming} />
                ))}
              </ul>
            )}
          </div>
        )}
      </div>

      {confirming && (
        <RevertConfirmModal
          cp={confirming}
          onCancel={() => setConfirming(null)}
          onDone={() => { setConfirming(null); void refresh(); }}
        />
      )}
    </>
  );
}

function CheckpointRow({ cp, onRequestRevert }: { cp: CheckpointInfo; onRequestRevert: (c: CheckpointInfo) => void; }) {
  const when = new Date(cp.created_at);
  const label = cp.label.length > 50 ? cp.label.slice(0, 50) + "…" : cp.label;
  return (
    <li className="group flex items-center gap-1.5 px-2 py-1 rounded hover:bg-surface-2 text-[11px]">
      <span className="font-mono text-gray-700 shrink-0 w-12 truncate" title={cp.git_sha}>
        {cp.git_sha.slice(0, 7)}
      </span>
      <span className={`flex-1 min-w-0 truncate ${cp.reverted ? "text-gray-700 line-through" : "text-gray-400"}`} title={cp.label}>
        {label || "(empty message)"}
      </span>
      <span className="text-[10px] text-gray-700 shrink-0" title={when.toLocaleString()}>
        {when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
      </span>
      {cp.reverted ? (
        <span className="text-[10px] text-emerald-500 shrink-0 flex items-center gap-0.5" title="Already reverted">
          <Check size={10} /> reverted
        </span>
      ) : (
        <button
          onClick={() => onRequestRevert(cp)}
          className="opacity-0 group-hover:opacity-100 transition-opacity shrink-0 flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[10px] text-gray-500 hover:text-amber-400 hover:bg-amber-500/10"
          title="Revert working tree to this checkpoint"
        >
          <RotateCcw size={10} /> revert
        </button>
      )}
    </li>
  );
}

function RevertConfirmModal({ cp, onCancel, onDone }: {
  cp: CheckpointInfo;
  onCancel: () => void;
  onDone: () => void;
}) {
  const [changes, setChanges] = useState<CheckpointFileChange[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<CheckpointFileChange[]>("checkpoint_changeset", { checkpointId: cp.id })
      .then(setChanges)
      .catch((e) => setError(String(e)));
  }, [cp.id]);

  const handleRevert = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke("revert_checkpoint", { checkpointId: cp.id });
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onCancel}>
      <div
        className="w-[min(560px,92vw)] max-h-[80vh] flex flex-col rounded-xl border border-border bg-surface-2 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-3 px-4 py-3 border-b border-border">
          <div className="min-w-0">
            <div className="text-sm font-semibold text-gray-100">Revert to checkpoint</div>
            <div className="text-[11px] text-gray-500 mt-0.5 truncate" title={cp.label}>
              {cp.git_sha.slice(0, 7)} · {cp.label || "(empty)"}
            </div>
          </div>
          <button onClick={onCancel} className="text-gray-600 hover:text-gray-300">
            <X size={14} />
          </button>
        </div>

        <div className="px-4 py-3 flex-1 overflow-y-auto">
          {error ? (
            <div className="flex items-start gap-2 p-2 rounded bg-rose-500/10 border border-rose-500/40 text-xs text-rose-800 dark:text-rose-300">
              <AlertCircle size={12} className="shrink-0 mt-0.5" />
              <span className="flex-1 break-words">{error}</span>
            </div>
          ) : changes === null ? (
            <div className="text-xs text-gray-500">Computing diff…</div>
          ) : changes.length === 0 ? (
            <div className="text-xs text-gray-500 italic">
              Working tree already matches this checkpoint — nothing would change.
            </div>
          ) : (
            <>
              <div className="text-[11px] text-gray-500 mb-2">
                These files will be restored to their state at the checkpoint
                (your current edits to them will be overwritten):
              </div>
              <ul className="space-y-0.5">
                {changes.map((c) => (
                  <li key={c.path} className="flex items-center gap-2 text-[11px] font-mono">
                    <StatusIcon status={c.status} />
                    <span className="text-gray-300 truncate flex-1" title={c.path}>{c.path}</span>
                    <span className="text-[10px] text-gray-600 uppercase tracking-wide">{c.status}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>

        <div className="flex justify-end gap-2 px-4 py-3 border-t border-border">
          <button
            onClick={onCancel}
            className="px-3 py-1.5 text-xs text-gray-400 hover:text-gray-200 rounded hover:bg-surface-3 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleRevert}
            disabled={busy || changes === null || changes.length === 0}
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs bg-amber-500/20 text-amber-800 dark:text-amber-300 border border-amber-500/40 rounded hover:bg-amber-500/30 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            <RotateCcw size={11} />
            {busy ? "Reverting…" : "Revert"}
          </button>
        </div>
      </div>
    </div>
  );
}

function StatusIcon({ status }: { status: CheckpointFileChange["status"] }) {
  switch (status) {
    case "added":    return <FilePlus size={11} className="text-emerald-400 shrink-0" />;
    case "deleted":  return <FileMinus size={11} className="text-rose-400 shrink-0" />;
    case "modified": return <FileEdit size={11} className="text-amber-400 shrink-0" />;
    default:         return <FileText size={11} className="text-gray-500 shrink-0" />;
  }
}
