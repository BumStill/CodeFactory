// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import {
  AlertTriangle,
  BookOpen,
  ChevronDown,
  ChevronRight,
  Settings as SettingsIcon,
  Moon,
  Sun,
  Monitor,
  Plus,
  RefreshCw,
  Circle,
  CheckCircle2,
  Loader2,
  XCircle,
  Play,
  Square,
  Brain,
  EyeOff,
  Puzzle,
  ShieldCheck,
  Gauge,
  UserRound,
  GitPullRequestArrow,
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { SessionSidebar } from "../../components/SessionSidebar";
import { SpecsPage } from "../Specs/SpecsPage";
import { ModelPicker } from "../../components/ModelPicker";
import { ReasoningEffortPicker } from "../../components/ReasoningEffortPicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { ContextUsageBar } from "../../components/ContextUsageBar";
import { ExecutionStream } from "../../components/ExecutionStream";
import { GitStatusBar } from "../../components/GitStatusBar";
import { CheckpointsPanel } from "../../components/CheckpointsPanel";
import { GitChangesPanel } from "../../components/GitChangesPanel";
import { GitHistoryPanel } from "../../components/GitHistoryPanel";
import { RemoteGitPanel } from "../../components/RemoteGitPanel";
import { useGitStore } from "../../stores/git";
import { invoke } from "../../lib/tauri";
import { useChatStore, activeRuntime } from "../../stores/chat";
import { QueueBadge } from "../../components/QueueBadge";
import { useSettingsStore } from "../../stores/settings";
import { useTasksStore } from "../../stores/tasks";
import { useLearningStore, type LearningEvent } from "../../stores/learning";
import type { Theme, TaskRun, VerificationResult } from "../../lib/tauri";
import { parseVerification, verificationSummary } from "../../lib/verification";

const EMPTY_LEARNING: LearningEvent[] = [];

