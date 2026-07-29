// SPDX-License-Identifier: Apache-2.0
//
// SessionSidebar — the Workspace's left rail: ONE recency-ordered list of
// conversations, where a directory quietly becomes a collapsible group once
// it holds more than one of them.
//
// Nobody creates a project here. There is no "new project" action and no
// classification step: a project is simply what a folder *becomes* after you
// have worked in it twice, and a folder used once stays an ordinary row. That
// keeps the concept a description of what happened rather than a decision
// forced before any work starts — and if the useful boundary later turns out
// not to be a directory, only `lib/projects` changes.
//
// Two rules this rail enforces:
//
//   1. "+" always opens a BLANK conversation. Picking a project (here or in
//      the composer) only chooses where the new conversation works — it never
//      re-opens an old one.
//   2. Entering an existing conversation happens ONE way: clicking its row.
//      Clicking a folder expands it, nothing more.
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Plus,
  Folder,
  FolderOpen,
  MessageSquare,
  Loader2,
  Pencil,
  Trash2,
  MoreHorizontal,
} from "lucide-react";
import { useChatStore } from "../stores/chat";
import { formatRelativeTime } from "../lib/time";
import { buildSessionRail } from "../lib/projects";
import type { Session } from "../lib/tauri";

interface SessionSidebarProps {
  /** The conversation currently open in the workspace (highlighted). */
  currentSessionId: string;
  /** Open an existing conversation (in-place; no Home round-trip). */
  onOpenSession: (id: string) => void;
  /** Start a blank conversation, optionally scoped to a project directory. */
  onNewConversation: (cwd?: string | null) => void;
}

export function SessionSidebar({
  currentSessionId,
  onOpenSession,
  onNewConversation,
}: SessionSidebarProps) {
  const sessions = useChatStore((s) => s.sessions);
  const draftSession = useChatStore((s) => s.draftSession);
  const loadSessions = useChatStore((s) => s.loadSessions);
  const deleteSession = useChatStore((s) => s.deleteSession);
  const renameSession = useChatStore((s) => s.renameSession);

  // Load the (single, unified) session list when the sidebar first mounts.
  useEffect(() => {
    void loadSessions();
  }, []);

  const rail = useMemo(() => buildSessionRail(sessions), [sessions]);
  const draftActive = draftSession?.id === currentSessionId;
  const draftProject = draftActive ? draftSession?.cwd ?? null : null;

  // A project expands when the user opens it, or when it contains the open
  // conversation/draft. Once expanded, keep it open across later session
  // switches; switching rows is not a collapse command. The only way to close a
  // visible project is clicking that project row again.
  const activeProjectCwd = useMemo(() => {
    if (draftProject) return draftProject;
    const current = sessions.find((s) => s.id === currentSessionId);
    return current && current.kind !== "quick" ? current.cwd : null;
  }, [currentSessionId, draftProject, sessions]);
  const [expandedProjects, setExpandedProjects] = useState<Record<string, boolean>>({});
  const isExpanded = (cwd: string) => Boolean(expandedProjects[cwd] || cwd === activeProjectCwd);
  const toggleProject = (cwd: string) =>
    setExpandedProjects((prev) => ({ ...prev, [cwd]: !isExpanded(cwd) }));

  // Remember projects that became visible because they held the active
  // conversation. Without this, clicking a standalone/quick session would make
  // the previously expanded project snap shut even though the user never asked
  // to collapse it.
  useEffect(() => {
    if (!activeProjectCwd) return;
    setExpandedProjects((prev) => {
      if (prev[activeProjectCwd]) return prev;
      return { ...prev, [activeProjectCwd]: true };
    });
  }, [activeProjectCwd, currentSessionId]);

  return (
    <div className="flex flex-col min-h-0 flex-1">
      {/* ── Toolbar: one unambiguous "new blank conversation" action. ──── */}
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border px-3">
        <span className="text-[11px] font-medium text-gray-400">会话</span>
        <button
          onClick={() => onNewConversation(null)}
          aria-label="新建会话"
          title="新建会话（空白，可在输入框上方选择项目）"
          className="flex h-7 w-7 items-center justify-center rounded-md text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
        >
          <Plus size={15} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto scrollbar-auto-hide px-1.5 py-2">
        {draftActive && draftSession && (
          <div
            aria-current="page"
            data-draft-row
            className="relative mb-1 flex min-h-10 w-full items-center gap-2 rounded-md bg-surface-3 px-2 py-1.5 text-left before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:rounded before:bg-accent"
          >
            <MessageSquare size={12} className="shrink-0 text-accent" />
            <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-gray-100">
              新会话
            </span>
            <span className="text-[9px] text-accent">{draftSession.anonymous ? "匿名草稿" : "草稿"}</span>
          </div>
        )}

        {rail.length === 0 ? (
          <p className="px-2 py-8 text-center text-[11px] leading-relaxed text-gray-600">
            还没有会话
            <br />
            <span className="text-gray-700">点右上角「＋」开始</span>
          </p>
        ) : (
          <ul className="space-y-0.5">
            {rail.map((entry) =>
              entry.kind === "project" ? (
                <ProjectRow
                  key={entry.project.cwd}
                  name={entry.project.name}
                  cwd={entry.project.cwd}
                  count={entry.project.sessions.length}
                  expanded={isExpanded(entry.project.cwd)}
                  onToggle={() => toggleProject(entry.project.cwd)}
                  onNewConversation={() => {
                    setExpandedProjects((prev) => ({ ...prev, [entry.project.cwd]: true }));
                    onNewConversation(entry.project.cwd);
                  }}
                >
                  {entry.project.sessions.map((session) => (
                    <SessionRow
                      key={session.id}
                      session={session}
                      active={session.id === currentSessionId}
                      nested
                      onClick={() => onOpenSession(session.id)}
                      onDelete={() => deleteSession(session.id)}
                      onRename={(title) => renameSession(session.id, title)}
                    />
                  ))}
                </ProjectRow>
              ) : (
                <SessionRow
                  key={entry.session.id}
                  session={entry.session}
                  active={entry.session.id === currentSessionId}
                  projectName={entry.projectName}
                  onClick={() => onOpenSession(entry.session.id)}
                  onDelete={() => deleteSession(entry.session.id)}
                  onRename={(title) => renameSession(entry.session.id, title)}
                />
              ),
            )}
          </ul>
        )}
      </div>
    </div>
  );
}

