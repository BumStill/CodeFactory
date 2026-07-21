// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
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
  Sparkles,
  Trash2,
  Wand2,
  X,
  Play,
  Square,
  Brain,
  EyeOff,
  PanelLeftClose,
  PanelLeft,
  Puzzle,
  ShieldCheck,
  Gauge,
  UserRound,
  GitPullRequestArrow,
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { SessionSidebar } from "../../components/SessionSidebar";
import { SessionSwitcherPopover } from "../../components/SessionSwitcherPopover";
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
import { useSpecsStore } from "../../stores/specs";
import { useLearningStore, type LearningEvent } from "../../stores/learning";
import type {
  Theme,
  TaskRun,
  TaskInput,
  TaskDep,
  VerificationResult,
} from "../../lib/tauri";
import { parseVerification, verificationSummary } from "../../lib/verification";

const EMPTY_LEARNING: LearningEvent[] = [];

interface DecomposedTask {
  tmp_id: string;
  title: string;
  description: string;
  dependencies: string[];
  /** Verifiable conditions for "done", populated by the decompose AI.
   *  Shown in the TaskCreator review step so the user can audit / edit
   *  before approving; persisted to task_runs and read back by the
   *  autonomous subagent that must verify each criterion. */
  acceptance_criteria: string[];
}

interface WorkspacePageProps {
  sessionId: string;
  /** Start another empty quick draft; kept under the legacy prop name so
   * existing embedders remain source-compatible while Home no longer exists. */
  onBackHome: () => void;
  onOpenSettings: () => void;
  /** Switch the workspace to another session in-place (from the sidebar). */
  onOpenSession: (id: string) => void;
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
  onOpenSession,
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

