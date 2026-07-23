// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  BookOpen,
  Settings as SettingsIcon,
  PanelLeftClose,
  PanelLeftOpen,
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
import { ModelPicker } from "../../components/ModelPicker";
import { ReasoningEffortPicker } from "../../components/ReasoningEffortPicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { ContextUsageBar } from "../../components/ContextUsageBar";
import { GitStatusBar } from "../../components/GitStatusBar";
import { WorkspaceDeliveryStatus } from "../../components/WorkspaceDeliveryStatus";
import { GitChangesPanel } from "../../components/GitChangesPanel";
import { GitHistoryPanel } from "../../components/GitHistoryPanel";
import { RemoteGitPanel } from "../../components/RemoteGitPanel";
import { useGitStore } from "../../stores/git";
import { invoke } from "../../lib/tauri";
import { useChatStore, activeRuntime } from "../../stores/chat";
import { QueueBadge } from "../../components/QueueBadge";
import { useSettingsStore } from "../../stores/settings";
import { useTasksStore } from "../../stores/tasks";
import type { TaskRun, VerificationResult } from "../../lib/tauri";
import { parseVerification, verificationSummary } from "../../lib/verification";

interface WorkspacePageProps {
  sessionId: string;
  /** Start another empty quick draft; kept under the legacy prop name so
   * existing embedders remain source-compatible while Home no longer exists. */
  onBackHome: () => void;
  onOpenSettings: (tab?: "capabilities" | "endpoints" | "permissions") => void;
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
  onBackHome,
  onOpenSettings,
  onOpenUsage,
  onOpenSession,
  initialTaskLogId,
}: WorkspacePageProps) {
  const {
    activeSession, draftSession,
    selectSession, sendOrQueue, cancelStream, removeFromQueue,
    respondPermission, exitAnonymous, renameSession,
  } = useChatStore();
  const activeDraft = draftSession?.id === sessionId ? draftSession : null;
  // Per-session chat state for the ACTIVE session. Background sessions keep
  // streaming into their own buckets; here we render the active one's slice.
  const { messages, streaming, queue, pendingPermission } = useChatStore(activeRuntime);
  const isAnonymous = activeSession?.kind === "anonymous";
  const settings = useSettingsStore((state) => state.settings);
  const persistedRunActive = useTasksStore((state) => state.running[sessionId] ?? false);
  const autonomousRunActive = activeDraft ? false : persistedRunActive;
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try {
      return localStorage.getItem("cf.workspace.sidebarCollapsed") === "1";
    } catch {
      return false;
    }
  });
  const toggleSidebar = () => {
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
  const guideNextStep = async (message: string) => {
    const trimmed = message.trim();
    if (!trimmed || activeDraft) return;
    await invoke("queue_interjection", { sessionId, message: trimmed });
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
  const projectTasks = useTasksStore((state) => state.tasks[sessionId]);
  const sessionTasks = projectTasks ?? [];
  const projectTaskCount = sessionTasks.length;
  const taskRunningCount = sessionTasks.filter((task) => task.status === "running").length;
  const taskPendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const failedTasks = sessionTasks.filter((task) => task.status === "failed");
  const taskFailedCount = failedTasks.length;
  const blockedTasks = failedTasks.filter((task) => task.failure_attribution?.repairable === false);
  const taskBlockedCount = blockedTasks.length;
  const taskProviderBlockedCount = blockedTasks.filter((task) => task.failure_attribution?.kind === "model-provider").length;
  const taskActivityVisible = taskRunningCount + taskPendingCount + taskFailedCount > 0;
  const [taskActivityOpen, setTaskActivityOpen] = useState(Boolean(initialTaskLogId));
  const taskActivityButtonRef = useRef<HTMLButtonElement>(null);
  const closeTaskActivity = () => {
    setTaskActivityOpen(false);
    requestAnimationFrame(() => taskActivityButtonRef.current?.focus());
  };
  const loadProjectTasks = useTasksStore((state) => state.loadTasks);
  const subscribeProjectTasks = useTasksStore((state) => state.subscribe);
  const isProjectSession = Boolean(
    activeSession && activeSession.kind !== "quick" && activeSession.kind !== "anonymous",
  );

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
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeTaskActivity();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [taskActivityOpen]);

  useEffect(() => {
    // Draft IDs are reused by materialization. Once the first message creates
    // the real session, activeSession already contains that same ID; calling
    // selectSession here would reload get_messages and race the live stream.
    if (activeDraft || activeSession?.id === sessionId) return;
    void selectSession(sessionId);
  }, [activeDraft, activeSession?.id, selectSession, sessionId]);

  return (
    <div className="h-full flex flex-col bg-surface-0">

      {/* ── Header ────────────────────────────────────────────────────────── */}
      <header aria-label="会话工具栏" className="flex items-center gap-3 px-3 py-1.5 border-b border-border bg-surface-1 shrink-0">
        <button
          onClick={toggleSidebar}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title={sidebarCollapsed ? "展开会话侧栏" : "收起会话侧栏"}
          aria-label={sidebarCollapsed ? "展开会话侧栏" : "收起会话侧栏"}
          aria-expanded={!sidebarCollapsed}
          aria-controls="workspace-session-sidebar"
        >
          {sidebarCollapsed ? <PanelLeftOpen size={14} /> : <PanelLeftClose size={14} />}
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium text-gray-200 truncate flex items-center gap-2">
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
                className="min-w-0 flex-1 rounded border border-accent/50 bg-surface-3 px-1.5 py-0.5 text-sm text-gray-100 outline-none"
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
                {activeSession?.title || (activeDraft?.mode === "project" ? "新项目" : "新对话")}
              </span>
            )}
            {(activeSession?.kind === "quick" || activeDraft?.mode === "quick") && (
              <span
                className="text-[9px] px-1.5 py-0.5 rounded bg-accent/15 text-accent font-normal"
                title={activeDraft ? "尚未创建记录；发送首条消息后生成" : "一次性助手会话，不会出现在「最近项目」"}
              >
                {activeDraft ? "草稿" : "Quick"}
              </span>
            )}
            {isAnonymous && (
              <span
                className="inline-flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-600 dark:text-amber-400 font-normal"
                title="匿名会话：不落库、不计费、不进记忆/画像。离开即丢弃。"
              >
                <EyeOff size={9} />
                匿名
              </span>
            )}
          </div>
          <div className="text-[10px] text-gray-600 font-mono truncate">
            {isAnonymous
              ? "无痕会话 · 不落库 · 不计费 · 不学习"
              : activeDraft
                ? (activeDraft.mode === "project" ? activeDraft.cwd : "发送首条消息后创建会话")
                : activeSession?.cwd}
          </div>
        </div>
        {isAnonymous && (
          <button
            onClick={() => {
              exitAnonymous();
              onBackHome();
            }}
            className="flex items-center gap-1 rounded border border-amber-500/40 bg-amber-500/10 px-2 py-1 text-xs text-amber-600 transition-colors hover:bg-amber-500/20 dark:text-amber-400"
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
            className={`inline-flex h-7 items-center gap-1 rounded-md px-2 text-[11px] transition-colors ${
              taskFailedCount > 0
                ? "bg-red-500/10 text-red-700 hover:bg-red-500/15 dark:text-red-300"
                : taskRunningCount > 0
                  ? "bg-accent/10 text-accent hover:bg-accent/15"
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
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="设置"
          aria-label="设置"
        >
          <SettingsIcon size={14} />
        </button>
      </header>

      {/* ── Body: 3 columns ──────────────────────────────────────────────── */}
      <div className="flex-1 flex min-h-0">

        {/* ─── Left: collapsible session rail; the header control always restores it. ─── */}
        {!sidebarCollapsed && (
          <aside id="workspace-session-sidebar" aria-label="会话列表" className="w-64 shrink-0 border-r border-border bg-surface-1 flex flex-col min-h-0">
            <SessionSidebar currentSessionId={sessionId} onOpenSession={onOpenSession} />
          </aside>
        )}

        {/* ─── Center: conversation remains the primary surface. ─────────── */}
        <main aria-label="会话窗口" className="flex-1 flex flex-col min-w-0">
          <MessageList
            messages={messages}
            streaming={streaming}
            cwd={activeCwd}
            onUsePrompt={(text) => setPendingInsert(text)}
            onOpenUsage={onOpenUsage}
          />
          <ContextUsageBar sessionId={activeSession?.id} />
          {queue.length > 0 && (
            <QueueBadge queue={queue} onRemove={removeFromQueue} />
          )}
          <MessageInput
            key={activeSession?.id ?? activeDraft?.id ?? sessionId}
            initialHistory={messages.filter((m) => m.role === "user").map((m) => m.content)}
            onSend={(t) => void sendOrQueue(t)}
            onGuide={guideNextStep}
            onCancel={() => cancelStream()}
            streaming={streaming}
            guidanceActive={autonomousRunActive}
            disabled={!activeSession && !activeDraft}
            pendingInsert={pendingInsert}
            onInsertConsumed={() => setPendingInsert(undefined)}
            cwd={activeCwd}
          />
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
          fullAccess={settings?.permissions.full_access ?? false}
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
  onOpenSettings: (tab: "endpoints" | "permissions") => void;
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
          <p className="text-[10px] text-gray-600">后台步骤、验收结果与恢复操作</p>
        </div>
        <button autoFocus type="button" onClick={onClose} aria-label="关闭任务活动" className="rounded p-1 text-gray-500 hover:bg-surface-3 hover:text-gray-200">
          <X size={15} />
        </button>
      </div>
      <div className="flex shrink-0 flex-col gap-2 border-b border-border px-4 py-2">
        <div className="flex flex-wrap items-center gap-2 text-[10px] text-gray-600">
          <span>已完成 {completedCount}</span><span>待执行 {pendingCount}</span>
          {runningCount > 0 && <span className="text-accent">执行中 {runningCount}</span>}
          {repairableFailedCount > 0 && <span className="text-amber-700 dark:text-amber-300">可重试 {repairableFailedCount}</span>}
          {providerBlockedTasks.length > 0 && <span className="text-red-700 dark:text-red-300">模型配置 {providerBlockedTasks.length}</span>}
          {permissionBlockedTasks.length > 0 && <span className="text-red-700 dark:text-red-300">权限配置 {permissionBlockedTasks.length}</span>}
          {conversationBlockedTasks.length > 0 && <span className="text-red-700 dark:text-red-300">需要你 {conversationBlockedTasks.length}</span>}
        </div>
        {!isRunning && pendingCount > 0 && (
          <p className={`text-[10px] ${failedTasks.length > 0 ? "text-amber-700 dark:text-amber-300" : "text-gray-500"}`}>
            {failedTasks.length > 0
              ? `先处理失败项，再继续剩余 ${pendingCount} 项。`
              : `执行已暂停，还有 ${pendingCount} 项等待执行。`}
          </p>
        )}
        <div className="flex flex-wrap items-center gap-1">
          {isRunning ? (
            <button onClick={() => void handleCancel()} className="flex items-center gap-1 rounded bg-red-500/10 px-2 py-1 text-[10px] text-red-700 hover:bg-red-500/20 dark:text-red-300"><Square size={9} />停止</button>
          ) : pendingCount > 0 && failedTasks.length === 0 ? (
            <span className="text-[10px] text-gray-500">任务已委派，由后台调度器自动执行；若长时间未开始请检查模型配置或重试委派。</span>
          ) : null}
          {!isRunning && repairableFailedCount > 0 && (
            <button onClick={() => void handleRepairFailed()} disabled={repairBusy} className="flex items-center gap-1 rounded bg-amber-500/10 px-2 py-1 text-[10px] text-amber-700 disabled:opacity-40 dark:text-amber-300" title="重试可自动修复的失败步骤">
              {repairBusy ? <Loader2 size={9} className="animate-spin" /> : <RefreshCw size={9} />}重试失败步骤
            </button>
          )}
          {!isRunning && providerBlockedTasks.length > 0 && (
            <><button onClick={() => onOpenSettings("endpoints")} className="rounded bg-accent/10 px-2 py-1 text-[10px] text-accent hover:bg-accent/20">打开模型设置</button>
            <button aria-label={`已修复，重试 ${providerBlockedTasks.length} 项`} title={`重试：${providerBlockedTasks.map((task) => task.title).join("、")}`} onClick={() => void handleRetryBlocked(providerBlockedTasks)} disabled={blockedRetryBusy} className="flex items-center gap-1 rounded bg-emerald-500/10 px-2 py-1 text-[10px] text-emerald-700 disabled:opacity-40 dark:text-emerald-300"><RefreshCw size={9} />已修复，重试 {providerBlockedTasks.length} 项</button></>
          )}
          {!isRunning && permissionBlockedTasks.length > 0 && (
            <><button onClick={() => onOpenSettings("permissions")} className="rounded bg-accent/10 px-2 py-1 text-[10px] text-accent hover:bg-accent/20">打开权限设置</button>
            <button onClick={() => void handleRetryBlocked(permissionBlockedTasks)} disabled={blockedRetryBusy} className="rounded bg-emerald-500/10 px-2 py-1 text-[10px] text-emerald-700 disabled:opacity-40 dark:text-emerald-300">已授权，重试 {permissionBlockedTasks.length} 项</button></>
          )}
          {!isRunning && conversationBlockedTasks.length > 0 && (
            <button onClick={() => onRequestRepair(conversationBlockedTasks[0])} className="rounded bg-accent/10 px-2 py-1 text-[10px] text-accent hover:bg-accent/20">回到对话处理</button>
          )}
        </div>
      </div>
      {startError && <div className="border-b border-red-500/20 bg-red-500/10 px-4 py-2 text-[10px] text-red-700 dark:text-red-300">{startError}</div>}
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
      className={`rounded transition-colors ${highlighted ? "bg-accent/10 ring-1 ring-accent/40" : "hover:bg-surface-3"}`}
      style={{ paddingLeft: `${0.375 + depth * 0.875}rem` }}
    >
      <div className="group flex items-start gap-2 px-1.5 py-1">
        <Icon
          size={11}
          className={`mt-1 shrink-0 ${statusColor(task.status)} ${
            task.status === "running" ? "animate-spin" : ""
          }`}
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-start gap-1.5">
            <span className="block flex-1 text-[11px] text-gray-300 leading-snug line-clamp-2">
              {task.title}
            </span>
            {summary && (
              <button
                onClick={() => setVerifOpen((v) => !v)}
                title={`验收验证：${summary.passed}/${summary.total} 通过（点击展开逐条）`}
                className={`mt-0.5 inline-flex shrink-0 items-center gap-0.5 rounded px-1 text-[9px] transition-colors hover:bg-surface-2 ${
                  summary.allPassed ? "text-green-500" : "text-red-500"
                }`}
              >
                {summary.allPassed ? <CheckCircle2 size={10} /> : <XCircle size={10} />}
                {summary.passed}/{summary.total}
              </button>
            )}
          </div>
          {task.spec_title && (
            <div
              className="mt-0.5 flex items-center gap-1 text-[9px] text-accent/80"
              title={`来自规范《${task.spec_title}》`}
            >
              <BookOpen size={9} className="shrink-0" />
              <span className="truncate">规范《{task.spec_title}》</span>
            </div>
          )}
          {task.failure_attribution && (
            <div
              className="mt-0.5 flex items-start gap-1 rounded bg-amber-500/10 px-1 py-0.5 text-[9px] text-amber-700 dark:text-amber-300"
              title={`${task.failure_attribution.summary}\n下一步：${task.failure_attribution.next_action}`}
            >
              <AlertTriangle size={9} className="mt-0.5 shrink-0" />
              <span className="shrink-0 font-medium">{task.failure_attribution.label}</span>
              <span className="min-w-0 truncate text-amber-800/80 dark:text-amber-200/80">
                {task.failure_attribution.next_action}
              </span>
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
        onClick={() => hasOutput && setShowOutput((v) => !v)}
      >
        {result.passed ? (
          <CheckCircle2 size={10} className="shrink-0 text-green-500" />
        ) : (
          <XCircle size={10} className="shrink-0 text-red-500" />
        )}
        <span className="flex-1 truncate text-[10px] text-gray-400">{result.check}</span>
        <span className="text-[9px] text-gray-600">{result.duration_ms}ms</span>
      </div>
      {showOutput && hasOutput && (
        <pre className="max-h-32 overflow-y-auto whitespace-pre-wrap px-1.5 pb-1 font-mono text-[9px] text-gray-500">
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
    case "completed":  return "text-green-500";
    case "running":    return "text-accent";
    case "failed":     return "text-red-500";
    default:           return "text-gray-600";
  }
}
