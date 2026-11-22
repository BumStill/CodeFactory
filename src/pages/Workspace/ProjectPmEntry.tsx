// SPDX-License-Identifier: Apache-2.0
import { useEffect, useId, useRef, useState } from "react";
import { useChatStore } from "../../stores/chat";

/** Workspace owns this navigation surface; a cwd is a display scope, not a PM
 * identity, grant, executable conversation or central project mapping. */
export function ProjectPmEntry({ sessionId }: { sessionId: string }) {
  const cwd = useChatStore((state) => {
    const draft = state.draftSession;
    if (draft?.id === sessionId) {
      return !draft.anonymous && draft.cwd?.trim() ? draft.cwd : null;
    }
    const listed = state.sessions.find((session) => session.id === sessionId);
    const active = state.activeSession?.id === sessionId ? state.activeSession : null;
    const matches = [listed, active].filter((session) => session != null);
    if (!matches.length || matches.some((session) =>
      // Missing kind is the existing legacy project-session representation.
      (session.kind !== undefined && session.kind !== "project") || !session.cwd.trim())) {
      return null;
    }
    const current = matches[0];
    // Conflicting snapshots must not expose either project's context.
    return matches.every((session) => session.cwd === current.cwd) ? current.cwd : null;
  });
  return cwd ? <ScopedEntry key={cwd} cwd={cwd} /> : null;
}

function ScopedEntry({ cwd }: { cwd: string }) {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const regionId = useId();
  useEffect(() => { if (open) heading.current?.focus(); }, [open]);
  return (
    <div className="shrink-0 border-b border-border/80 p-2">
      <button ref={button} type="button" aria-label="打开当前项目 PM Bot"
        aria-expanded={open} aria-controls={regionId}
        onClick={() => setOpen(!open)}
        className="w-full rounded-lg px-3 py-2 text-left text-note font-medium text-gray-200 hover:bg-surface-3">
        项目管理 Bot
      </button>
      {open && (
        <section id={regionId} role="region" aria-label="当前项目 PM Bot"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault(); event.stopPropagation();
              setOpen(false); button.current?.focus();
            }
          }}
          className="max-h-[40vh] space-y-2 overflow-y-auto rounded-lg bg-surface-2 p-3 text-note text-gray-300">
          <h2 ref={heading} tabIndex={-1} className="font-medium outline-none">当前项目 PM Bot</h2>
          <p className="break-all">{cwd}</p>
          <p role="status">此项目的 PM Bot 运行时尚未接入，当前无法开始对话或安排工作。</p>
        </section>
      )}
    </div>
  );
}
