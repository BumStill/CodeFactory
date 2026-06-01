// SPDX-License-Identifier: Apache-2.0
//
// SessionSidebar — Codex-style left rail for the Workspace. A unified,
// most-recent-first list of every session (quick + project, each tagged),
// with in-place switching and a "+ 新建" menu (quick or project). This is
// the primary left element; the task tree is now an *adaptive* section shown
// only for project sessions (see WorkspacePage) — quick chats don't get a
// meaningless task column.
//
// Mental model: 快速任务 ≈ lightweight "cowork" chat, 项目 ≈ full "code"
// project — both created and switched from this one rail.
import { useEffect, useMemo, useRef, useState } from "react";
import { Plus, ChevronDown, Zap, Folder, EyeOff, Loader2 } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useChatStore } from "../stores/chat";
import { createQuickSession } from "../lib/tauri";
import { formatRelativeTime } from "../lib/time";
import type { Session } from "../lib/tauri";

interface SessionSidebarProps {
  /** The session currently open in the workspace (highlighted in the list). */
  currentSessionId: string;
  /** Switch the workspace to another session (in-place; no Home round-trip). */
  onOpenSession: (id: string) => void;
}

export function SessionSidebar({ currentSessionId, onOpenSession }: SessionSidebarProps) {
  const sessions = useChatStore((s) => s.sessions);
  const quickSessions = useChatStore((s) => s.quickSessions);
  const activeModel = useChatStore((s) => s.activeModel);
  const createSession = useChatStore((s) => s.createSession);
  const loadSessions = useChatStore((s) => s.loadSessions);
  const loadQuickSessions = useChatStore((s) => s.loadQuickSessions);
  const startAnonymousSession = useChatStore((s) => s.startAnonymousSession);

  const [menuOpen, setMenuOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Load both lists when the sidebar first mounts.
  useEffect(() => {
    void loadSessions();
    void loadQuickSessions();
  }, []);

  // Close the "+ 新建" menu on an outside click.
  useEffect(() => {
    if (!menuOpen) return;
    const onClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [menuOpen]);

  // Unified, most-recent-first. `sessions` is project-only and `quickSessions`
  // is quick-only (no overlap), so a plain concat + sort is correct.
  const merged = useMemo(
    () => [...sessions, ...quickSessions].sort((a, b) => b.updated_at - a.updated_at),
    [sessions, quickSessions],
  );

  const handleNewQuick = async () => {
    setMenuOpen(false);
    if (busy) return;
    setBusy(true);
    try {
      const s = await createQuickSession(activeModel);
      await loadQuickSessions();
      onOpenSession(s.id);
    } catch (e) {
      // eslint-disable-next-line no-alert
      alert(`新建快速任务失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleNewProject = async () => {
    setMenuOpen(false);
    if (busy) return;
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    if (!dir) return;
    setBusy(true);
    try {
      const s = await createSession(dir as string, activeModel);
      if (s) onOpenSession(s.id);
    } catch (e) {
      // eslint-disable-next-line no-alert
      alert(`新建项目失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleNewAnonymous = () => {
    setMenuOpen(false);
    if (busy) return;
    // Purely in-memory — no backend call, nothing persisted. Navigate to it.
    const s = startAnonymousSession();
    onOpenSession(s.id);
  };

  return (
    <div className="flex flex-col min-h-0 flex-1">
      {/* ── "+ 新建" menu ─────────────────────────────────────────────── */}
      <div className="relative p-2 border-b border-border" ref={menuRef}>
        <button
          onClick={() => setMenuOpen((v) => !v)}
          disabled={busy}
          className="flex w-full items-center justify-center gap-1.5 rounded-lg bg-accent px-2 py-1.5 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
        >
          <Plus size={13} />
          新建
          <ChevronDown size={12} className="opacity-80" />
        </button>
        {menuOpen && (
          <div className="absolute left-2 right-2 top-full z-50 mt-1 overflow-hidden rounded-lg border border-border bg-surface-2 shadow-xl">
            <button
              onClick={handleNewQuick}
              className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-gray-300 transition-colors hover:bg-surface-3"
            >
              <Zap size={12} className="text-accent" />
              <span className="flex-1">新建快速任务</span>
              <span className="text-[9px] text-gray-600">轻量对话</span>
            </button>
            <button
              onClick={handleNewProject}
              className="flex w-full items-center gap-2 border-t border-border px-3 py-2 text-left text-xs text-gray-300 transition-colors hover:bg-surface-3"
            >
              <Folder size={12} className="text-gray-400" />
              <span className="flex-1">新建项目</span>
              <span className="text-[9px] text-gray-600">完整工程</span>
            </button>
            <button
              onClick={handleNewAnonymous}
              className="flex w-full items-center gap-2 border-t border-border px-3 py-2 text-left text-xs text-gray-300 transition-colors hover:bg-surface-3"
            >
              <EyeOff size={12} className="text-gray-400" />
              <span className="flex-1">新建匿名任务</span>
              <span className="text-[9px] text-gray-600">无痕 · 不留存</span>
            </button>
          </div>
        )}
      </div>

      {/* ── Unified recent-session list ───────────────────────────────── */}
      <div className="flex-1 overflow-y-auto p-1.5">
        <h2 className="px-1.5 py-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500">
          最近会话
        </h2>
        {merged.length === 0 ? (
          <p className="px-1.5 py-6 text-center text-[11px] leading-relaxed text-gray-600">
            还没有会话
            <br />
            <span className="text-gray-700">点上面「新建」开始</span>
          </p>
        ) : (
          <ul className="space-y-0.5">
            {merged.map((s) => (
              <SessionRow
                key={s.id}
                session={s}
                active={s.id === currentSessionId}
                onClick={() => onOpenSession(s.id)}
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function SessionRow({
  session,
  active,
  onClick,
}: {
  session: Session;
  active: boolean;
  onClick: () => void;
}) {
  const isQuick = session.kind === "quick";
  // Per-session streaming indicator: with concurrent sessions, any row may be
  // mid-stream even when it's not the foreground one.
  const streaming = useChatStore((s) => s.runtime?.[session.id]?.streaming ?? false);
  return (
    <li>
      <button
        onClick={onClick}
        aria-current={active ? "page" : undefined}
        className={`group w-full rounded-md px-2 py-1.5 text-left transition-colors ${
          active
            ? "border border-accent/40 bg-accent/15"
            : "border border-transparent hover:bg-surface-2"
        }`}
      >
        <div className="flex items-center gap-1.5">
          {isQuick ? (
            <Zap size={11} className="shrink-0 text-accent" />
          ) : (
            <Folder size={11} className="shrink-0 text-gray-500" />
          )}
          <span
            className={`flex-1 truncate text-[12px] ${
              active ? "font-medium text-gray-100" : "text-gray-300"
            }`}
          >
            {session.title || (isQuick ? "快速任务" : "未命名项目")}
          </span>
          {streaming && (
            <Loader2 size={11} className="shrink-0 animate-spin text-accent" aria-label="运行中" />
          )}
          <span
            className={`shrink-0 rounded px-1 py-0.5 text-[8px] ${
              isQuick ? "bg-accent/15 text-accent" : "bg-surface-3 text-gray-500"
            }`}
          >
            {isQuick ? "快速" : "项目"}
          </span>
        </div>
        <div className="mt-0.5 pl-[18px] text-[9px] text-gray-600">
          {formatRelativeTime(session.updated_at)}
        </div>
      </button>
    </li>
  );
}