  // Collapsible session sidebar. Collapsed → the left rail hides (chat widens)
  // and the top-left icon opens a popover with the full quick-switcher, so
  // collapsing never buries navigation. Persisted so it sticks across launches.
  const [sidebarCollapsed, setSidebarCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem("cf.workspace.sidebarCollapsed") === "1";
    } catch {
      return false; // localStorage unavailable (e.g. some test envs)
    }
  });
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const sidebarCtrlRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    try {
      localStorage.setItem("cf.workspace.sidebarCollapsed", sidebarCollapsed ? "1" : "0");
    } catch {
      /* persistence is best-effort */
    }
    if (!sidebarCollapsed) setSwitcherOpen(false);
  }, [sidebarCollapsed]);
  // Dismiss the collapsed-state quick switcher on an outside click. The ref
  // wraps BOTH the toggle button and the popover, so clicking the toggle
  // itself doesn't count as "outside" (the button owns its own toggle).
  useEffect(() => {
    if (!switcherOpen) return;
    const onDown = (e: MouseEvent) => {
      if (sidebarCtrlRef.current && !sidebarCtrlRef.current.contains(e.target as Node)) {
        setSwitcherOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [switcherOpen]);

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
        {/* Sidebar toggle: collapse the session rail, or (when collapsed) open
            a quick-switcher popover so navigation is never buried. */}
        <div className="relative" ref={sidebarCtrlRef}>
          <button
            onClick={() =>
              sidebarCollapsed ? setSwitcherOpen((v) => !v) : setSidebarCollapsed(true)
            }
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title={sidebarCollapsed ? "会话（点击快速切换 / 展开侧栏）" : "收起会话侧栏"}
          >
            {sidebarCollapsed ? <PanelLeft size={14} /> : <PanelLeftClose size={14} />}
          </button>
          {sidebarCollapsed && switcherOpen && (
            <SessionSwitcherPopover
              currentSessionId={sessionId}
              onOpenSession={(id) => {
                onOpenSession(id);
                setSwitcherOpen(false);
              }}
              onExpand={() => {
                setSidebarCollapsed(false);
                setSwitcherOpen(false);
              }}
            />
          )}
        </div>
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

        {/* ─── Left: Session sidebar + adaptive task tree (collapsible) ─── */}
        {!sidebarCollapsed && (
          <aside aria-label="会话列表" className="w-64 shrink-0 border-r border-border bg-surface-1 flex flex-col min-h-0">
            <SessionSidebar currentSessionId={sessionId} onOpenSession={onOpenSession} />
            {/* Adaptive: the task tree only makes sense for project sessions.
                Quick + anonymous chats have no tasks, so the panel is omitted. */}
            {activeSession &&
              activeSession.kind !== "quick" &&
              activeSession.kind !== "anonymous" && (
                <TasksColumn sessionId={sessionId} />
              )}
          </aside>
        )}

        {/* ─── Center: Execution stream + input ──────────────────────── */}
        <main aria-label="会话窗口" className="flex-1 flex flex-col min-w-0">
          {!activeDraft && <ExecutionStream sessionId={sessionId} />}
          <MessageList
            messages={messages}
            streaming={streaming}
            cwd={activeCwd}
            onUsePrompt={(text) => setPendingInsert(text)}
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

// Derive a short spec title from a free-form intent: first non-empty line,
// trimmed and capped so it makes a sane filename slug + sidebar label.
function deriveSpecTitle(intent: string): string {
  const firstLine =
    intent.split("\n").map((l) => l.trim()).find(Boolean) ?? intent.trim();
  const capped = firstLine.length > 48 ? `${firstLine.slice(0, 48)}…` : firstLine;
  return capped || "未命名需求";
}

// Prompt for spec_ai_assist "generate" — mirrors the Specs generator but injects
// the real req_id/title create_spec already allocated, so the persisted spec and
// the linked tasks agree on one identifier.
function buildSpecPrompt(intent: string, reqId: string | null, title: string): string {
  return (
    `Generate a complete software specification document in markdown with YAML frontmatter for the following feature. ` +
    `Include: frontmatter (req_id: ${reqId ?? "CF-001"}, title: ${title}, status: draft, created_at, updated_at, tags, acceptance_criteria), ` +
    `then sections: Overview, Requirements table, Decision Points (with <!-- DECISION: ... --> comments for anything ambiguous), ` +
    `and Testing Matrix. Feature description: ${intent}`
  );
}

function TasksColumn({ sessionId }: { sessionId: string }) {
  const { tasks, running, loadTasks, subscribe, createTaskTree, start, cancel, retryFailedTasks } = useTasksStore();
  const { activeSession } = useChatStore();
  const { createSpec, saveSpec } = useSpecsStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const isRunning = running[sessionId] ?? false;
  const pendingCount = sessionTasks.filter((t) => t.status === "pending").length;
  const runningCount = sessionTasks.filter((t) => t.status === "running").length;
  const completedCount = sessionTasks.filter((t) => t.status === "completed").length;
  const failedTasks = sessionTasks.filter((t) => t.status === "failed" || t.status === "cancelled");
  const failedCount = failedTasks.length;
  const repairableFailedCount = failedTasks.filter((t) => t.failure_attribution?.repairable).length;
  const blockedFailedCount = failedCount - repairableFailedCount;
  const [creatorOpen, setCreatorOpen] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [repairBusy, setRepairBusy] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  // 自主模式 (②-2): describe intent → auto-decompose → auto-create → auto-start,
  // no modal, no manual review/start. Persisted so it survives reloads. When
  // off, the reviewed modal flow (+ → describe → review → create → 开始) stays.
  const [autonomous, setAutonomousState] = useState<boolean>(() => {
    try {
      return localStorage.getItem("cf.workspace.autonomous") === "1";
    } catch {
      return false; // localStorage unavailable (e.g. some test envs)
    }
  });
  const setAutonomous = (v: boolean) => {
    setAutonomousState(v);
    try {
      localStorage.setItem("cf.workspace.autonomous", v ? "1" : "0");
    } catch {
      // localStorage unavailable — keep the toggle in-memory only.
    }
  };
  const [autoIntent, setAutoIntent] = useState("");
  const [autoBusy, setAutoBusy] = useState(false);
  // ②-3 自动写 spec: when on, a substantial intent is first formalized into a
  // persisted spec (AI), then decomposed from that spec — folding the Specs
  // "生成" capability into the autonomous flow. Off → intent decomposes directly.
  const [specFirst, setSpecFirstState] = useState<boolean>(() => {
    try {
      return localStorage.getItem("cf.workspace.specFirst") === "1";
    } catch {
      return false;
    }
  });
  const setSpecFirst = (v: boolean) => {
    setSpecFirstState(v);
    try {
      localStorage.setItem("cf.workspace.specFirst", v ? "1" : "0");
    } catch {
      // localStorage unavailable — in-memory only.
    }
  };
  // Short phase label shown while the autonomous chain runs ("起草规范…" etc.).
  const [autoStatus, setAutoStatus] = useState("");

  useEffect(() => {
    loadTasks(sessionId);
    let unsub: (() => void) | undefined;
    subscribe(sessionId).then((u) => { unsub = u; });
    return () => { unsub?.(); };
  }, [sessionId]);

  const createFromDecomposed = async (decomposed: DecomposedTask[]) => {
    const cwd = activeSession?.cwd ?? "";
    const inputs: TaskInput[] = decomposed.map((d) => ({
      tmp_id: d.tmp_id,
      title: d.title,
      description: d.description,
      cwd,
      acceptance_criteria: d.acceptance_criteria,
    }));
    const deps: TaskDep[] = decomposed.flatMap((d) =>
      d.dependencies.map((depId) => ({
        task_tmp_id: d.tmp_id,
        depends_on_tmp_id: depId,
      }))
    );
    await createTaskTree(sessionId, inputs, deps);
  };

  const handleConfirm = async (decomposed: DecomposedTask[]) => {
    await createFromDecomposed(decomposed);
    setCreatorOpen(false);
  };

  // 自主一键闭环:意图 →(可选先写 spec)→ 拆解 → 建任务树 → 开始执行,无模态框、
  // 无人工审核。start() 从数据库读 pending 任务执行,故 createTaskTree → start 背靠背可行。
  const runAutonomous = async () => {
    const intent = autoIntent.trim();
    if (!intent || autoBusy || isRunning) return;
    setAutoBusy(true);
    setStartError(null);
    setAutoStatus("");
    const cwd = activeSession?.cwd ?? "";
    try {
      let result: DecomposedTask[];
      let specReqId: string | undefined;
      let specTitle: string | undefined;
      // ②-3: 大意图先 formalize 成一份持久化 spec,再从 spec 拆解。落盘需要项目
      // 目录 —— 无 cwd(临时会话)时回退为直接拆解。
      if (specFirst && cwd) {
        setAutoStatus("起草规范…");
        const specFile = await createSpec(cwd, deriveSpecTitle(intent));
        specReqId = specFile.meta.req_id ?? undefined;
        specTitle = specFile.meta.title;
        let specMd = await invoke<string>("spec_ai_assist", {
          specContent: "",
          instruction: buildSpecPrompt(intent, specFile.meta.req_id, specFile.meta.title),
        });
        // Pin the persisted frontmatter req_id to the one create_spec allocated,
        // so the saved spec and the linked tasks agree on one identifier even if
        // the model drifts from the requested id.
        if (specFile.meta.req_id) {
          specMd = specMd.replace(/^(\s*req_id:\s*).*$/m, `$1${specFile.meta.req_id}`);
        }
        await saveSpec(specFile.meta.file_path, specMd);
        setAutoStatus("拆解规范…");
        result = await invoke<DecomposedTask[]>("decompose_spec_to_tasks", {
          specContent: specMd,
          cwd,
        });
      } else {
        setAutoStatus("拆解需求…");
        result = await invoke<DecomposedTask[]>("decompose_request_to_tasks", {
          request: intent,
          cwd,
        });
      }
      if (result.length === 0) {
        setStartError("AI 没有拆出可执行任务,换个说法再试。");
        return;
      }
      await createFromDecomposed(result);
      setAutoIntent("");
      setAutoStatus("");
      // Spec linkage rides on start() (same contract SpecsPage uses); undefined
      // for the direct path collapses to a plain start.
      await start(sessionId, specReqId, specTitle);
    } catch (e) {
      setStartError(String(e));
    } finally {
      setAutoBusy(false);
      setAutoStatus("");
    }
  };

  const handleStart = async () => {
    setStartError(null);
    try {
      await start(sessionId);
    } catch (e) {
      setStartError(String(e));
    }
  };

  const handleCancel = async () => {
    try {
      await cancel(sessionId);
    } catch (e) {
      setStartError(String(e));
    }
  };

  const handleRepairFailed = async () => {
    if (repairBusy || isRunning || repairableFailedCount === 0) return;
    setRepairBusy(true);
    setStartError(null);
    try {
      const retried = await retryFailedTasks(sessionId);
      if (retried > 0) {
        await start(sessionId, undefined, undefined);
      }
    } catch (e) {
      setStartError(String(e));
    } finally {
      setRepairBusy(false);
    }
  };

  return (
    <div className={`flex shrink-0 flex-col border-t border-border ${collapsed ? "" : "max-h-[55%] min-h-0"}`}>
      <div className="flex items-center justify-between px-3 py-2 border-b border-border gap-2">
        <button
          onClick={() => setCollapsed((v) => !v)}
          className="flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500 transition-colors hover:text-gray-300"
          title={collapsed ? "展开任务" : "折叠任务"}
        >
          {collapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
          任务
          {sessionTasks.length > 0 && (
            <span className="ml-0.5 text-gray-600">· {sessionTasks.length}</span>
          )}
        </button>
        <div className="flex items-center gap-1">
          <button
            onClick={() => setAutonomous(!autonomous)}
            className={`flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors ${
              autonomous
                ? "text-accent bg-accent/10 hover:bg-accent/20"
                : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
            }`}
            title={
              autonomous
                ? "自主模式已开:描述需求即自动拆解并执行(点此关闭,改用审核流)"
                : "开启自主模式:描述需求即自动拆解并执行,免逐步确认"
            }
          >
            <Sparkles size={10} className={autonomous ? "fill-accent/40" : ""} />
            自主
          </button>
          {isRunning ? (
            <button
              onClick={handleCancel}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] text-red-700 dark:text-red-300 bg-red-500/10 hover:bg-red-500/20 transition-colors"
              title="取消执行"
            >
              <Square size={9} />
              停止
            </button>
          ) : (
            pendingCount > 0 && (
              <button
                onClick={handleStart}
                className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] text-white bg-accent hover:bg-accent-hover transition-colors"
                title={`开始执行 ${pendingCount} 个待处理任务`}
              >
                <Play size={9} />
                开始
              </button>
            )
          )}
          {!isRunning && failedCount > 0 && (
            <button
              onClick={handleRepairFailed}
              disabled={repairBusy || repairableFailedCount === 0}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] text-amber-700 dark:text-amber-300 bg-amber-500/10 hover:bg-amber-500/20 transition-colors disabled:opacity-40 disabled:hover:bg-amber-500/10"
              title={
                repairableFailedCount > 0
                  ? `重置 ${repairableFailedCount} 个可修复失败任务为待处理，并立即重新执行；${blockedFailedCount} 个需要先处理失败原因`
                  : "没有可自动修复项；请先处理模型、权限或运行环境问题"
              }
            >
              {repairBusy ? (
                <Loader2 size={9} className="animate-spin" />
              ) : (
                <RefreshCw size={9} />
              )}
              {repairableFailedCount > 0 ? "修复可修复项" : "先处理失败原因"}
            </button>
          )}
          {!autonomous && (
            <button
              onClick={() => setCreatorOpen(true)}
              className="p-0.5 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
              title="AI 拆解需求为任务"
            >
              <Plus size={12} />
            </button>
          )}
        </div>
      </div>
      {!collapsed && (
        <>
          {startError && (
            <div className="px-3 py-2 text-[10px] text-red-700 dark:text-red-300 bg-red-500/10 border-b border-red-500/20">
              {startError}
            </div>
          )}
          {sessionTasks.length > 0 && (
            <div className="flex items-center gap-2 border-b border-border px-3 py-1 text-[10px] text-gray-600">
              <span>完成 {completedCount}</span>
              <span>待处理 {pendingCount}</span>
              {runningCount > 0 && <span className="text-accent">运行中 {runningCount}</span>}
              {repairableFailedCount > 0 && (
                <span className="text-amber-700 dark:text-amber-300">可修复 {repairableFailedCount}</span>
              )}
              {blockedFailedCount > 0 && (
                <span className="text-red-700 dark:text-red-300">需处理 {blockedFailedCount}</span>
              )}
            </div>
          )}
          {autonomous && (
            <div className="px-2 py-2 border-b border-border">
              <textarea
                value={autoIntent}
                onChange={(e) => setAutoIntent(e.target.value)}
                disabled={autoBusy || isRunning}
                rows={2}
                aria-label="自主任务描述"
                placeholder={
                  isRunning
                    ? "执行中…用下方「引导下一步」插嘴"
                    : specFirst
                    ? "描述大需求,回车先写规范、再拆解执行…"
                    : "描述要做什么,回车自动拆解并执行…"
                }
                className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-[12px] text-gray-200 outline-none focus:border-accent resize-none disabled:opacity-50"
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void runAutonomous();
                  }
                }}
              />
              <div className="mt-1 flex items-center justify-between gap-2">
                <button
                  onClick={() => setSpecFirst(!specFirst)}
                  disabled={autoBusy || isRunning}
                  className={`flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors disabled:opacity-40 ${
                    specFirst
                      ? "text-accent bg-accent/10 hover:bg-accent/20"
                      : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
                  }`}
                  title={
                    specFirst
                      ? "先写规范:大意图先 formalize 成 spec、落盘后再拆解(点此关闭)"
                      : "开启先写规范:大需求先让 AI 写一份 spec,落盘后再拆解执行"
                  }
                >
                  <BookOpen size={10} />
                  先写规范
                </button>
                <div className="flex items-center gap-2 min-w-0">
                  <span className="truncate text-[10px] text-gray-600">
                    {autoBusy && autoStatus ? (
                      <span className="text-accent">{autoStatus}</span>
                    ) : (
                      "回车 · Shift+Enter 换行"
                    )}
                  </span>
                  <button
                    onClick={() => void runAutonomous()}
                    disabled={autoBusy || isRunning || !autoIntent.trim()}
                    className="flex shrink-0 items-center gap-1 px-2 py-0.5 rounded text-[10px] text-white bg-accent hover:bg-accent-hover transition-colors disabled:opacity-40"
                  >
                    {autoBusy ? (
                      <Loader2 size={10} className="animate-spin" />
                    ) : specFirst ? (
                      <BookOpen size={10} />
                    ) : (
                      <Sparkles size={10} />
                    )}
                    {autoBusy ? "执行中…" : specFirst ? "写规范并执行" : "自主执行"}
                  </button>
                </div>
              </div>
            </div>
          )}
          <div className="flex-1 overflow-y-auto p-2">
            {sessionTasks.length === 0 ? (
              autonomous ? (
                <p className="text-center text-[11px] text-gray-600 py-8 leading-relaxed">
                  自主模式已开启<br />
                  <span className="text-gray-700">在上方描述需求<br />AI 自动拆解并执行</span>
                </p>
              ) : (
                <button
                  onClick={() => setCreatorOpen(true)}
                  className="w-full text-[11px] text-gray-600 hover:text-gray-300 hover:bg-surface-2 rounded transition-colors py-8 leading-relaxed cursor-pointer"
                >
                  还没有任务<br />
                  <span className="text-gray-700">点这里描述需求<br />AI 会自动拆解</span>
                </button>
              )
            ) : (
              <ul className="space-y-0.5">
                {buildTaskTree(sessionTasks).map(({ task, depth }) => (
                  <TaskRow key={task.id} task={task} depth={depth} />
                ))}
              </ul>
            )}
          </div>
        </>
      )}
      {creatorOpen && (
        <TaskCreatorModal
          cwd={activeSession?.cwd ?? null}
          onCancel={() => setCreatorOpen(false)}
          onConfirm={handleConfirm}
        />
      )}
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// TaskCreatorModal — describe → AI decompose → review → create
// ─────────────────────────────────────────────────────────────────────────────