/** A project: a place to work. Clicking it expands — it never opens a
 *  conversation, which is exactly the confusion this rail is fixing. */
function ProjectRow({
  name,
  cwd,
  count,
  expanded,
  onToggle,
  onNewConversation,
  children,
}: {
  name: string;
  cwd: string;
  count: number;
  expanded: boolean;
  onToggle: () => void;
  onNewConversation: () => void;
  children: React.ReactNode;
}) {
  return (
    <li>
      <div
        role="button"
        tabIndex={0}
        aria-expanded={expanded}
        title={cwd}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
        className="group flex min-h-8 w-full cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-left transition-colors hover:bg-surface-2"
      >
        {expanded ? (
          <FolderOpen size={11} className="shrink-0 text-gray-400" />
        ) : (
          <Folder size={11} className="shrink-0 text-gray-500" />
        )}
        <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-gray-200">{name}</span>
        <span className="shrink-0 text-[9px] text-gray-600">{count}</span>
        <span
          role="button"
          aria-label={`在 ${name} 里新建会话`}
          title="在此项目里新建会话"
          onClick={(e) => {
            e.stopPropagation();
            onNewConversation();
          }}
          className="flex shrink-0 items-center rounded p-0.5 text-gray-600 opacity-0 transition-opacity hover:bg-surface-3 hover:text-gray-200 group-hover:opacity-100 group-focus-within:opacity-100"
        >
          <Plus size={12} />
        </span>
      </div>
      {expanded && <ul className="mt-0.5 space-y-0.5 pl-3">{children}</ul>}
    </li>
  );
}

function SessionRow({
  session,
  active,
  nested = false,
  projectName = null,
  onClick,
  onDelete,
  onRename,
}: {
  session: Session;
  active: boolean;
  nested?: boolean;
  /** Folder this conversation ran in, when it isn't already under one. */
  projectName?: string | null;
  onClick: () => void;
  onDelete: () => void;
  onRename: (title: string) => void;
}) {
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
        className={`group relative min-h-8 w-full cursor-pointer rounded-md px-2 py-1 text-left transition-colors before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:rounded ${
          active ? "bg-surface-3 before:bg-accent" : "before:bg-transparent hover:bg-surface-2"
        }`}
      >
        <div className="flex items-center gap-1.5">
          <MessageSquare size={11} className={`shrink-0 ${active ? "text-accent" : "text-gray-600"}`} />
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
              {session.title || "未命名会话"}
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
                  menuOpen
                    ? "text-gray-200 opacity-100"
                    : "text-gray-600 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
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
          {!nested && projectName && (
            <>
              <span className="truncate">{projectName}</span>
              <span aria-hidden="true">·</span>
            </>
          )}
          <span className="shrink-0">{formatRelativeTime(session.updated_at)}</span>
        </div>
      </div>
    </li>
  );
}
