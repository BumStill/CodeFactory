// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  BookOpen,
  Settings as SettingsIcon,
  MessageSquare,
  Circle,
  CheckCircle2,
  Loader2,
  XCircle,
  Square,
  EyeOff,
  X,
  Globe2,
  ExternalLink,
  FileText,
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
import { DocumentPreview, type DocumentTab } from "../../components/DocumentPreview";
import { MessageInput } from "../../components/MessageInput";
import { SessionSidebar } from "../../components/SessionSidebar";
import { DraftScopeBar } from "../../components/DraftScopeBar";
import { ModelPicker } from "../../components/ModelPicker";
import { ReasoningEffortPicker } from "../../components/ReasoningEffortPicker";
import { PermissionModePicker } from "../../components/PermissionModePicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { ContextUsageBar } from "../../components/ContextUsageBar";
import { GitStatusBar } from "../../components/GitStatusBar";
import { WorkspaceDeliveryStatus } from "../../components/WorkspaceDeliveryStatus";
import { GitChangesPanel } from "../../components/GitChangesPanel";
import { GitHistoryPanel } from "../../components/GitHistoryPanel";
import { RemoteGitPanel } from "../../components/RemoteGitPanel";
import { useGitStore } from "../../stores/git";
import { recentProjects } from "../../lib/projects";
import { invoke } from "../../lib/tauri";
import { useChatStore, activeRuntime } from "../../stores/chat";
import { QueueBadge } from "../../components/QueueBadge";
import { useTasksStore } from "../../stores/tasks";
import type { BrowserSession, TaskRun, VerificationResult } from "../../lib/tauri";
import type { ExternalJobState, TurnTimingProfile } from "../../lib/chatPlan";
import { parseVerification, verificationSummary } from "../../lib/verification";

type WorkspaceBrowserSession = BrowserSession & {
  status?: string | null;
  pane_url?: string | null;
  current_host?: string | null;
  page_title?: string | null;
};

const BROWSER_PANE_DEFAULT_WIDTH = 38;

function sessionHost(session: WorkspaceBrowserSession): string {
  if (session.current_host) return session.current_host;
  const url = session.pane_url;
  if (!url) return "受管浏览器";
  try {
    return new URL(url).host || "受管浏览器";
  } catch {
    return "受管浏览器";
  }
}

function sessionTitle(session: WorkspaceBrowserSession): string {
  return session.page_title || sessionHost(session);
}

interface WorkspacePageProps {
  sessionId: string;
  /** Start a blank conversation, optionally scoped to a project directory. */
  onNewConversation: (cwd?: string | null) => void;
  onOpenSettings: (tab?: "capabilities" | "endpoints") => void;
  onOpenUsage?: () => void;
  /** Switch the workspace to another session in-place (from the sidebar). */
  onOpenSession: (id: string) => void;
  /** Reveal the task workbench and highlight this task when deep-linked from usage. */
  initialTaskLogId?: string | null;
}

/**
 * Workspace — the primary working surface.
 *
 * Three-column layout:
 *   Left   — Session sidebar (Codex-style: unified quick+project list, in-place
 *            switching, "+ 新建" menu) PLUS an ADAPTIVE task tree shown only for
 *            project sessions — quick chats get no meaningless task column.
 *   Center — Execution stream (AI work in progress + chat input)
 *   Right  — Active skills + memory increments (transparency surface)
 */
