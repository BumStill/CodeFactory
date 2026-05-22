// SPDX-License-Identifier: Apache-2.0
//
// Per-message "remember this" affordance. Click → small inline editor
// pre-filled with a useful starting point from the message content;
// Save → backend appends a dated entry to .codefactory/memory.md, which
// gets auto-injected into the system prompt every future session in
// this repo. The user's mental model: "teach the AI once, it sticks."

import { useState } from "react";
import { Brain, Check, X } from "lucide-react";
import { invoke } from "../lib/tauri";
import type { ProjectMemory } from "../lib/tauri";

interface Props {
  cwd: string | null;
  suggestedText: string;
}

/**
 * Floats top-right on hover of an assistant message. Hidden when there's
 * no cwd to write into.
 */
export function RememberButton({ cwd, suggestedText }: Props) {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<{ kind: "ok" | "err"; msg: string } | null>(null);

  if (!cwd) return null;

  const handleOpen = () => {
    // Distil the suggested text to one short factual line. The user can
    // edit before saving — we just want a useful starting point.
    const trimmed = suggestedText.trim().split("\n")[0].slice(0, 200);
    setText(trimmed);
    setOpen(true);
  };

  const handleSave = async () => {
    const fact = text.trim();
    if (!fact) return;
    setSaving(true);
    setStatus(null);
    try {
      const saved = await invoke<ProjectMemory>("append_project_memory", {
        cwd,
        fact,
      });
      setStatus({ kind: "ok", msg: `Saved to ${condensePath(saved.path)}` });
      // Briefly show the confirmation, then collapse the editor.
      setTimeout(() => setOpen(false), 1200);
    } catch (e) {
      setStatus({ kind: "err", msg: `Failed: ${e}` });
    } finally {
      setSaving(false);
    }
  };

  if (!open) {
    return (
      <button
        onClick={handleOpen}
        className="opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] text-gray-600 hover:text-amber-300 hover:bg-amber-500/10"
        title="Append a fact to .codefactory/memory.md — auto-injected into every future session"
      >
        <Brain size={11} />
        Remember
      </button>
    );
  }

  return (
    <div className="mt-1 rounded border border-amber-500/40 bg-amber-500/5 p-2 space-y-2">
      <div className="text-[11px] text-amber-300 flex items-center gap-1.5">
        <Brain size={11} />
        Save to project memory
      </div>
      <textarea
        autoFocus
        value={text}
        onChange={(e) => setText(e.target.value)}
        rows={3}
        className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none focus:border-amber-500/60 resize-y"
        placeholder="e.g. This project uses pnpm not npm. Models live under src/models/."
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void handleSave();
          } else if (e.key === "Escape") {
            setOpen(false);
          }
        }}
      />
      <div className="flex justify-end gap-1">
        <button
          onClick={() => setOpen(false)}
          className="flex items-center gap-1 px-2 py-1 rounded text-[11px] text-gray-500 hover:text-gray-300 hover:bg-surface-3"
        >
          <X size={10} />
          Cancel
        </button>
        <button
          onClick={() => void handleSave()}
          disabled={saving || !text.trim()}
          className="flex items-center gap-1 px-2 py-1 rounded text-[11px] bg-amber-500/20 text-amber-300 border border-amber-500/40 hover:bg-amber-500/30 disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <Check size={10} />
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
      {status && (
        <div className={`text-[10px] ${status.kind === "ok" ? "text-emerald-400" : "text-rose-400"}`}>
          {status.msg}
        </div>
      )}
      <div className="text-[10px] text-gray-600">
        ⌘/Ctrl+Enter to save · Esc to cancel
      </div>
    </div>
  );
}

function condensePath(full: string): string {
  // Drop the leading cwd path — the user already knows which project
  // they're in; just show the codefactory-relative tail.
  const idx = full.lastIndexOf(".codefactory");
  if (idx >= 0) return full.slice(idx).replace(/\\/g, "/");
  return full;
}