interface WorkspacePageProps {
  sessionId: string;
  /** Start another empty quick draft; kept under the legacy prop name so
   * existing embedders remain source-compatible while Home no longer exists. */
  onBackHome: () => void;
  onOpenSettings: () => void;
  onOpenUsage?: () => void;
  /** Switch the workspace to another session in-place (from the sidebar). */
  onOpenSession: (id: string) => void;
  /** Reveal the task workbench and highlight this task when deep-linked from usage. */
  initialTaskLogId?: string | null;
  onOpenResources?: () => void;
  onOpenControlPlane?: () => void;
  onOpenBenchmarks?: () => void;
  onOpenProfile?: () => void;
  /** Open the human evolution review workbench, optionally scoped to a project. */
  onOpenEvolution?: (cwd?: string) => void;
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
  onOpenResources,
  onOpenControlPlane,
  onOpenBenchmarks,
  onOpenProfile,
  onOpenEvolution,
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
  const { settings, setTheme } = useSettingsStore();
  const persistedRunActive = useTasksStore((state) => state.running[sessionId] ?? false);
  const autonomousRunActive = activeDraft ? false : persistedRunActive;
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);
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
  }, [sessionId]);
  // Git / environment panel — surface the (previously unwired) git UI in the
  // right column: a branch/status bar + slide-out Changes / History / PR panels.
  const [gitPanel, setGitPanel] = useState<"changes" | "history" | "remote" | null>(null);
  const gitBranch = useGitStore((s) => s.status?.branch ?? "");
  const activeCwd = activeSession?.cwd ?? activeDraft?.cwd ?? null;
  const learningEvents = useLearningStore(
    (state) => (activeCwd ? state.events[activeCwd] ?? EMPTY_LEARNING : EMPTY_LEARNING),
  );
  const loadLearning = useLearningStore((state) => state.load);
  const subscribeLearning = useLearningStore((state) => state.subscribe);
  const pendingLearningCount = learningEvents.filter((event) => event.status === "pending").length;
  // Specs workbench, folded into the Workspace as a full-screen overlay: it's
  // invoked in-context, scoped to this session's cwd, and its "开始实现" creates +
  // runs tasks in THIS session (no navigation away — unified flow).
  const [specsOpen, setSpecsOpen] = useState(false);
  const projectTaskCount = useTasksStore((state) => state.tasks[sessionId]?.length ?? 0);
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
    // Draft IDs are reused by materialization. Once the first message creates
    // the real session, activeSession already contains that same ID; calling
    // selectSession here would reload get_messages and race the live stream.
    if (activeDraft || activeSession?.id === sessionId) return;
    void selectSession(sessionId);
  }, [activeDraft, activeSession?.id, selectSession, sessionId]);

  useEffect(() => {
    if (!activeCwd) return;
    void loadLearning(activeCwd);
    let off: (() => void) | undefined;
    subscribeLearning(activeCwd).then((unsubscribe) => { off = unsubscribe; });
    return () => { off?.(); };
  }, [activeCwd, loadLearning, subscribeLearning]);

  return (
    <div className="h-full flex flex-col bg-surface-0">

      {/* ── Header ────────────────────────────────────────────────────────── */}
      <header className="flex items-center gap-3 px-3 py-1.5 border-b border-border bg-surface-1 shrink-0">
        <button
          onClick={onBackHome}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="新建空白会话"
          aria-label="新建空白会话"
        >
          <Plus size={14} />
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

        {/* Theme toggle */}
        <div className="flex items-center rounded border border-border overflow-hidden">
          {([
            { v: "dark",   Icon: Moon },
            { v: "light",  Icon: Sun },
            { v: "system", Icon: Monitor },
          ] as { v: Theme; Icon: React.ElementType }[]).map(({ v, Icon }) => (
            <button
              key={v}
              onClick={() => setTheme(v)}
              className={`p-1 transition-colors ${
                settings?.theme === v
                  ? "bg-surface-3 text-accent"
                  : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
              }`}
            >
              <Icon size={13} />
            </button>
          ))}
        </div>

        <button
          onClick={() => setSpecsOpen(true)}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="规范工作台（在当前会话里写需求规范并就地实现）"
        >
          <BookOpen size={14} />
        </button>
        <div className="flex items-center gap-1.5">
          <GitStatusBar
            cwd={activeCwd}
            onOpenChanges={() => setGitPanel("changes")}
            onOpenHistory={() => setGitPanel("history")}
            onOpenRemote={() => setGitPanel("remote")}
          />
          {pendingLearningCount > 0 && activeCwd && onOpenEvolution && (
            <button
              onClick={() => onOpenEvolution(activeCwd)}
              aria-label={`记忆 ${pendingLearningCount}`}
              title="审核 AI 学到的项目记忆"
              className="inline-flex items-center gap-1 rounded border border-accent/30 bg-accent/5 px-2 py-1 text-[11px] text-accent transition-colors hover:bg-accent/10"
            >
              <Brain size={11} />
              <span>记忆</span>
              <span className="tabular-nums">{pendingLearningCount}</span>
            </button>
          )}
          {!activeDraft && <CheckpointsPanel sessionId={sessionId} />}
        </div>
        {onOpenProfile && (
          <button
            onClick={onOpenProfile}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="我的画像"
            aria-label="我的画像"
          >
            <UserRound size={14} />
          </button>
        )}
        {onOpenEvolution && (
          <button
            onClick={() => onOpenEvolution(activeCwd ?? undefined)}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="进化审查"
            aria-label="进化审查"
          >
            <GitPullRequestArrow size={14} />
          </button>
        )}
        {onOpenBenchmarks && (
          <button
            onClick={onOpenBenchmarks}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="能力评测"
            aria-label="能力评测"
          >
            <Gauge size={14} />
          </button>
        )}
        {onOpenResources && (
          <button
            onClick={onOpenResources}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="资源中心"
            aria-label="资源中心"
          >
            <Puzzle size={14} />
          </button>
        )}
        {onOpenControlPlane && (
          <button
            onClick={onOpenControlPlane}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="AI Coding OS"
            aria-label="AI Coding OS"
          >
            <ShieldCheck size={14} />
          </button>
        )}
        <button
          onClick={onOpenSettings}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="设置"
        >
          <SettingsIcon size={14} />
        </button>
      </header>

      {/* ── Body: 3 columns ──────────────────────────────────────────────── */}
      <div className="flex-1 flex min-h-0">

        {/* ─── Left: the session rail is a permanent part of the app shell. ─── */}
        <aside aria-label="会话列表" className="w-64 shrink-0 border-r border-border bg-surface-1 flex flex-col min-h-0">
          <SessionSidebar currentSessionId={sessionId} onOpenSession={onOpenSession} />
        </aside>

        {/* ─── Center: conversation + its internal execution detail ─────── */}
        <main aria-label="会话窗口" className="flex-1 flex flex-col min-w-0">
          {isProjectSession && projectTaskCount > 0 && (
            <TasksColumn sessionId={sessionId} highlightedTaskId={initialTaskLogId} />
          )}
          {!activeDraft && <ExecutionStream sessionId={sessionId} />}
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

      {/* ── Specs workbench, folded into the Workspace as a full-screen ───
          overlay. SpecsPage reads the active session from the chat store, so
          it auto-scopes to this session's cwd; its "开始实现" runs in this very
          session. onOpenWorkspace just closes (we're already here). */}
      {specsOpen && (
        <div className="fixed inset-0 z-50">
          <SpecsPage
            onBack={() => setSpecsOpen(false)}
            onOpenWorkspace={() => setSpecsOpen(false)}
          />
        </div>
      )}

      {/* ── Git / environment slide-out panels (opened from the status bar) ─ */}
      {gitPanel === "changes" && <GitChangesPanel onClose={() => setGitPanel(null)} />}
      {gitPanel === "history" && <GitHistoryPanel onClose={() => setGitPanel(null)} />}
      {gitPanel === "remote" && (
        <RemoteGitPanel
          cwd={activeSession?.cwd ?? null}
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
function TasksColumn({ sessionId, highlightedTaskId }: { sessionId: string; highlightedTaskId?: string | null }) {
  const { tasks, running, start, cancel, retryFailedTasks } = useTasksStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const isRunning = running[sessionId] ?? false;
  const pendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const runningCount = sessionTasks.filter((task) => task.status === "running").length;
  const completedCount = sessionTasks.filter((task) => task.status === "completed").length;
  const failedTasks = sessionTasks.filter(
    (task) => task.status === "failed" || task.status === "cancelled",
  );
  const repairableFailedCount = failedTasks.filter(
    (task) => task.failure_attribution?.repairable,
  ).length;
  const blockedFailedCount = failedTasks.length - repairableFailedCount;
  const [startError, setStartError] = useState<string | null>(null);
  const [repairBusy, setRepairBusy] = useState(false);
  const [collapsed, setCollapsed] = useState(false);

  const handleStart = async () => {
    setStartError(null);
    try { await start(sessionId); } catch (error) { setStartError(String(error)); }
  };
  const handleCancel = async () => {
    try { await cancel(sessionId); } catch (error) { setStartError(String(error)); }
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
    <div className={`flex shrink-0 flex-col border-t border-border ${collapsed ? "" : "max-h-[55%] min-h-0"}`}>
      <div className="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
        <button onClick={() => setCollapsed((value) => !value)} className="flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500 transition-colors hover:text-gray-300" title={collapsed ? "展开执行详情" : "折叠执行详情"}>
          {collapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
          <span title="对话根据需求自动委派的执行步骤">会话执行详情</span>
          <span className="ml-0.5 text-gray-600">· {sessionTasks.length}</span>
        </button>
        <div className="flex items-center gap-1">
          {isRunning ? (
            <button onClick={() => void handleCancel()} className="flex items-center gap-1 rounded bg-red-500/10 px-1.5 py-0.5 text-[10px] text-red-700 hover:bg-red-500/20 dark:text-red-300" title="停止当前会话的委派执行"><Square size={9} />停止</button>
          ) : pendingCount > 0 ? (
            <button onClick={() => void handleStart()} className="flex items-center gap-1 rounded bg-accent px-1.5 py-0.5 text-[10px] text-white hover:bg-accent-hover" title={`继续执行 ${pendingCount} 个待处理步骤`}><Play size={9} />继续</button>
          ) : null}
          {!isRunning && failedTasks.length > 0 && (
            <button onClick={() => void handleRepairFailed()} disabled={repairBusy || repairableFailedCount === 0} className="flex items-center gap-1 rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-700 disabled:opacity-40 dark:text-amber-300" title={repairableFailedCount > 0 ? "重试可自动修复的失败步骤" : "失败原因需要先在对话里处理"}>
              {repairBusy ? <Loader2 size={9} className="animate-spin" /> : <RefreshCw size={9} />}
              {repairableFailedCount > 0 ? "重试失败步骤" : "需在对话处理"}
            </button>
          )}
        </div>
      </div>
      {!collapsed && (
        <>
          {startError && <div className="border-b border-red-500/20 bg-red-500/10 px-3 py-2 text-[10px] text-red-700 dark:text-red-300">{startError}</div>}
          <div className="flex items-center gap-2 border-b border-border px-3 py-1 text-[10px] text-gray-600">
            <span>完成 {completedCount}</span><span>待处理 {pendingCount}</span>
            {runningCount > 0 && <span className="text-accent">运行中 {runningCount}</span>}
            {repairableFailedCount > 0 && <span className="text-amber-700 dark:text-amber-300">可重试 {repairableFailedCount}</span>}
            {blockedFailedCount > 0 && <span className="text-red-700 dark:text-red-300">需处理 {blockedFailedCount}</span>}
          </div>
          <div className="flex-1 overflow-y-auto p-2"><ul className="space-y-0.5">{buildTaskTree(sessionTasks).map(({ task, depth }) => <TaskRow key={task.id} task={task} depth={depth} highlighted={task.id === highlightedTaskId} />)}</ul></div>
        </>
      )}
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