interface TaskCreatorModalProps {
  cwd: string | null;
  onCancel: () => void;
  onConfirm: (tasks: DecomposedTask[]) => Promise<void>;
}

function TaskCreatorModal({ cwd, onCancel, onConfirm }: TaskCreatorModalProps) {
  const [phase, setPhase] = useState<"input" | "decomposing" | "review">("input");
  const [request, setRequest] = useState("");
  const [tasks, setTasks] = useState<DecomposedTask[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const decompose = async () => {
    if (!request.trim()) return;
    setPhase("decomposing");
    setError(null);
    try {
      // cwd lets the backend inject this user/project's context (memory,
      // learnings, preferences) so the decomposition is tailored, not
      // generic. Sent as null when no project is open — backend handles.
      const result = await invoke<DecomposedTask[]>("decompose_request_to_tasks", {
        request: request.trim(),
        cwd,
      });
      setTasks(result);
      setPhase("review");
    } catch (e) {
      setError(String(e));
      setPhase("input");
    }
  };

  const confirm = async () => {
    setBusy(true);
    try {
      await onConfirm(tasks);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const updateTask = (idx: number, patch: Partial<DecomposedTask>) => {
    setTasks((prev) => prev.map((t, i) => (i === idx ? { ...t, ...patch } : t)));
  };

  const removeTask = (idx: number) => {
    setTasks((prev) => prev.filter((_, i) => i !== idx));
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div className="w-full max-w-2xl rounded-lg border border-border bg-surface-1 shadow-2xl flex flex-col max-h-[80vh]">
        <header className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-gray-200 flex items-center gap-2">
            <Wand2 size={14} className="text-accent" />
            {phase === "input" && "描述你要做什么"}
            {phase === "decomposing" && "AI 正在拆解..."}
            {phase === "review" && "审核并确认任务"}
          </h2>
          <button
            onClick={onCancel}
            disabled={busy}
            className="p-1 rounded text-gray-500 hover:text-gray-200 hover:bg-surface-3 transition-colors disabled:opacity-40"
          >
            <X size={14} />
          </button>
        </header>

        <div className="flex-1 overflow-y-auto p-4">
          {phase === "input" && (
            <>
              <textarea
                autoFocus
                value={request}
                onChange={(e) => setRequest(e.target.value)}
                rows={6}
                className="w-full bg-surface-2 border border-border rounded px-3 py-2 text-sm text-gray-200 outline-none focus:border-accent resize-y"
                placeholder="例如：做一个本地记账 app，能记录收支、按月汇总、生成图表。&#10;或：给这个项目加上深色模式的设置面板。"
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void decompose();
                  }
                }}
              />
              <p className="mt-2 text-[11px] text-gray-500">
                Cmd/Ctrl + Enter 直接拆解
              </p>
              {error && (
                <p className="mt-2 text-xs text-red-700 dark:text-red-300">{error}</p>
              )}
            </>
          )}

          {phase === "decomposing" && (
            <div className="flex items-center justify-center py-12 gap-3 text-gray-400">
              <Loader2 size={18} className="animate-spin text-accent" />
              <span className="text-sm">AI 正在思考任务拆解...</span>
            </div>
          )}

          {phase === "review" && (
            <div className="space-y-3">
              <ol className="space-y-2">
                {tasks.map((t, i) => (
                  <li
                    key={t.tmp_id}
                    className="border border-border rounded-lg bg-surface-2 p-3"
                  >
                    <div className="flex items-start gap-2">
                      <span className="text-[11px] font-mono text-gray-500 mt-1 w-5 text-right shrink-0">
                        {i + 1}
                      </span>
                      <div className="flex-1 space-y-1.5">
                        <input
                          type="text"
                          value={t.title}
                          onChange={(e) => updateTask(i, { title: e.target.value })}
                          className="w-full bg-transparent border-0 border-b border-transparent focus:border-accent text-sm font-medium text-gray-200 outline-none"
                        />
                        <textarea
                          value={t.description}
                          onChange={(e) => updateTask(i, { description: e.target.value })}
                          rows={2}
                          className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-[12px] text-gray-300 outline-none focus:border-accent resize-y"
                        />
                        {t.dependencies.length > 0 && (
                          <div className="text-[10px] text-gray-500">
                            依赖：{t.dependencies.join(", ")}
                          </div>
                        )}
                        {/* Acceptance criteria — editable. The autonomous
                            subagent reads these as the contract for "done"
                            and verifies each before reporting completion. */}
                        <AcceptanceEditor
                          value={t.acceptance_criteria}
                          onChange={(next) => updateTask(i, { acceptance_criteria: next })}
                          disabled={busy}
                        />
                      </div>
                      <button
                        onClick={() => removeTask(i)}
                        disabled={busy}
                        className="p-1 rounded text-gray-600 hover:text-red-700 dark:hover:text-red-400 hover:bg-surface-3 disabled:opacity-40"
                        title="移除"
                      >
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </li>
                ))}
                {tasks.length === 0 && (
                  <p className="text-center text-xs text-gray-500 py-8">
                    全部移除了。返回重新描述。
                  </p>
                )}
              </ol>
            </div>
          )}
        </div>

        <footer className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border">
          {phase === "input" && (
            <>
              <button
                onClick={onCancel}
                className="px-3 py-1.5 rounded text-xs text-gray-400 hover:bg-surface-3"
              >
                取消
              </button>
              <button
                onClick={decompose}
                disabled={!request.trim()}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40"
              >
                <Wand2 size={12} />
                AI 拆解
              </button>
            </>
          )}
          {phase === "review" && (
            <>
              <button
                onClick={() => setPhase("input")}
                disabled={busy}
                className="px-3 py-1.5 rounded text-xs text-gray-400 hover:bg-surface-3 disabled:opacity-40"
              >
                返回修改
              </button>
              <button
                onClick={confirm}
                disabled={busy || tasks.length === 0}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40"
              >
                {busy && <Loader2 size={12} className="animate-spin" />}
                {busy ? "创建中..." : `创建 ${tasks.length} 个任务`}
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  );
}

// Flat list → depth-annotated render order via parent_task_id.
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

function TaskRow({ task, depth }: { task: TaskRun; depth: number }) {
  const Icon = statusIcon(task.status);
  // Surface acceptance-criteria verification right here in the task tree — the
  // "did it actually pass?" proof that previously only lived in evidence packs.
  const verif = parseVerification(task.verification_results);
  const summary = verificationSummary(task.verification_results);
  const [verifOpen, setVerifOpen] = useState(false);
  return (
    <li
      className="rounded hover:bg-surface-3 transition-colors"
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

// ─────────────────────────────────────────────────────────────────────────────
// AcceptanceEditor — inline list of "what makes this task done" bullets.
// Used inside TaskCreator review step so users can audit / refine the
// AI-proposed acceptance criteria before approving. The autonomous
// subagent treats these as a hard contract: it must verify each line
// before reporting completion.
// ─────────────────────────────────────────────────────────────────────────────

function AcceptanceEditor({
  value,
  onChange,
  disabled,
}: {
  value: string[];
  onChange: (next: string[]) => void;
  disabled: boolean;
}) {
  const [draft, setDraft] = useState("");

  const add = () => {
    const v = draft.trim();
    if (!v) return;
    onChange([...value, v]);
    setDraft("");
  };
  const remove = (i: number) => onChange(value.filter((_, idx) => idx !== i));
  const update = (i: number, v: string) =>
    onChange(value.map((x, idx) => (idx === i ? v : x)));

  return (
    <div className="mt-1 space-y-1">
      <div className="text-[10px] text-gray-500 flex items-center gap-1">
        <CheckCircle2 size={10} className="text-accent" />
        验收条件（AI 必须逐条核对才算完成）
      </div>
      {value.length === 0 && (
        <div className="text-[10px] text-amber-700 dark:text-amber-400 italic">
          ⚠ 没有验收条件 — AI 可能凭感觉报完成。建议至少 1-2 条。
        </div>
      )}
      <ul className="space-y-1">
        {value.map((c, i) => (
          <li key={i} className="flex items-start gap-1.5">
            <span className="text-[10px] text-gray-600 mt-1.5">•</span>
            <input
              type="text"
              value={c}
              onChange={(e) => update(i, e.target.value)}
              disabled={disabled}
              className="flex-1 bg-surface-3 border border-border rounded px-1.5 py-0.5 text-[11px] text-gray-300 font-mono outline-none focus:border-accent disabled:opacity-40"
            />
            <button
              onClick={() => remove(i)}
              disabled={disabled}
              className="p-0.5 rounded text-gray-600 hover:text-red-700 dark:hover:text-red-400 disabled:opacity-40"
              title="移除"
            >
              <X size={10} />
            </button>
          </li>
        ))}
      </ul>
      <div className="flex items-center gap-1.5 mt-1">
        <input
          type="text"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          disabled={disabled}
          placeholder="加一条验收条件，例如：cargo test foo 通过"
          className="flex-1 bg-surface-3 border border-border rounded px-1.5 py-0.5 text-[11px] text-gray-400 font-mono outline-none focus:border-accent placeholder-gray-600 disabled:opacity-40"
        />
        <button
          onClick={add}
          disabled={disabled || !draft.trim()}
          className="p-0.5 rounded text-gray-600 hover:text-accent disabled:opacity-40"
          title="添加"
        >
          <Plus size={11} />
        </button>
      </div>
    </div>
  );
}
