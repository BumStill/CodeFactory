// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  BookOpen,
  Settings as SettingsIcon,
  MessageSquare,
  RefreshCw,
  Circle,
  CheckCircle2,
  Loader2,
  XCircle,
  Square,
  EyeOff,
  ListTodo,
  X,
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
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
import type { TaskRun, VerificationResult } from "../../lib/tauri";
import type { ExternalJobState, TurnTimingProfile } from "../../lib/chatPlan";
import { parseVerification, verificationSummary } from "../../lib/verification";

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
  const taskBlockedCount = blockedTasks.length;
  const taskProviderBlockedCount = blockedTasks.filter((task) => task.failure_attribution?.kind === "model-provider").length;
  const taskActivityVisible = taskPendingCount + taskRunningCount + taskFailedCount > 0;
  const [taskActivityOpen, setTaskActivityOpen] = useState(Boolean(initialTaskLogId));
  const taskActivityButtonRef = useRef<HTMLButtonElement>(null);
  const taskActivityDialogRef = useRef<HTMLElement>(null);
  const closeTaskActivity = useCallback(() => {
    setTaskActivityOpen(false);
    requestAnimationFrame(() => taskActivityButtonRef.current?.focus());
  }, []);
  const loadProjectTasks = useTasksStore((state) => state.loadTasks);
  const subscribeProjectTasks = useTasksStore((state) => state.subscribe);
  const isProjectSession = Boolean(
    activeSession && activeSession.kind !== "quick" && activeSession.kind !== "anonymous",
  );

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
        <ModelPicker />
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
        {isProjectSession && taskActivityVisible && (
          <button
            ref={taskActivityButtonRef}
            type="button"
            onClick={() => setTaskActivityOpen(true)}
            aria-label="打开任务活动"
            aria-expanded={taskActivityOpen}
            aria-controls="workspace-task-activity-dialog"
            className={`inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[13px] transition-colors ${
              taskBlockedCount > 0
                ? "bg-status-danger-soft text-status-danger hover:brightness-95"
                : taskFailedCount > 0
                  ? "bg-status-warning-soft text-status-warning hover:brightness-95"
                : taskRunningCount > 0
                  ? "bg-status-progress-soft text-status-progress hover:brightness-95"
                  : "text-gray-500 hover:bg-surface-3 hover:text-gray-300"
            }`}
            title="查看后台任务、验收结果和恢复操作"
          >
            <ListTodo size={12} />
            <span>
              {taskBlockedCount > 0
                ? (taskProviderBlockedCount === taskBlockedCount ? "模型配置待修复" : "需要你处理")
                : taskFailedCount > 0
                  ? `${taskFailedCount} 个步骤失败`
                  : taskRunningCount > 0
                    ? `正在执行 ${taskRunningCount}`
                    : taskPendingCount > 0
                      ? `待执行 ${taskPendingCount}`
                      : `任务 ${projectTaskCount}`}
            </span>
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
        <main aria-label="会话窗口" className="flex min-w-0 flex-1 flex-col bg-surface-2">
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
            timingProfile={turnTimingProfile}
            externalJobs={externalJobs}
          />
          <div data-testid="workspace-composer-shell" className="shrink-0 bg-surface-1 px-3 pb-3 pt-2">
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
              onRequestRepair={(task) => {
                const evidence = task.error || task.failure_attribution?.summary || "未提供错误详情";
                setPendingInsert(`请继续处理失败任务「${task.title}」。先诊断并修复根因，再重试该任务。\n\n失败证据：${evidence}`);
                closeTaskActivity();
              }}
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
// TasksColumn
// ─────────────────────────────────────────────────────────────────────────────

// Task decomposition is internal to the conversation; this panel only renders
// the execution detail after the agent has delegated work.
function TasksColumn({ sessionId, highlightedTaskId, onOpenSettings, onRequestRepair, onClose }: {
  sessionId: string;
  highlightedTaskId?: string | null;
  onOpenSettings: (tab: "endpoints") => void;
  onRequestRepair: (task: TaskRun) => void;
  onClose: () => void;
}) {
  const { tasks, running, start, cancel, retryFailedTasks, retryTasks } = useTasksStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const isRunning = running[sessionId] ?? false;
  const pendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const runningCount = sessionTasks.filter((task) => task.status === "running").length;
  const completedCount = sessionTasks.filter((task) => task.status === "completed").length;
  const failedTasks = sessionTasks.filter((task) => task.status === "failed");
  const repairableFailedCount = failedTasks.filter(
    (task) => task.failure_attribution?.repairable,
  ).length;
  const blockedTasks = failedTasks.filter((task) => task.failure_attribution?.repairable === false);
  const providerBlockedTasks = blockedTasks.filter((task) => task.failure_attribution?.kind === "model-provider");
  const permissionBlockedTasks = blockedTasks.filter((task) => task.failure_attribution?.kind === "permission");
  const conversationBlockedTasks = blockedTasks.filter((task) => !["model-provider", "permission"].includes(task.failure_attribution?.kind ?? "unknown"));
  const [startError, setStartError] = useState<string | null>(null);
  const [repairBusy, setRepairBusy] = useState(false);
  const [blockedRetryBusy, setBlockedRetryBusy] = useState(false);

  const handleCancel = async () => {
    try { await cancel(sessionId); } catch (error) { setStartError(String(error)); }
  };
  const handleRetryBlocked = async (selected: TaskRun[]) => {
    if (blockedRetryBusy || isRunning || selected.length === 0) return;
    setBlockedRetryBusy(true);
    setStartError(null);
    try {
      const retried = await retryTasks(sessionId, selected.map((task) => task.id));
      if (retried > 0) await start(sessionId);
    } catch (error) { setStartError(String(error)); }
    finally { setBlockedRetryBusy(false); }
  };

  const handleRepairFailed = async () => {
    if (repairBusy || isRunning || repairableFailedCount === 0) return;
    setRepairBusy(true);
    setStartError(null);
    try {
      const retried = await retryFailedTasks(sessionId);
      if (retried > 0) await start(sessionId);
    } catch (error) { setStartError(String(error)); }
    finally { setRepairBusy(false); }
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
          <span>已完成 {completedCount}</span><span>待执行 {pendingCount}</span>
          {runningCount > 0 && <span className="text-status-progress">执行中 {runningCount}</span>}
          {repairableFailedCount > 0 && <span className="text-status-warning">可重试 {repairableFailedCount}</span>}
          {providerBlockedTasks.length > 0 && <span className="text-status-danger">模型配置 {providerBlockedTasks.length}</span>}
          {permissionBlockedTasks.length > 0 && <span className="text-status-danger">权限配置 {permissionBlockedTasks.length}</span>}
          {conversationBlockedTasks.length > 0 && <span className="text-status-danger">需要你 {conversationBlockedTasks.length}</span>}
        </div>
        {!isRunning && pendingCount > 0 && (
          <p className={`text-[12px] leading-5 ${failedTasks.length > 0 ? "text-status-warning" : "text-gray-500"}`}>
            {failedTasks.length > 0
              ? `先处理失败项，再继续剩余 ${pendingCount} 项。`
              : `执行已暂停，还有 ${pendingCount} 项等待执行。`}
          </p>
        )}
        <div className="flex flex-wrap items-center gap-1">
          {isRunning ? (
            <button onClick={() => void handleCancel()} className="flex min-h-8 items-center gap-1.5 rounded-lg bg-status-danger-soft px-2.5 text-[13px] text-status-danger hover:brightness-95"><Square size={11} />停止</button>
          ) : pendingCount > 0 && failedTasks.length === 0 ? (
            <span className="text-[12px] leading-5 text-gray-500">任务已委派，由后台调度器自动执行；若长时间未开始请检查模型配置或重试委派。</span>
          ) : null}
          {!isRunning && repairableFailedCount > 0 && (
            <button onClick={() => void handleRepairFailed()} disabled={repairBusy} className="flex min-h-8 items-center gap-1.5 rounded-lg bg-status-warning-soft px-2.5 text-[13px] text-status-warning disabled:opacity-40" title="重试可自动修复的失败步骤">
              {repairBusy ? <Loader2 size={11} className="animate-spin motion-reduce:animate-none" /> : <RefreshCw size={11} />}重试失败步骤
            </button>
          )}
          {!isRunning && providerBlockedTasks.length > 0 && (
            <><button onClick={() => onOpenSettings("endpoints")} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">打开模型设置</button>
            <button aria-label={`已修复，重试 ${providerBlockedTasks.length} 项`} title={`重试：${providerBlockedTasks.map((task) => task.title).join("、")}`} onClick={() => void handleRetryBlocked(providerBlockedTasks)} disabled={blockedRetryBusy} className="flex min-h-8 items-center gap-1.5 rounded-lg border border-border bg-surface-2 px-2.5 text-[13px] text-gray-300 disabled:opacity-40"><RefreshCw size={11} />已修复，重试 {providerBlockedTasks.length} 项</button></>
          )}
          {!isRunning && permissionBlockedTasks.length > 0 && (
            <><button onClick={() => onOpenSettings("endpoints")} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">调整会话权限</button>
            <button onClick={() => void handleRetryBlocked(permissionBlockedTasks)} disabled={blockedRetryBusy} className="min-h-8 rounded-lg border border-border bg-surface-2 px-2.5 text-[13px] text-gray-300 disabled:opacity-40">已授权，重试 {permissionBlockedTasks.length} 项</button></>
          )}
          {!isRunning && conversationBlockedTasks.length > 0 && (
            <button onClick={() => onRequestRepair(conversationBlockedTasks[0])} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">回到对话处理</button>
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
              title={`${task.failure_attribution.summary}\n下一步：${task.failure_attribution.next_action}`}
            >
              <AlertTriangle size={9} className="mt-0.5 shrink-0" />
              <span className="shrink-0 font-medium">{task.failure_attribution.label}</span>
              <span className="min-w-0 truncate opacity-80">
                {task.failure_attribution.next_action}
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
