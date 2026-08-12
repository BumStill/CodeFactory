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
  Search,
  ShieldQuestion,
  ChevronLeft,
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
  /** Collapse or dismiss this session-owned rail from its own header. */
  onCollapse?: () => void;
  /** Narrow overlays use close language; wide rails use collapse language. */
  collapseLabel?: "收起会话侧栏" | "关闭会话侧栏";
}

export function SessionSidebar({
  currentSessionId,
  onOpenSession,
  onNewConversation,
  onCollapse,
  collapseLabel = "收起会话侧栏",
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

  const [query, setQuery] = useState("");
  const filteredSessions = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return sessions;
    return sessions.filter((session) =>
      `${session.title ?? ""}\n${session.cwd ?? ""}`
        .toLocaleLowerCase()
        .includes(normalized),
    );
  }, [query, sessions]);
  const rail = useMemo(() => buildSessionRail(filteredSessions), [filteredSessions]);
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
      <div className="shrink-0 border-b border-border/80 px-2 pb-2">
        <div className="flex h-11 items-center justify-between px-1">
          <div className="flex min-w-0 items-center gap-1">
            <span className="text-note font-semibold text-gray-300">会话</span>
            {onCollapse && (
              <button
                type="button"
                onClick={onCollapse}
                aria-label={collapseLabel}
                title={collapseLabel}
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
              >
                <ChevronLeft size={16} aria-hidden="true" />
              </button>
            )}
          </div>
          <button
            type="button"
            onClick={() => onNewConversation(null)}
            aria-label="新建会话"
            title="新建会话（空白，可在输入框上方选择项目）"
            className="flex h-8 w-8 items-center justify-center rounded-lg text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
          >
            <Plus size={16} />
          </button>
        </div>
        <label className="flex h-8 items-center gap-2 rounded-lg border border-border/70 bg-surface-2 px-2 text-gray-500 transition-colors focus-within:border-accent/50 focus-within:text-gray-400">
          <Search size={13} aria-hidden="true" className="shrink-0" />
          <span className="sr-only">搜索会话</span>
          <input
            type="search"
            aria-label="搜索会话"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索会话"
            className="min-w-0 flex-1 bg-transparent text-note text-gray-300 placeholder:text-gray-600 outline-none"
          />
        </label>
      </div>

      <div className="flex-1 overflow-y-auto scrollbar-auto-hide px-1.5 py-2.5">
        {draftActive && draftSession && (
          <div
            aria-current="page"
            data-draft-row
            className="relative mb-1 flex min-h-10 w-full items-center gap-2 rounded-lg bg-surface-3 px-2 py-1.5 text-left before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:rounded before:bg-accent"
          >
            <MessageSquare size={12} className="shrink-0 text-accent" />
            <span className="min-w-0 flex-1 truncate text-note font-medium text-gray-100">
              新会话
            </span>
            <span className="text-caption text-accent">{draftSession.anonymous ? "匿名草稿" : "草稿"}</span>
          </div>
        )}

        {rail.length === 0 ? (
          <p className="px-2 py-8 text-center text-note leading-relaxed text-gray-600">
            {query ? "没有匹配的会话" : "还没有会话"}
            <br />
            <span className="text-caption text-gray-600">
              {query ? "试试标题或项目路径" : "点右上角「＋」开始"}
            </span>
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
      <div className="group flex min-h-9 w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-surface-2">
        <button
          type="button"
          aria-label={`${expanded ? "收起" : "展开"}项目 ${name}`}
          aria-expanded={expanded}
          title={cwd}
          onClick={onToggle}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 self-stretch rounded-md text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/60"
        >
          {expanded ? (
            <FolderOpen size={11} className="shrink-0 text-gray-400" />
          ) : (
            <Folder size={11} className="shrink-0 text-gray-500" />
          )}
          <span className="min-w-0 flex-1 truncate text-note font-medium text-gray-200">
            {name}
          </span>
          <span className="shrink-0 text-caption tabular-nums text-gray-600">{count}</span>
        </button>
        <button
          type="button"
          aria-label={`在 ${name} 里新建会话`}
          title="在此项目里新建会话"
          onClick={onNewConversation}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-gray-600 opacity-0 transition-opacity hover:bg-surface-3 hover:text-gray-200 group-hover:opacity-100 group-focus-within:opacity-100"
        >
          <Plus size={12} />
        </button>
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
  const waitingPermission = useChatStore(
    (s) => s.runtime?.[session.id]?.pendingPermission != null,
  );
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
  const title = session.title || "未命名会话";
  const statusIndicator = waitingPermission ? (
    <ShieldQuestion size={12} className="shrink-0 text-status-warning" aria-label="等待批准" />
  ) : streaming ? (
    <Loader2
      size={11}
      className="shrink-0 animate-spin text-status-progress motion-reduce:animate-none"
      aria-label="运行中"
    />
  ) : null;
  const metadata = (
    <div className="mt-0.5 flex min-w-0 items-center gap-1 pl-[18px] text-caption leading-4 text-gray-600">
      {!nested && projectName && (
        <>
          <span className="truncate">{projectName}</span>
          <span aria-hidden="true">·</span>
        </>
      )}
      <span className="shrink-0">{formatRelativeTime(session.updated_at)}</span>
    </div>
  );

  return (
    <li>
      <div
        data-session-row
        className={`group relative flex min-h-10 w-full items-start rounded-lg px-2 py-1.5 text-left transition-colors before:absolute before:inset-y-1.5 before:left-0 before:w-0.5 before:rounded ${
          active ? "bg-surface-3 before:bg-accent" : "before:bg-transparent hover:bg-surface-2"
        }`}
      >
        {editing ? (
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <MessageSquare
                size={11}
                className={`shrink-0 ${active ? "text-accent" : "text-gray-600"}`}
              />
              <input
                autoFocus
                aria-label="重命名会话"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitRename();
                  if (e.key === "Escape") setEditing(false);
                }}
                onBlur={commitRename}
                className="min-w-0 flex-1 rounded-md border border-accent/50 bg-surface-3 px-1.5 py-0.5 text-note text-gray-100 outline-none"
              />
              {statusIndicator}
            </div>
            {metadata}
          </div>
        ) : (
          <button
            type="button"
            aria-label={`打开会话 ${title}`}
            aria-current={active ? "page" : undefined}
            title={`${title} · 双击标题可重命名`}
            onClick={onClick}
            className="min-w-0 flex-1 cursor-pointer rounded-md text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/60"
          >
            <div className="flex items-center gap-1.5">
              <MessageSquare
                size={11}
                className={`shrink-0 ${active ? "text-accent" : "text-gray-600"}`}
              />
              <span
                title={`${title} · 双击重命名`}
                onDoubleClick={(e) => {
                  e.stopPropagation();
                  startRename();
                }}
                className={`flex-1 truncate text-note ${
                  active ? "font-medium text-gray-100" : "text-gray-300"
                }`}
              >
                {title}
              </span>
              {statusIndicator}
            </div>
            {metadata}
          </button>
        )}
        {!editing && !confirming && (
          <div className="relative shrink-0" ref={menuRef}>
            <button
              type="button"
              title="更多操作"
              aria-label="更多操作"
              onClick={() => setMenuOpen((v) => !v)}
              className={`flex items-center rounded p-0.5 transition-opacity hover:bg-surface-3 hover:text-gray-200 ${
                menuOpen
                  ? "text-gray-200 opacity-100"
                  : "text-gray-600 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
              }`}
            >
              <MoreHorizontal size={13} />
            </button>
            {menuOpen && (
              <div className="absolute right-0 top-full z-50 mt-1 min-w-[100px] overflow-hidden rounded-md border border-border bg-surface-2 py-0.5 shadow-xl">
                <button
                  type="button"
                  onClick={startRename}
                  className="flex min-h-8 w-full items-center gap-1.5 px-2.5 py-1.5 text-note text-gray-300 hover:bg-surface-3"
                >
                  <Pencil size={11} />
                  重命名
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setMenuOpen(false);
                    setConfirming(true);
                  }}
                  className="flex min-h-8 w-full items-center gap-1.5 px-2.5 py-1.5 text-note text-status-danger hover:bg-status-danger-soft"
                >
                  <Trash2 size={11} />
                  删除
                </button>
              </div>
            )}
          </div>
        )}
        {confirming && (
          <span className="flex shrink-0 items-center gap-1">
            <button
              type="button"
              aria-label="确认删除"
              title="确认删除"
              onClick={onDelete}
              className="inline-flex min-h-7 items-center rounded-md bg-status-danger-soft px-1.5 text-caption text-status-danger hover:brightness-95"
            >
              删除
            </button>
            <button
              type="button"
              aria-label="取消删除"
              title="取消"
              onClick={() => setConfirming(false)}
              className="inline-flex min-h-7 items-center rounded-md px-1.5 text-caption text-gray-500 hover:bg-surface-3"
            >
              取消
            </button>
          </span>
        )}
      </div>
    </li>
  );
}
