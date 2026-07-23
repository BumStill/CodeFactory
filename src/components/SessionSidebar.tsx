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
import { Plus, Zap, Folder, EyeOff, Loader2, Pencil, Trash2, MoreHorizontal } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useChatStore } from "../stores/chat";
import { formatRelativeTime } from "../lib/time";
import type { Session } from "../lib/tauri";

type SessionGroup = { label: "今天" | "昨天" | "过去 7 天" | "更早"; sessions: Session[] };

function groupSessionsByRecency(sessions: Session[], now = Date.now()): SessionGroup[] {
  const day = 24 * 60 * 60 * 1000;
  const startToday = new Date(now);
  startToday.setHours(0, 0, 0, 0);
  const start = startToday.getTime();
  const buckets: Record<SessionGroup["label"], Session[]> = {
    "今天": [],
    "昨天": [],
    "过去 7 天": [],
    "更早": [],
  };
  for (const session of sessions) {
    const timestamp = session.updated_at > 10_000_000_000 ? session.updated_at : session.updated_at * 1000;
    if (timestamp >= start) buckets["今天"].push(session);
    else if (timestamp >= start - day) buckets["昨天"].push(session);
    else if (timestamp >= start - 6 * day) buckets["过去 7 天"].push(session);
    else buckets["更早"].push(session);
  }
  return (["今天", "昨天", "过去 7 天", "更早"] as const)
    .map((label) => ({ label, sessions: buckets[label] }))
    .filter((group) => group.sessions.length > 0);
}

interface SessionSidebarProps {
  /** The session currently open in the workspace (highlighted in the list). */
  currentSessionId: string;
  /** Switch the workspace to another session (in-place; no Home round-trip). */
  onOpenSession: (id: string) => void;
}