export function WorkspacePage({
  sessionId,
  onNewConversation,
  onOpenSettings,
  onOpenUsage,
  onOpenSession,
  initialTaskLogId,
}: WorkspacePageProps) {
  const {
    activeSession, draftSession, sessions,
    sendOrQueue, steerRun, cancelStream, removeFromQueue, setDraftProject, setDraftAnonymous,
    respondPermission, exitAnonymous, renameSession, loadOlderMessages,
  } = useChatStore();
  const activeDraft = draftSession?.id === sessionId ? draftSession : null;
  // Per-session chat state for the ACTIVE session. Background sessions keep
  // streaming into their own buckets; here we render the active one's slice.
  const {
    messages,
    streaming,
    queue,
    pendingPermission,
    hasOlderHistory,
    loadingOlderHistory,
    historyTruncated,
  } = useChatStore(activeRuntime);
  const isAnonymous = activeSession?.kind === "anonymous";
  const persistedRunActive = useTasksStore((state) => state.running[sessionId] ?? false);
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try {
      return localStorage.getItem("cf.workspace.sidebarCollapsed") === "1";
    } catch {
      return false;
    }
  });
  const [narrowViewport, setNarrowViewport] = useState(() =>
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(max-width: 720px)").matches,
  );
  const [narrowSidebarOpen, setNarrowSidebarOpen] = useState(false);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(max-width: 720px)");
    const syncViewport = () => setNarrowViewport(media.matches);
    syncViewport();
    media.addEventListener("change", syncViewport);
    return () => media.removeEventListener("change", syncViewport);
  }, []);
  useEffect(() => {
    if (narrowViewport) setNarrowSidebarOpen(false);
  }, [narrowViewport, sessionId]);
  const sidebarVisible = narrowViewport ? narrowSidebarOpen : !sidebarCollapsed;
  const toggleSidebar = () => {
    if (narrowViewport) {
      setNarrowSidebarOpen((open) => !open);
      return;
    }
    setSidebarCollapsed((collapsed) => {
      const next = !collapsed;
      try {
        localStorage.setItem("cf.workspace.sidebarCollapsed", next ? "1" : "0");
      } catch {
        // Storage is optional; the current workspace still responds immediately.
      }
      return next;
    });
  };
  // Steering covers both in-flight surfaces: a streaming chat turn and an
  // autonomous task run. Same keystroke, same meaning, same queue — which of
  // the two happens to be running is the framework's business, not the user's.
  const steerActive = activeDraft ? false : streaming || persistedRunActive;
  const guideNextStep = async (message: string) => {
    const trimmed = message.trim();
    if (!trimmed || activeDraft) return;
    await steerRun(trimmed);
  };
  // Double-click the session title (here or in the sidebar) to rename it inline.
  const [titleEditing, setTitleEditing] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const commitTitle = () => {
    const t = titleDraft.trim();
    setTitleEditing(false);
    if (activeSession && t && t !== activeSession.title) renameSession(activeSession.id, t);
  };
  // Leave title-edit mode if the workspace switches to another session, so a
  // pending edit can't blur-commit onto the wrong session.
  useEffect(() => {
    setTitleEditing(false);
    setTaskActivityOpen(Boolean(initialTaskLogId));
  }, [initialTaskLogId, sessionId]);
  // Git / environment panel — surface the (previously unwired) git UI in the
  // right column: a branch/status bar + slide-out Changes / History / PR panels.
  const [gitPanel, setGitPanel] = useState<"changes" | "history" | "remote" | null>(null);
  const gitBranch = useGitStore((s) => s.status?.branch ?? "");
  const activeCwd = activeSession?.cwd ?? activeDraft?.cwd ?? null;
  const draftProjects = useMemo(() => recentProjects(sessions ?? []), [sessions]);
  const projectTasks = useTasksStore((state) => state.tasks[sessionId]);
  const sessionTasks = projectTasks ?? [];
  const externalJobs = useMemo<ExternalJobState[]>(
    () =>
      sessionTasks.map((task) => ({
        id: task.id,
        status: task.status,
        startedAt: task.started_at ? Date.parse(task.started_at) : null,
        completedAt: task.completed_at ? Date.parse(task.completed_at) : null,
      })),
    [sessionTasks],
  );
  const [turnTimingProfile, setTurnTimingProfile] =
    useState<TurnTimingProfile | null>(null);
  const projectTaskCount = sessionTasks.length;
  const taskRunningCount = sessionTasks.filter((task) => task.status === "running").length;
  const taskPendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const failedTasks = sessionTasks.filter((task) => task.status === "failed");
  const taskFailedCount = failedTasks.length;
  const blockedTasks = failedTasks.filter((task) => task.failure_attribution?.repairable === false);
  const coreInputTasks = blockedTasks.filter((task) =>
    ["model-provider", "permission"].includes(task.failure_attribution?.kind ?? ""),
  );
  const taskBlockedCount = coreInputTasks.length;
  const taskProviderBlockedCount = coreInputTasks.filter((task) => task.failure_attribution?.kind === "model-provider").length;
  const taskActivityVisible = taskPendingCount + taskRunningCount + taskFailedCount > 0;
  const taskActivitySummary = useMemo(() => {
    if (taskBlockedCount > 0) {
      const title = taskProviderBlockedCount === taskBlockedCount
        ? "模型配置需要处理"
        : "后台任务需要一项授权";
      const label = taskProviderBlockedCount === taskBlockedCount
        ? `${taskBlockedCount} 个后台任务需要修复模型配置`
        : `${taskBlockedCount} 个后台任务等待必要授权`;
      return { count: taskBlockedCount, label, title, tone: "danger" as const, kind: "blocked" as const };
    }
    if (taskFailedCount > 0) {
      return { count: taskFailedCount, label: `系统正在恢复 ${taskFailedCount} 个后台任务`, title: `系统正在恢复 ${taskFailedCount} 个后台任务`, tone: "warning" as const, kind: "failed" as const };
    }
    if (taskRunningCount > 0) {
      return { count: taskRunningCount, label: `${taskRunningCount} 个后台任务正在运行`, title: `${taskRunningCount} 个后台任务正在运行`, tone: "progress" as const, kind: "running" as const };
    }
    if (taskPendingCount > 0) {
      return { count: taskPendingCount, label: `${taskPendingCount} 个后台任务等待调度`, title: `${taskPendingCount} 个后台任务等待调度`, tone: "neutral" as const, kind: "pending" as const };
    }
    return null;
  }, [taskBlockedCount, taskFailedCount, taskPendingCount, taskProviderBlockedCount, taskRunningCount]);
  const [taskActivityOpen, setTaskActivityOpen] = useState(Boolean(initialTaskLogId));
  const taskActivityButtonRef = useRef<HTMLButtonElement>(null);
  const taskActivityDialogRef = useRef<HTMLElement>(null);
  const closeTaskActivity = useCallback(() => {
    setTaskActivityOpen(false);
    requestAnimationFrame(() => taskActivityButtonRef.current?.focus());
  }, []);
  const [browserSessions, setBrowserSessions] = useState<WorkspaceBrowserSession[]>([]);
  const [browserLoadError, setBrowserLoadError] = useState<string | null>(null);
  const [browserPaneCollapsed, setBrowserPaneCollapsed] = useState(false);
  const [documentTabs, setDocumentTabs] = useState<DocumentTab[]>([]);
  const [activeRightTab, setActiveRightTab] = useState<string | null>(null);
  const browserPaneWidth = BROWSER_PANE_DEFAULT_WIDTH;
  const loadProjectTasks = useTasksStore((state) => state.loadTasks);
  const subscribeProjectTasks = useTasksStore((state) => state.subscribe);
  const isProjectSession = Boolean(
    activeSession && activeSession.kind !== "quick" && activeSession.kind !== "anonymous",
  );
  const activeBrowserSessions = useMemo(
    () =>
      (Array.isArray(browserSessions) ? browserSessions : []).filter(
        (session) =>
          !session.expired &&
          session.owner_session_id === sessionId &&
          session.status !== "closed",
      ),
    [browserSessions, sessionId],
  );
  const browserPaneOpen = (activeBrowserSessions.length > 0 || documentTabs.length > 0) && !browserPaneCollapsed;

  useEffect(() => {
    setBrowserPaneCollapsed(false);
  }, [sessionId]);

  useEffect(() => {
    let cancelled = false;
    const refreshBrowserSessions = async () => {
      try {
        const sessions = await invoke<WorkspaceBrowserSession[]>("list_browser_sessions");
        if (!cancelled) {
          setBrowserSessions(Array.isArray(sessions) ? sessions : []);
          setBrowserLoadError(null);
        }
      } catch (error) {
        if (!cancelled) setBrowserLoadError(String(error));
      }
    };
    void refreshBrowserSessions();
    const timer = window.setInterval(() => {
      void refreshBrowserSessions();
    }, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [sessionId]);

  const closeBrowserPaneSession = async (browserSessionId: string) => {
    await invoke("close_browser_session", { sessionId: browserSessionId });
    setBrowserSessions((sessions) =>
      sessions.filter((session) => session.session_id !== browserSessionId),
    );
  };

  useEffect(() => {
    let cancelled = false;
    if (!activeCwd || activeDraft) {
      setTurnTimingProfile(null);
      return () => {
        cancelled = true;
      };
    }
    void Promise.resolve(
      invoke<TurnTimingProfile>("get_turn_timing_profile", { cwd: activeCwd }),
    )
      .then((profile) => {
        if (!cancelled) setTurnTimingProfile(profile);
      })
      .catch(() => {
        if (!cancelled) setTurnTimingProfile(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activeCwd, activeDraft, messages.length]);

  // Subscribe as soon as a project Session opens. `delegate_tasks` may create
  // the first task from the chat agent, so waiting for a task panel to mount
  // would miss the event that makes that panel visible.
  useEffect(() => {
    if (!isProjectSession) return;
    void loadProjectTasks(sessionId);
    let unsubscribe: (() => void) | undefined;
    void subscribeProjectTasks(sessionId).then((stop) => { unsubscribe = stop; });
    return () => { unsubscribe?.(); };
  }, [isProjectSession, loadProjectTasks, sessionId, subscribeProjectTasks]);

  useEffect(() => {
    if (!taskActivityOpen) return;
    const dialog = taskActivityDialogRef.current;
    requestAnimationFrame(() => {
      dialog?.querySelector<HTMLElement>("[data-dialog-initial-focus]")?.focus();
    });
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeTaskActivity();
        return;
      }
      if (event.key !== "Tab" || !dialog) return;
      const focusable = Array.from(
        dialog.querySelectorAll<HTMLElement>(
          'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((element) => !element.hasAttribute("hidden"));
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
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [closeTaskActivity, taskActivityOpen]);

  return (
    <div className="h-full flex flex-col bg-surface-0">

      {/* ── Header ────────────────────────────────────────────────────────── */}
      <header aria-label="会话工具栏" className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-border/80 bg-surface-1/95 px-3 py-1.5">
        {!sidebarVisible && (
          <button
            type="button"
            onClick={toggleSidebar}
            className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[13px] font-medium text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
            title="展开会话侧栏"
            aria-label="展开会话侧栏"
            aria-expanded={false}
            aria-controls="workspace-session-sidebar"
          >
            <MessageSquare size={14} aria-hidden="true" />
            <span>会话</span>
          </button>
        )}
        <div className="flex-1 min-w-0">
          <div className="flex truncate text-[13px] font-semibold text-gray-200 items-center gap-2">
            {titleEditing && activeSession ? (
              <input
                autoFocus
                value={titleDraft}
                onChange={(e) => setTitleDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitTitle();
                  if (e.key === "Escape") setTitleEditing(false);
                }}
                onBlur={commitTitle}
                className="min-w-0 flex-1 rounded-md border border-accent/50 bg-surface-2 px-1.5 py-0.5 text-[13px] text-gray-100 outline-none"
              />
            ) : (
              <span
                className="truncate"
                title={isAnonymous ? undefined : "双击重命名"}
                onDoubleClick={() => {
                  if (!isAnonymous && activeSession) {
                    setTitleDraft(activeSession.title || "");
                    setTitleEditing(true);
                  }
                }}
              >
                {activeSession?.title || "新会话"}
              </span>
            )}
            {activeDraft ? (
              <span
                className="rounded-md bg-status-progress-soft px-1.5 py-0.5 text-[11px] font-normal text-status-progress"
                title="尚未创建记录；发送首条消息后生成"
              >
                草稿
              </span>
            ) : activeSession?.kind === "quick" ? (
              <span
                className="rounded-md bg-surface-3 px-1.5 py-0.5 text-[11px] font-normal text-gray-400"
                title="没有绑定项目的独立任务"
              >
                独立任务
              </span>
            ) : null}
            {isAnonymous && (
              <span
                className="inline-flex items-center gap-1 rounded-md bg-status-warning-soft px-1.5 py-0.5 text-[11px] font-normal text-status-warning"
                title="匿名会话：不落库、不计费、不进记忆/画像。离开即丢弃。"
              >
                <EyeOff size={9} />
                匿名
              </span>
            )}
          </div>
          <div className="hidden truncate font-mono text-[11px] text-gray-600 lg:block">
            {isAnonymous
              ? "无痕会话 · 不落库 · 不计费 · 不学习"
              : activeDraft
                ? (activeDraft.cwd ?? "发送首条消息后创建会话")
                : activeSession?.cwd}
          </div>
        </div>
        {isAnonymous && (
          <button
            onClick={() => {
              exitAnonymous();
              onNewConversation(null);
            }}
            className="flex min-h-8 items-center gap-1 rounded-lg border border-status-warning/30 bg-status-warning-soft px-2 text-[13px] text-status-warning transition-colors hover:brightness-95"
            title="退出匿名会话并丢弃其历史"
          >
            <EyeOff size={12} />
            退出匿名
          </button>
        )}
        {!activeDraft && <ModelPicker />}
        {/* Per-session reasoning override needs a DB row; anonymous chats use
            the global default, so the picker is hidden for them. */}
        {!isAnonymous && <ReasoningEffortPicker />}
        <PermissionModePicker />

        <div className="flex items-center gap-1.5">
          <GitStatusBar
            cwd={activeCwd}
            onOpenChanges={() => setGitPanel("changes")}
          />
          {!activeDraft && (
            <WorkspaceDeliveryStatus
              cwd={activeCwd}
              sessionId={sessionId}
              currentBranch={gitBranch}
              messages={messages}
            />
          )}
        </div>
        {isProjectSession && taskActivityVisible && taskActivitySummary && (
          <button
            ref={taskActivityButtonRef}
            type="button"
            onClick={() => setTaskActivityOpen(true)}
            aria-label={`打开任务活动：${taskActivitySummary.label}`}
            aria-expanded={taskActivityOpen}
            aria-controls="workspace-task-activity-dialog"
            className={`inline-flex h-8 min-w-8 items-center justify-center gap-1 rounded-lg px-2 text-[13px] font-medium transition-colors ${
              taskActivitySummary.tone === "danger"
                ? "bg-status-danger-soft text-status-danger hover:brightness-95"
                : taskActivitySummary.tone === "warning"
                  ? "bg-status-warning-soft text-status-warning hover:brightness-95"
                : taskActivitySummary.tone === "progress"
                  ? "bg-status-progress-soft text-status-progress hover:brightness-95"
                  : "text-gray-500 hover:bg-surface-3 hover:text-gray-300"
            }`}
            title={taskActivitySummary.title}
          >
            {taskActivitySummary.kind === "running" ? (
              <Loader2 size={12} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
            ) : taskActivitySummary.kind === "pending" ? (
              <Circle size={11} aria-hidden="true" />
            ) : taskActivitySummary.kind === "failed" ? (
              <AlertTriangle size={12} aria-hidden="true" />
            ) : (
              <XCircle size={12} aria-hidden="true" />
            )}
            <span>{taskActivitySummary.count}</span>
          </button>
        )}
        <button
          onClick={() => onOpenSettings()}
          className="flex h-8 w-8 items-center justify-center rounded-lg text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300"
          title="设置"
          aria-label="设置"
        >
          <SettingsIcon size={14} />
        </button>
      </header>

      {/* ── Body: 3 columns ──────────────────────────────────────────────── */}
      <div className="relative flex min-h-0 flex-1">

        {/* ─── Left: the rail owns collapse; the workspace header only restores it. ─── */}
        {narrowViewport && sidebarVisible && (
          <div
            aria-hidden="true"
            onMouseDown={() => setNarrowSidebarOpen(false)}
            className="absolute inset-0 z-30 bg-black/30"
          />
        )}
        {sidebarVisible && (
          <aside
            id="workspace-session-sidebar"
            aria-label="会话列表"
            className={`flex min-h-0 flex-col border-r border-border/80 bg-surface-1 ${
              narrowViewport
                ? "absolute inset-y-0 left-0 z-40 w-[min(272px,88vw)] shadow-2xl"
                : "w-[272px] shrink-0"
            }`}
          >
            <SessionSidebar
              currentSessionId={sessionId}
              onOpenSession={(id) => {
                if (narrowViewport) setNarrowSidebarOpen(false);
                onOpenSession(id);
              }}
              onNewConversation={(cwd) => {
                if (narrowViewport) setNarrowSidebarOpen(false);
                onNewConversation(cwd);
              }}
              onCollapse={toggleSidebar}
              collapseLabel={narrowViewport ? "关闭会话侧栏" : "收起会话侧栏"}
            />
          </aside>
        )}

        {/* ─── Center: conversation remains the primary surface. ─────────── */}
        <main
          aria-label="会话窗口"
          data-browser-pane={browserPaneOpen ? "open" : "closed"}
          className="flex min-w-0 flex-1 flex-col bg-surface-2"
        >
          <MessageList
            messages={messages}
            streaming={streaming}
            cwd={activeCwd}
            conversationKey={activeSession?.id ?? activeDraft?.id ?? sessionId}
            hasOlderHistory={hasOlderHistory}
            loadingOlderHistory={loadingOlderHistory}
            historyTruncated={historyTruncated}
            onLoadOlder={loadOlderMessages}
            onUsePrompt={(text) => setPendingInsert(text)}
            onOpenUsage={onOpenUsage}
            onOpenSession={onOpenSession}
            onPickProject={activeDraft ? setDraftProject : undefined}
            onOpenDocument={(path) => {
              const id = `document:${path}`;
              setDocumentTabs((tabs) => tabs.some((tab) => tab.id === id)
                ? tabs
                : [...tabs, { id, path, title: path.replace(/\\/g, "/").split("/").pop() ?? path }]);
              setActiveRightTab(id);
              setBrowserPaneCollapsed(false);
            }}
            timingProfile={turnTimingProfile}
            externalJobs={externalJobs}
          />
          <div data-testid="workspace-composer-shell" className="shrink-0 bg-surface-2 px-3 pb-3 pt-2">
            <div className="mx-auto w-full max-w-[920px] overflow-hidden rounded-2xl border border-border/80 bg-surface-2 shadow-lg">
              {queue.length > 0 && (
                <QueueBadge queue={queue} onRemove={removeFromQueue} />
              )}
              {/* A draft's two remaining choices — where it works, and whether it
                  leaves a trace — live inside the composer surface because they
                  stop being editable the moment the first message is sent. */}
              {activeDraft && (
                <DraftScopeBar
                  cwd={activeDraft.cwd}
                  anonymous={activeDraft.anonymous}
                  projects={draftProjects}
                  modelPicker={<ModelPicker portal prominent />}
                  onPickProject={setDraftProject}
                  onToggleAnonymous={setDraftAnonymous}
                />
              )}
              <ContextUsageBar
                sessionId={activeSession?.id}
                onOpenUsage={onOpenUsage ? () => onOpenUsage() : undefined}
              />
              <MessageInput
                key={activeSession?.id ?? activeDraft?.id ?? sessionId}
                initialHistory={messages.filter((m) => m.role === "user").map((m) => m.content)}
                onSend={(t) => void sendOrQueue(t)}
                onGuide={guideNextStep}
                onCancel={() => cancelStream()}
                streaming={streaming}
                guidanceActive={steerActive}
                disabled={!activeSession && !activeDraft}
                pendingInsert={pendingInsert}
                onInsertConsumed={() => setPendingInsert(undefined)}
                cwd={activeCwd}
              />
            </div>
          </div>
        </main>

        {browserPaneOpen && (
          <RightWorkbenchPane
            sessions={activeBrowserSessions}
            documents={documentTabs}
            activeTab={activeRightTab}
            widthPercent={browserPaneWidth}
            loadError={browserLoadError}
            cwd={activeCwd}
            onSelectTab={setActiveRightTab}
            onCloseDocument={(id) => {
              setDocumentTabs((tabs) => tabs.filter((tab) => tab.id !== id));
              setActiveRightTab((current) => current === id ? null : current);
            }}
            onCollapse={() => setBrowserPaneCollapsed(true)}
            onCloseSession={(browserSessionId) => void closeBrowserPaneSession(browserSessionId)}
          />
        )}

      </div>

      {taskActivityOpen && isProjectSession && projectTaskCount > 0 && (
        <div
          className="fixed inset-0 z-40 flex justify-end bg-black/20"
          role="presentation"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) closeTaskActivity();
          }}
        >
          <section
            id="workspace-task-activity-dialog"
            ref={taskActivityDialogRef}
            role="dialog"
            aria-label="任务活动"
            aria-modal="true"
            className="flex h-full w-[min(420px,92vw)] flex-col border-l border-border bg-surface-1 shadow-2xl"
          >
            <TasksColumn
              sessionId={sessionId}
              highlightedTaskId={initialTaskLogId}
              onOpenSettings={onOpenSettings}
              onClose={closeTaskActivity}
            />
          </section>
        </div>
      )}

      {/* ── Git / environment slide-out panels (opened from the status bar) ─ */}
      {gitPanel === "changes" && (
        <GitChangesPanel
          sessionId={activeDraft ? null : sessionId}
          onOpenHistory={() => setGitPanel("history")}
          onOpenRemote={() => setGitPanel("remote")}
          onClose={() => setGitPanel(null)}
        />
      )}
      {gitPanel === "history" && <GitHistoryPanel onClose={() => setGitPanel(null)} />}
      {gitPanel === "remote" && (
        <RemoteGitPanel
          currentBranch={gitBranch}
          onClose={() => setGitPanel(null)}
        />
      )}

      {/* ── Permission dialog overlay ───────────────────────────────────── */}
      {pendingPermission && (
        <PermissionDialog
          request={pendingPermission}
          trusted={(activeSession?.permission_mode ?? "standard") === "trusted"}
          onAllow={() => respondPermission(true)}
          onDeny={() => respondPermission(false)}
          onAllowFullAccess={() => respondPermission(true, { grantFullAccess: true })}
        />
      )}
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// EmbeddedBrowserPane
// ─────────────────────────────────────────────────────────────────────────────

function RightWorkbenchPane({ sessions, documents, activeTab, widthPercent, loadError, cwd, onSelectTab, onCloseDocument, onCollapse, onCloseSession }: {
  sessions: WorkspaceBrowserSession[];
  documents: DocumentTab[];
  activeTab: string | null;
  widthPercent: number;
  loadError: string | null;
  cwd?: string | null;
  onSelectTab: (id: string) => void;
  onCloseDocument: (id: string) => void;
  onCollapse: () => void;
  onCloseSession: (sessionId: string) => void;
}) {
  const activeBrowser = sessions[0];
  const selected = activeTab ?? (activeBrowser ? `browser:${activeBrowser.session_id}` : documents[0]?.id ?? null);
  const activeDocument = documents.find((tab) => tab.id === selected);
  return (
    <aside aria-label={sessions.length > 0 ? "内置浏览器" : "右侧工作区"} data-browser-width={String(widthPercent)} className="hidden min-h-0 shrink-0 flex-col border-l border-border bg-surface-1 xl:flex" style={{ width: `${widthPercent}%`, minWidth: 420, maxWidth: "50%" }}>
      <div className="flex min-w-0 items-center gap-1 border-b border-border px-2 py-1.5">
        <div role="tablist" aria-label="右侧工作区标签" className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {sessions.map((session) => {
            const id = `browser:${session.session_id}`;
            return <button key={id} type="button" role="tab" aria-selected={selected === id} onClick={() => onSelectTab(id)} className={`shrink-0 rounded px-2 py-1 text-[11px] ${selected === id ? "bg-surface-3 text-gray-200" : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"}`}><Globe2 size={11} className="mr-1 inline" />{sessionTitle(session)}</button>;
          })}
          {documents.map((tab) => <button key={tab.id} type="button" role="tab" aria-selected={selected === tab.id} onClick={() => onSelectTab(tab.id)} className={`flex max-w-40 shrink-0 items-center gap-1 rounded px-2 py-1 text-[11px] ${selected === tab.id ? "bg-surface-3 text-gray-200" : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"}`}><FileText size={11} /> <span className="truncate">{tab.title}</span></button>)}
        </div>
        <button type="button" onClick={onCollapse} className="shrink-0 rounded px-1.5 py-1 text-[11px] text-gray-500 hover:bg-surface-3 hover:text-gray-200" title="临时折叠右侧工作区">折叠</button>
      </div>
      {loadError && <div className="border-b border-red-500/20 bg-red-500/10 px-3 py-2 text-[11px] text-red-700 dark:text-red-300">浏览器状态读取失败：{loadError}</div>}
      {activeDocument ? (
        <DocumentPreview tab={activeDocument} cwd={cwd} onClose={() => onCloseDocument(activeDocument.id)} />
      ) : activeBrowser ? (
        <EmbeddedBrowserPane sessions={sessions} widthPercent={widthPercent} loadError={loadError} onCollapse={onCollapse} onCloseSession={onCloseSession} hideHeader />
      ) : null}
    </aside>
  );
}

// EmbeddedBrowserPane remains the isolated native-webview renderer. Its header
// is supplied by RightWorkbenchPane so browser and document tabs share one rail.
function EmbeddedBrowserPane({ sessions, widthPercent, loadError, onCollapse, onCloseSession, hideHeader = false }: {
  sessions: WorkspaceBrowserSession[];
  widthPercent: number;
  loadError: string | null;
  onCollapse: () => void;
  onCloseSession: (sessionId: string) => void;
  hideHeader?: boolean;
}) {
  const active = sessions[0];
  if (!active) return null;
  const host = sessionHost(active);
  const title = sessionTitle(active);
  const safeUrl = active.pane_url && /^https?:\/\//i.test(active.pane_url) ? active.pane_url : null;
  const viewportRef = useRef<HTMLDivElement>(null);

  const syncNativeWebView = useCallback(() => {
    if (!safeUrl || !viewportRef.current) return;
    const rect = viewportRef.current.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    void invoke("embedded_browser_mount", {
      sessionId: active.session_id,
      url: safeUrl,
      bounds: { x: rect.left, y: rect.top, width: rect.width, height: rect.height },
    });
  }, [active.session_id, safeUrl]);

  useEffect(() => {
    syncNativeWebView();
    if (!safeUrl) return;
    const onResize = () => syncNativeWebView();
    window.addEventListener("resize", onResize);
    const timer = window.setInterval(syncNativeWebView, 1000);
    return () => {
      window.removeEventListener("resize", onResize);
      window.clearInterval(timer);
      void invoke("embedded_browser_unmount", { sessionId: active.session_id });
    };
  }, [active.session_id, safeUrl, syncNativeWebView]);

  return (
    <div
      aria-label={hideHeader ? undefined : "内置浏览器"}
      data-browser-width={String(widthPercent)}
      className="hidden min-h-0 shrink-0 flex-col border-l border-border bg-surface-1 xl:flex"
      style={{ width: `${widthPercent}%`, minWidth: 420, maxWidth: "50%" }}
    >
      {hideHeader && <div className="flex items-center gap-2 border-b border-border px-3 py-2"><Globe2 size={14} className="shrink-0 text-accent" aria-hidden="true" /><div className="min-w-0 flex-1"><div className="truncate text-xs font-medium text-gray-200">{host}</div><div className="truncate text-[11px] text-gray-500">{title}</div></div><button type="button" onClick={() => onCloseSession(active.session_id)} className="rounded bg-red-500/10 px-1.5 py-1 text-[11px] text-red-700 dark:text-red-300" aria-label="结束浏览器">结束</button></div>}
      {!hideHeader && <div className="flex items-center gap-2 border-b border-border px-3 py-2">
        <Globe2 size={14} className="shrink-0 text-accent" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-medium text-gray-200">{host}</div>
          <div className="truncate text-[11px] text-gray-500">{title}</div>
        </div>
        {sessions.length > 1 && <span className="rounded-full bg-accent/10 px-1.5 py-0.5 text-[11px] text-accent">{sessions.length}</span>}
        <button type="button" onClick={onCollapse} className="rounded px-1.5 py-1 text-[11px] text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200" title="临时折叠浏览器">折叠</button>
        <button type="button" onClick={() => onCloseSession(active.session_id)} className="rounded bg-red-500/10 px-1.5 py-1 text-[11px] text-red-700 hover:bg-red-500/20 dark:text-red-300" aria-label="结束浏览器" title="结束当前会话的受管浏览器">结束</button>
      </div>}
      {loadError && (
        <div className="border-b border-red-500/20 bg-red-500/10 px-3 py-2 text-[11px] text-red-700 dark:text-red-300">
          浏览器状态读取失败：{loadError}
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col bg-surface-0">
        {safeUrl ? (
          <div
            ref={viewportRef}
            role="application"
            aria-label={`网页视图：${title}`}
            className="relative min-h-0 flex-1 overflow-hidden bg-white"
            data-codefactory-browser-session={active.session_id}
          >
            <div className="absolute inset-0 flex items-center justify-center bg-white text-xs text-gray-600">
              正在内置浏览器中打开 {host}
            </div>
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center p-6 text-center text-xs text-gray-500">
            <div>
              <ExternalLink size={18} className="mx-auto mb-2 text-gray-600" aria-hidden="true" />
              <p className="font-medium text-gray-300">受管浏览器已连接</p>
              <p className="mt-1">等待页面地址；Agent 可继续读取和操作该会话。</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// TasksColumn
// ─────────────────────────────────────────────────────────────────────────────

// Task decomposition is internal to the conversation; this panel only renders
// the execution detail after the agent has delegated work.
function TasksColumn({ sessionId, highlightedTaskId, onOpenSettings, onClose }: {
  sessionId: string;
  highlightedTaskId?: string | null;
  onOpenSettings: (tab: "endpoints") => void;
  onClose: () => void;
}) {
  const { tasks, running, cancel } = useTasksStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const isRunning = running[sessionId] ?? false;
  const pendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const runningCount = sessionTasks.filter((task) => task.status === "running").length;
  const completedCount = sessionTasks.filter((task) => task.status === "completed").length;
  const failedTasks = sessionTasks.filter((task) => task.status === "failed");
  const blockedTasks = failedTasks.filter((task) => task.failure_attribution?.repairable === false);
  const providerBlockedTasks = blockedTasks.filter((task) => task.failure_attribution?.kind === "model-provider");
  const permissionBlockedTasks = blockedTasks.filter((task) => task.failure_attribution?.kind === "permission");
  const systemRecoveryTasks = failedTasks.filter((task) => !["model-provider", "permission"].includes(task.failure_attribution?.kind ?? "unknown"));
  const [startError, setStartError] = useState<string | null>(null);

  const handleCancel = async () => {
    try { await cancel(sessionId); } catch (error) { setStartError(String(error)); }
  };
  // Task decomposition is an implementation detail of the conversation.
  if (sessionTasks.length === 0) return null;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between border-b border-border px-4">
        <div className="min-w-0">
          <h2 className="text-sm font-medium text-gray-200">任务活动</h2>
          <p className="text-[11px] text-gray-600">后台步骤、验收结果与恢复操作</p>
        </div>
        <button data-dialog-initial-focus type="button" onClick={onClose} aria-label="关闭任务活动" className="flex h-8 w-8 items-center justify-center rounded-lg text-gray-500 hover:bg-surface-3 hover:text-gray-200">
          <X size={15} />
        </button>
      </div>
      <div className="flex shrink-0 flex-col gap-2 border-b border-border px-4 py-3">
        <div className="flex flex-wrap items-center gap-2 text-[12px] text-gray-600">
          <span>已完成 {completedCount}</span><span>等待调度 {pendingCount}</span>
          {runningCount > 0 && <span className="text-status-progress">执行中 {runningCount}</span>}
          {failedTasks.length > 0 && <span className="text-status-warning">系统恢复 {failedTasks.length}</span>}
          {providerBlockedTasks.length > 0 && <span className="text-status-danger">模型配置 {providerBlockedTasks.length}</span>}
          {permissionBlockedTasks.length > 0 && <span className="text-status-danger">权限配置 {permissionBlockedTasks.length}</span>}
          {systemRecoveryTasks.length > 0 && <span className="text-status-warning">正在诊断 {systemRecoveryTasks.length}</span>}
        </div>
        {!isRunning && pendingCount > 0 && (
          <p className={`text-[12px] leading-5 ${failedTasks.length > 0 ? "text-status-warning" : "text-gray-500"}`}>
            {failedTasks.length > 0
              ? `系统正在处理失败项，并会自动续接剩余 ${pendingCount} 项。`
              : `已委派，还有 ${pendingCount} 项等待后台调度。`}
          </p>
        )}
        <div className="flex flex-wrap items-center gap-1">
          {isRunning ? (
            <button onClick={() => void handleCancel()} className="flex min-h-8 items-center gap-1.5 rounded-lg bg-status-danger-soft px-2.5 text-[13px] text-status-danger hover:brightness-95"><Square size={11} />停止</button>
          ) : pendingCount > 0 && failedTasks.length === 0 ? (
            <span className="text-[12px] leading-5 text-gray-500">任务已委派，系统会持续调度并自动诊断恢复，无需手动重试。</span>
          ) : null}
          {!isRunning && providerBlockedTasks.length > 0 && (
            <button onClick={() => onOpenSettings("endpoints")} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">打开模型设置</button>
          )}
          {!isRunning && permissionBlockedTasks.length > 0 && (
            <button onClick={() => onOpenSettings("endpoints")} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">调整会话权限</button>
          )}
          {!isRunning && systemRecoveryTasks.length > 0 && (
            <span role="status" className="text-[12px] leading-5 text-status-warning">
              CodeFactory 正在诊断并自动续接，无需手动处理。
            </span>
          )}
        </div>
      </div>
      {startError && <div className="border-b border-status-danger/20 bg-status-danger-soft px-4 py-2 text-[12px] text-status-danger">{startError}</div>}
      <div className="flex-1 overflow-y-auto p-3"><ul className="space-y-1">{buildTaskTree(sessionTasks).map(({ task, depth }) => <TaskRow key={task.id} task={task} depth={depth} highlighted={task.id === highlightedTaskId} />)}</ul></div>
    </div>
  );
}

function buildTaskTree(tasks: TaskRun[]): { task: TaskRun; depth: number }[] {
  const byParent = new Map<string | null, TaskRun[]>();
  for (const t of tasks) {
    const k = t.parent_task_id;
    if (!byParent.has(k)) byParent.set(k, []);
    byParent.get(k)!.push(t);
  }
  const out: { task: TaskRun; depth: number }[] = [];
  const walk = (parentId: string | null, depth: number) => {
    for (const t of byParent.get(parentId) ?? []) {
      out.push({ task: t, depth });
      walk(t.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

function TaskRow({ task, depth, highlighted = false }: { task: TaskRun; depth: number; highlighted?: boolean }) {
  const Icon = statusIcon(task.status);
  // Surface acceptance-criteria verification right here in the task tree — the
  // "did it actually pass?" proof that previously only lived in evidence packs.
  const verif = parseVerification(task.verification_results);
  const summary = verificationSummary(task.verification_results);
  const [verifOpen, setVerifOpen] = useState(false);
  return (
    <li
      id={`task-log-${task.id}`}
      aria-current={highlighted ? "true" : undefined}
      className={`rounded-lg transition-colors ${highlighted ? "bg-status-progress-soft ring-1 ring-status-progress/30" : "hover:bg-surface-3"}`}
      style={{ paddingLeft: `${0.375 + depth * 0.875}rem` }}
    >
      <div className="group flex items-start gap-2 px-1.5 py-1">
        <Icon
          size={13}
          className={`mt-1 shrink-0 ${statusColor(task.status)} ${
            task.status === "running" ? "animate-spin motion-reduce:animate-none" : ""
          }`}
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-start gap-1.5">
            <span className="block flex-1 text-[13px] leading-5 text-gray-300 line-clamp-2">
              {task.title}
            </span>
            {summary && (
              <button
                onClick={() => setVerifOpen((v) => !v)}
                title={`验收验证：${summary.passed}/${summary.total} 通过（点击展开逐条）`}
                className={`mt-0.5 inline-flex min-h-7 shrink-0 items-center gap-1 rounded-md px-1.5 text-[11px] transition-colors hover:bg-surface-2 ${
                  summary.allPassed ? "text-status-success" : "text-status-danger"
                }`}
              >
                {summary.allPassed ? <CheckCircle2 size={10} /> : <XCircle size={10} />}
                {summary.passed}/{summary.total}
              </button>
            )}
          </div>
          {task.spec_title && (
            <div
              className="mt-0.5 flex items-center gap-1 text-[11px] text-status-progress"
              title={`来自规范《${task.spec_title}》`}
            >
              <BookOpen size={9} className="shrink-0" />
              <span className="truncate">规范《{task.spec_title}》</span>
            </div>
          )}
          {task.failure_attribution && (
            <div
              data-status-tone={
                task.failure_attribution.repairable === false ? "danger" : "warning"
              }
              className={`mt-1 flex items-start gap-1.5 rounded-md px-2 py-1 text-[11px] leading-4 ${
                task.failure_attribution.repairable === false
                  ? "bg-status-danger-soft text-status-danger"
                  : "bg-status-warning-soft text-status-warning"
              }`}
              title={task.failure_attribution.summary}
            >
              <AlertTriangle size={9} className="mt-0.5 shrink-0" />
              <span className="shrink-0 font-medium">
                {["model-provider", "permission"].includes(task.failure_attribution.kind)
                  ? task.failure_attribution.label
                  : "系统正在恢复"}
              </span>
              <span className="min-w-0 truncate opacity-80">
                {["model-provider", "permission"].includes(task.failure_attribution.kind)
                  ? "完成必要输入后将自动续接"
                  : "正在诊断、修复并重新验证"}
              </span>
            </div>
          )}
          {task.attempts && task.attempts.length > 0 && (
            <div className="mt-0.5 text-[11px] text-gray-600">
              {task.attempts.length} 次执行记录
              {task.attempts[task.attempts.length - 1]?.status === "failed" && (
                <span className="ml-1 text-status-warning">最近一次失败</span>
              )}
            </div>
          )}
       </div>
      </div>
      {verifOpen && verif && (
        <div className="mb-1 ml-5 mr-1 space-y-0.5">
          {verif.map((r, i) => (
            <VerifCheckRow key={i} result={r} />
          ))}
        </div>
      )}
    </li>
  );
}

/** One acceptance-criterion check: ✓/✗ + name + duration, click to reveal the
 *  captured output (only when there is any). */
function VerifCheckRow({ result }: { result: VerificationResult }) {
  const [showOutput, setShowOutput] = useState(false);
  const hasOutput = result.output.trim().length > 0;
  return (
    <div className="rounded bg-surface-2">
      <div
        className={`flex items-center gap-1.5 px-1.5 py-0.5 ${hasOutput ? "cursor-pointer" : ""}`}
        role={hasOutput ? "button" : undefined}
        tabIndex={hasOutput ? 0 : undefined}
        aria-expanded={hasOutput ? showOutput : undefined}
        aria-label={hasOutput ? `${result.check}，查看验收输出` : undefined}
        onClick={() => hasOutput && setShowOutput((v) => !v)}
        onKeyDown={(event) => {
          if (hasOutput && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            setShowOutput((value) => !value);
          }
        }}
      >
        {result.passed ? (
          <CheckCircle2 size={10} className="shrink-0 text-status-success" />
        ) : (
          <XCircle size={10} className="shrink-0 text-status-danger" />
        )}
        <span className="flex-1 truncate text-[12px] text-gray-400">{result.check}</span>
        <span className="text-[11px] text-gray-600">{result.duration_ms}ms</span>
      </div>
      {showOutput && hasOutput && (
        <pre className="max-h-32 overflow-y-auto whitespace-pre-wrap px-1.5 pb-1 font-mono text-[11px] text-gray-500">
          {result.output}
        </pre>
      )}
    </div>
  );
}

function statusIcon(status: string) {
  switch (status) {
    case "completed":  return CheckCircle2;
    case "running":    return Loader2;
    case "failed":     return XCircle;
    default:           return Circle;
  }
}

function statusColor(status: string): string {
  switch (status) {
    case "completed":  return "text-status-success";
    case "running":    return "text-status-progress";
    case "failed":     return "text-status-danger";
    default:           return "text-gray-600";
  }
}