export function SessionSidebar({ currentSessionId, onOpenSession }: SessionSidebarProps) {
  const sessions = useChatStore((s) => s.sessions);
  const quickSessions = useChatStore((s) => s.quickSessions);
  const draftSession = useChatStore((s) => s.draftSession);
  const beginQuickDraft = useChatStore((s) => s.beginQuickDraft);
  const beginProjectDraft = useChatStore((s) => s.beginProjectDraft);
  const loadSessions = useChatStore((s) => s.loadSessions);
  const loadQuickSessions = useChatStore((s) => s.loadQuickSessions);
  const startAnonymousSession = useChatStore((s) => s.startAnonymousSession);
  const deleteSession = useChatStore((s) => s.deleteSession);
  const renameSession = useChatStore((s) => s.renameSession);

  const [menuOpen, setMenuOpen] = useState(false);
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
  const grouped = useMemo(() => groupSessionsByRecency(merged), [merged]);
  const draftActive = draftSession?.id === currentSessionId;

  const handleNewQuick = () => {
    setMenuOpen(false);
    const draft = beginQuickDraft();
    onOpenSession(draft.id);
  };

  const handleNewProject = async () => {
    setMenuOpen(false);
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    if (!dir) return;
    const draft = beginProjectDraft(dir as string);
    onOpenSession(draft.id);
  };

  const handleNewAnonymous = () => {
    setMenuOpen(false);
    // Purely in-memory — no backend call, nothing persisted. Navigate to it.
    const s = startAnonymousSession();
    onOpenSession(s.id);
  };

  return (
    <div className="flex flex-col min-h-0 flex-1">
      {/* ── Compact toolbar: one low-emphasis new-session entry. ──────── */}
      <div className="relative flex h-10 shrink-0 items-center justify-between border-b border-border px-3" ref={menuRef}>
        <span className="text-[11px] font-medium text-gray-400">会话</span>
        <button
          onClick={() => setMenuOpen((v) => !v)}
          aria-label="新建"
          title="新建会话"
          className="flex h-7 w-7 items-center justify-center rounded-md text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
        >
          <Plus size={15} />
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

      {/* ── Unified recent-session list grouped for fast scanning. ────── */}
      <div className="flex-1 overflow-y-auto px-1.5 py-2">
        {draftActive && draftSession && (
          <button
            type="button"
            aria-current="page"
            className="relative mb-1 flex min-h-10 w-full items-center gap-2 rounded-md bg-surface-3 px-2 py-1.5 text-left before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:rounded before:bg-accent"
          >
            {draftSession.mode === "quick" ? (
              <Zap size={12} className="shrink-0 text-accent" />
            ) : (
              <Folder size={12} className="shrink-0 text-gray-500" />
            )}
            <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-gray-100">
              {draftSession.mode === "quick" ? "新对话" : (draftSession.cwd?.split(/[/\\]/).pop() || "新项目")}
            </span>
            <span className="text-[9px] text-accent">草稿</span>
          </button>
        )}
        {merged.length === 0 && !draftActive ? (
          <p className="px-2 py-8 text-center text-[11px] leading-relaxed text-gray-600">
            还没有会话<br /><span className="text-gray-700">点击右上角「＋」开始</span>
          </p>
        ) : (
          <div className="space-y-2">
            {grouped.map((group) => (
              <section key={group.label} aria-labelledby={`session-group-${group.label}`}>
                <h2 id={`session-group-${group.label}`} className="px-2 pb-0.5 text-[9px] font-medium tracking-wide text-gray-600">
                  {group.label}
                </h2>
                <ul className="space-y-0.5">
                  {group.sessions.map((session) => (
                    <SessionRow
                      key={session.id}
                      session={session}
                      active={session.id === currentSessionId}
                      onClick={() => onOpenSession(session.id)}
                      onDelete={() => deleteSession(session.id)}
                      onRename={(title) => renameSession(session.id, title)}
                    />
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function SessionRow({
  session,
  active,
  onClick,
  onDelete,
  onRename,
}: {
  session: Session;
  active: boolean;
  onClick: () => void;
  onDelete: () => void;
  onRename: (title: string) => void;
}) {
  const isQuick = session.kind === "quick";
  // Per-session streaming indicator: with concurrent sessions, any row may be
  // mid-stream even when it's not the foreground one.
  const streaming = useChatStore((s) => s.runtime?.[session.id]?.streaming ?? false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Close the ⋯ menu on an outside click.
  useEffect(() => {
    if (!menuOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  const startRename = () => {
    setConfirming(false);
    setMenuOpen(false);
    setDraft(session.title || "");
    setEditing(true);
  };
  const commitRename = () => {
    const t = draft.trim();
    setEditing(false);
    if (t && t !== session.title) onRename(t);
  };

  return (
    <li>
      <div
        role="button"
        data-session-row
        tabIndex={0}
        aria-current={active ? "page" : undefined}
        onClick={() => {
          if (!editing) onClick();
        }}
        onKeyDown={(e) => {
          if (!editing && e.key === "Enter") onClick();
        }}
        className={`group relative min-h-10 w-full cursor-pointer rounded-md px-2 py-1.5 text-left transition-colors before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:rounded ${
          active
            ? "bg-surface-3 before:bg-accent"
            : "before:bg-transparent hover:bg-surface-2"
        }`}
      >
        <div className="flex items-center gap-1.5">
          {isQuick ? (
            <Zap size={11} className="shrink-0 text-accent" />
          ) : (
            <Folder size={11} className="shrink-0 text-gray-500" />
          )}
          {editing ? (
            <input
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") setEditing(false);
              }}
              onBlur={commitRename}
              className="min-w-0 flex-1 rounded border border-accent/50 bg-surface-3 px-1 text-[12px] text-gray-100 outline-none"
            />
          ) : (
            <span
              title="双击重命名"
              onDoubleClick={(e) => {
                e.stopPropagation();
                startRename();
              }}
              className={`flex-1 truncate text-[12px] ${
                active ? "font-medium text-gray-100" : "text-gray-300"
              }`}
            >
              {session.title || (isQuick ? "快速任务" : "未命名项目")}
            </span>
          )}
          {streaming && (
            <Loader2 size={11} className="shrink-0 animate-spin text-accent" aria-label="运行中" />
          )}
          {!editing && !confirming && (
            <div className="relative shrink-0" ref={menuRef}>
              <span
                role="button"
                title="更多操作"
                aria-label="更多操作"
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuOpen((v) => !v);
                }}
                className={`flex items-center rounded p-0.5 transition-opacity hover:bg-surface-3 hover:text-gray-200 ${
                  menuOpen ? "text-gray-200 opacity-100" : "text-gray-600 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
                }`}
              >
                <MoreHorizontal size={13} />
              </span>
              {menuOpen && (
                <div className="absolute right-0 top-full z-50 mt-1 min-w-[100px] overflow-hidden rounded-md border border-border bg-surface-2 py-0.5 shadow-xl">
                  <span
                    role="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      startRename();
                    }}
                    className="flex items-center gap-1.5 px-2.5 py-1.5 text-[11px] text-gray-300 hover:bg-surface-3"
                  >
                    <Pencil size={11} />
                    重命名
                  </span>
                  <span
                    role="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      setMenuOpen(false);
                      setConfirming(true);
                    }}
                    className="flex items-center gap-1.5 px-2.5 py-1.5 text-[11px] text-red-400 hover:bg-red-500/15"
                  >
                    <Trash2 size={11} />
                    删除
                  </span>
                </div>
              )}
            </div>
          )}
          {confirming && (
            <span className="flex shrink-0 items-center gap-1">
              <span
                role="button"
                title="确认删除"
                onClick={(e) => {
                  e.stopPropagation();
                  onDelete();
                }}
                className="rounded bg-red-500/15 px-1 py-0.5 text-[9px] text-red-400 hover:bg-red-500/25"
              >
                删除
              </span>
              <span
                role="button"
                title="取消"
                onClick={(e) => {
                  e.stopPropagation();
                  setConfirming(false);
                }}
                className="rounded px-1 py-0.5 text-[9px] text-gray-500 hover:bg-surface-3"
              >
                取消
              </span>
            </span>
          )}
        </div>
        <div className="mt-0.5 flex min-w-0 items-center gap-1 pl-[18px] text-[9px] text-gray-600">
          {!isQuick && <span className="truncate">{session.cwd?.split(/[/\\]/).pop() || "项目"}</span>}
          {!isQuick && <span aria-hidden="true">·</span>}
          <span className="shrink-0">{formatRelativeTime(session.updated_at)}</span>
        </div>
      </div>
    </li>
  );
}
