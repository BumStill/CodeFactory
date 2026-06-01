// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  BookOpen,
  ChevronLeft,
  ChevronDown,
  ChevronRight,
  Settings as SettingsIcon,
  Moon,
  Sun,
  Monitor,
  Plus,
  Circle,
  CheckCircle2,
  Loader2,
  XCircle,
  Puzzle,
  RefreshCw,
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
  PanelRightClose,
  PanelRight,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
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
import { invoke } from "../../lib/tauri";
import { useChatStore, activeRuntime } from "../../stores/chat";
import { QueueBadge } from "../../components/QueueBadge";
import { useSettingsStore } from "../../stores/settings";
import { useTasksStore } from "../../stores/tasks";
import { useSkillsStore } from "../../stores/skills";
import { useLearningStore } from "../../stores/learning";
import { useKnowledgeStore } from "../../stores/knowledge";
import type {
  KnowledgeLibrary,
  TaskConnectorContext,
  Theme,
  TaskRun,
  TaskInput,
  TaskDep,
  VerificationResult,
} from "../../lib/tauri";
import { parseVerification, verificationSummary } from "../../lib/verification";

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
  onBackHome: () => void;
  onOpenSettings: () => void;
  /** Switch the workspace to another session in-place (from the sidebar). */
  onOpenSession: (id: string) => void;
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
export function WorkspacePage({ sessionId, onBackHome, onOpenSettings, onOpenSession }: WorkspacePageProps) {
  const {
    activeSession,
    selectSession, sendOrQueue, cancelStream, removeFromQueue,
    respondPermission, exitAnonymous,
  } = useChatStore();
  // Per-session chat state for the ACTIVE session. Background sessions keep
  // streaming into their own buckets; here we render the active one's slice.
  const { messages, streaming, queue, pendingPermission } = useChatStore(activeRuntime);
  const isAnonymous = activeSession?.kind === "anonymous";
  const { settings, setTheme } = useSettingsStore();
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);
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
  // Right "连接器" column (knowledge / skills / memory) — informational, so a
  // plain collapse toggle (no popover). Persisted independently of the left.
  const [connectorsCollapsed, setConnectorsCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem("cf.workspace.connectorsCollapsed") === "1";
    } catch {
      return false;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("cf.workspace.connectorsCollapsed", connectorsCollapsed ? "1" : "0");
    } catch {
      /* persistence is best-effort */
    }
  }, [connectorsCollapsed]);
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
    selectSession(sessionId);
  }, [sessionId]);

  return (
    <div className="h-full flex flex-col bg-surface-0">

      {/* ── Header ────────────────────────────────────────────────────────── */}
      <header className="flex items-center gap-3 px-3 py-1.5 border-b border-border bg-surface-1 shrink-0">
        <button
          onClick={onBackHome}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="返回首页"
        >
          <ChevronLeft size={14} />
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
            {activeSession?.title || "..."}
            {activeSession?.kind === "quick" && (
              <span
                className="text-[9px] px-1.5 py-0.5 rounded bg-accent/15 text-accent font-normal"
                title="一次性助手会话，不会出现在「最近项目」"
              >
                Quick
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
            {isAnonymous ? "无痕会话 · 不落库 · 不计费 · 不学习" : activeSession?.cwd}
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
        <button
          onClick={() => setConnectorsCollapsed((v) => !v)}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title={connectorsCollapsed ? "显示连接器（知识库 / 技能 / 记忆）" : "收起连接器面板"}
        >
          {connectorsCollapsed ? <PanelRight size={14} /> : <PanelRightClose size={14} />}
        </button>
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
          <aside className="w-64 shrink-0 border-r border-border bg-surface-1 flex flex-col min-h-0">
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
        <main className="flex-1 flex flex-col min-w-0">
          <ExecutionStream sessionId={sessionId} />
          <MessageList
            messages={messages}
            streaming={streaming}
            cwd={activeSession?.cwd ?? null}
            onUsePrompt={(text) => setPendingInsert(text)}
          />
          <ContextUsageBar sessionId={activeSession?.id} />
          {queue.length > 0 && (
            <QueueBadge queue={queue} onRemove={removeFromQueue} />
          )}
          <MessageInput
            key={activeSession?.id ?? sessionId}
            initialHistory={messages.filter((m) => m.role === "user").map((m) => m.content)}
            onSend={(t) => void sendOrQueue(t)}
            onCancel={() => cancelStream()}
            streaming={streaming}
            disabled={!activeSession}
            pendingInsert={pendingInsert}
            onInsertConsumed={() => setPendingInsert(undefined)}
            cwd={activeSession?.cwd ?? null}
          />
        </main>

        {/* ─── Right: connectors (collapsible — informational, not nav) ── */}
        {!connectorsCollapsed && (
          <aside className="w-60 shrink-0 border-l border-border bg-surface-1 flex flex-col">
            <ConnectorsColumn cwd={activeSession?.cwd ?? null} />
          </aside>
        )}
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

      {/* ── Permission dialog overlay ───────────────────────────────────── */}
      {pendingPermission && (
        <PermissionDialog
          request={pendingPermission}
          fullAccess={settings?.permissions.full_access ?? false}
          onAllow={() => respondPermission(true)}
          onDeny={() => respondPermission(false)}
          onAllowFullAccess={() => respondPermission(true)}
        />
      )}
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// TasksColumn
// ─────────────────────────────────────────────────────────────────────────────

function TasksColumn({ sessionId }: { sessionId: string }) {
  const { tasks, running, loadTasks, subscribe, createTaskTree, start, cancel } = useTasksStore();
  const { activeSession } = useChatStore();
  const { libraries } = useKnowledgeStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const isRunning = running[sessionId] ?? false;
  const pendingCount = sessionTasks.filter((t) => t.status === "pending").length;
  const [creatorOpen, setCreatorOpen] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(false);

  useEffect(() => {
    loadTasks(sessionId);
    let unsub: (() => void) | undefined;
    subscribe(sessionId).then((u) => { unsub = u; });
    return () => { unsub?.(); };
  }, [sessionId]);

  const handleConfirm = async (decomposed: DecomposedTask[]) => {
    const cwd = activeSession?.cwd ?? "";
    const knowledgeLibraries = libraries.filter((library) => library.enabled);
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
    await createTaskTree(
      sessionId,
      inputs,
      deps,
      buildTaskConnectorContext(knowledgeLibraries),
    );
    setCreatorOpen(false);
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
          <button
            onClick={() => setCreatorOpen(true)}
            className="p-0.5 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="AI 拆解需求为任务"
          >
            <Plus size={12} />
          </button>
        </div>
      </div>
      {!collapsed && (
        <>
          {startError && (
            <div className="px-3 py-2 text-[10px] text-red-700 dark:text-red-300 bg-red-500/10 border-b border-red-500/20">
              {startError}
            </div>
          )}
          <div className="flex-1 overflow-y-auto p-2">
            {sessionTasks.length === 0 ? (
              <button
                onClick={() => setCreatorOpen(true)}
                className="w-full text-[11px] text-gray-600 hover:text-gray-300 hover:bg-surface-2 rounded transition-colors py-8 leading-relaxed cursor-pointer"
              >
                还没有任务<br />
                <span className="text-gray-700">点这里描述需求<br />AI 会自动拆解</span>
              </button>
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
          knowledgeLibraries={libraries.filter((library) => library.enabled)}
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
  knowledgeLibraries: KnowledgeLibrary[];
  onCancel: () => void;
  onConfirm: (tasks: DecomposedTask[]) => Promise<void>;
}

function TaskCreatorModal({ cwd, knowledgeLibraries, onCancel, onConfirm }: TaskCreatorModalProps) {
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
              <KnowledgeContextPanel knowledgeLibraries={knowledgeLibraries} />
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
              <KnowledgeContextPanel knowledgeLibraries={knowledgeLibraries} compact />
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
  const context = parseTaskConnectorContext(task.task_context_json);
  const knowledgeCount = context.knowledge_libraries.length;
  const hasUnindexed = context.knowledge_libraries.some((library) =>
    !["ready", "completed", "completed_with_errors"].includes(library.scan_status),
  );
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
          {knowledgeCount > 0 && (
            <div
              className="mt-0.5 flex items-center gap-1 text-[9px] text-gray-600"
              title={context.knowledge_libraries
                .map((library) => `${library.name} · ${scanStatusText(library.scan_status)}`)
                .join("\n")}
            >
              <BookOpen size={9} className="shrink-0 text-accent" />
              <span>知识库 {knowledgeCount}</span>
              {hasUnindexed && <span className="text-amber-700 dark:text-amber-300">待索引</span>}
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
// ConnectorsColumn — shows what capabilities are currently active
// ─────────────────────────────────────────────────────────────────────────────

// Stable empty array so selectors that fall back to it don't churn
// referential equality every render and re-enter the store subscription.
const EMPTY_LEARNING: LearningEventForSelector[] = [];

type LearningEventForSelector = ReturnType<typeof useLearningStore.getState>["events"][string][number];

function ConnectorsColumn({ cwd }: { cwd: string | null }) {
  const { skills, loadSkills } = useSkillsStore();
  const learningEvents = useLearningStore(
    (s) => (cwd ? s.events[cwd] ?? EMPTY_LEARNING : EMPTY_LEARNING),
  );
  const loadLearning = useLearningStore((s) => s.load);
  const subscribeLearning = useLearningStore((s) => s.subscribe);
  const acceptLearning = useLearningStore((s) => s.accept);
  const rejectLearning = useLearningStore((s) => s.reject);

  // Subscribe to learning events for the current cwd. The store dedups
  // multiple subscribe() calls per cwd so this is safe to remount.
  useEffect(() => {
    if (!cwd) return;
    void loadLearning(cwd);
    let off: (() => void) | undefined;
    subscribeLearning(cwd).then((u) => { off = u; });
    return () => { off?.(); };
  }, [cwd]);
  const {
    libraries,
    scanSummaries,
    loading: knowledgeLoading,
    scanning,
    error: knowledgeError,
    loadLibraries,
    registerLibrary,
    scanLibrary,
  } = useKnowledgeStore();
  const [libraryError, setLibraryError] = useState<string | null>(null);

  useEffect(() => {
    loadSkills();
    void loadLibraries();
  }, []);

  const enabled = skills.filter((s) => s.enabled);
  const enabledLibraries = libraries.filter((library) => library.enabled);
  const connectorCount = enabled.length + enabledLibraries.length;

  const addLibrary = async () => {
    setLibraryError(null);
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "选择知识库文件夹",
      });
      if (typeof selected !== "string") return;
      const name = selected.split(/[\\/]/).filter(Boolean).pop() ?? "个人知识库";
      await registerLibrary(name, selected);
    } catch (e) {
      setLibraryError(String(e));
    }
  };

  const scan = async (libraryId: string) => {
    setLibraryError(null);
    try {
      await scanLibrary(libraryId);
    } catch (e) {
      setLibraryError(String(e));
    }
  };

  return (
    <>
      <div className="flex items-center gap-1.5 px-3 py-2 border-b border-border">
        <Puzzle size={11} className="text-gray-500" />
        <h2 className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">
          连接器
        </h2>
        <span className="ml-auto text-[10px] text-gray-600">{connectorCount}</span>
      </div>
      <div className="flex-1 overflow-y-auto p-2">
        <section className="mb-3">
          <div className="mb-1.5 flex items-center gap-1.5 px-1">
            <BookOpen size={11} className="text-gray-500" />
            <h3 className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">
              个人知识库
            </h3>
            <span className="ml-auto text-[10px] text-gray-600">
              {knowledgeLoading ? "..." : `${enabledLibraries.length} 个知识库`}
            </span>
          </div>
          <div className="space-y-1">
            {enabledLibraries.length === 0 ? (
              <button
                onClick={addLibrary}
                className="w-full rounded border border-dashed border-border bg-surface-2 px-2 py-3 text-center text-[11px] text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
              >
                添加本地资料文件夹
              </button>
            ) : (
              <>
                <button
                  onClick={addLibrary}
                  className="flex w-full items-center justify-center gap-1 rounded border border-border bg-surface-2 px-2 py-1.5 text-[11px] text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
                >
                  <Plus size={11} />
                  添加知识库
                </button>
                <ul className="space-y-1">
                  {enabledLibraries.map((library) => {
                    const summary = scanSummaries[library.id];
                    const isScanning = scanning[library.id] ?? false;
                    return (
                      <li
                        key={library.id}
                        className="rounded border border-border bg-surface-2 px-2 py-1.5"
                        title={library.root_path}
                      >
                        <div className="flex items-start gap-1.5">
                          <BookOpen size={10} className="mt-0.5 shrink-0 text-accent" />
                          <div className="min-w-0 flex-1">
                            <div className="truncate text-[11px] font-medium text-gray-300">
                              {library.name}
                            </div>
                            <div className="truncate font-mono text-[9px] text-gray-600">
                              {library.root_path}
                            </div>
                          </div>
                          <button
                            onClick={() => void scan(library.id)}
                            disabled={isScanning}
                            className="rounded p-0.5 text-gray-600 hover:text-gray-300 hover:bg-surface-3 disabled:opacity-40"
                            title="扫描知识库"
                          >
                            <RefreshCw
                              size={11}
                              className={isScanning ? "animate-spin" : ""}
                            />
                          </button>
                        </div>
                        <div className="mt-1 flex items-center justify-between gap-2 text-[10px] text-gray-600">
                          <span className="truncate">
                            {summary
                              ? `${summary.indexed_documents} 文档 / ${summary.chunks_indexed} 片段`
                              : scanStatusText(library.scan_status)}
                          </span>
                          {summary && summary.failed_documents > 0 && (
                            <span className="inline-flex items-center gap-0.5 text-amber-700 dark:text-amber-300">
                              <AlertTriangle size={9} />
                              {summary.failed_documents} 失败
                            </span>
                          )}
                        </div>
                      </li>
                    );
                  })}
                </ul>
              </>
            )}
          </div>
          {(libraryError || knowledgeError) && (
            <div className="mt-2 rounded border border-red-500/20 bg-red-500/10 px-2 py-1.5 text-[10px] text-red-700 dark:text-red-300 break-words">
              {libraryError || knowledgeError}
            </div>
          )}
        </section>

        <section>
          <div className="mb-1.5 flex items-center gap-1.5 px-1">
            <Sparkles size={11} className="text-gray-500" />
            <h3 className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">
              技能
            </h3>
            <span className="ml-auto text-[10px] text-gray-600">{enabled.length}</span>
          </div>
          {enabled.length === 0 ? (
            <div className="text-[11px] text-gray-600 text-center py-5 leading-relaxed">
              没有激活的技能<br />
              <span className="text-gray-700">到「技能库」里启用</span>
            </div>
          ) : (
            <ul className="space-y-1">
              {enabled.map((s) => (
                <li
                  key={s.id}
                  className="px-2 py-1.5 rounded border border-border bg-surface-2 hover:bg-surface-3 transition-colors"
                  title={s.description}
                >
                  <div className="flex items-center gap-1.5">
                    <Sparkles size={9} className="text-accent shrink-0" />
                    <span className="text-[11px] font-medium text-gray-300 truncate">
                      {s.name}
                    </span>
                  </div>
                  {s.description && (
                    <p className="text-[10px] text-gray-600 mt-0.5 line-clamp-2 leading-tight">
                      {s.description}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      {/* Memory increments — live learning events for this cwd */}
      <div className="border-t border-border px-3 py-3 max-h-[40%] overflow-y-auto">
        <div className="flex items-center gap-1.5 mb-1.5">
          <Brain size={11} className="text-gray-500" />
          <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">
            记忆增量
          </span>
          {(() => {
            const pending = learningEvents.filter((e) => e.status === "pending").length;
            return pending > 0 ? (
              <span className="ml-auto text-[10px] px-1.5 py-0.5 rounded bg-accent/15 text-accent">
                {pending} 待审
              </span>
            ) : null;
          })()}
        </div>
        {!cwd ? (
          <p className="text-[10px] text-gray-600">开个项目后这里会显示 AI 学到的事</p>
        ) : learningEvents.length === 0 ? (
          <p className="text-[10px] text-gray-600 leading-relaxed">
            AI 在本次会话中学到的事会出现在这里。<br />
            <span className="text-gray-700">任务/会话结束后自动总结。</span>
          </p>
        ) : (
          <ul className="space-y-1.5">
            {learningEvents
              .filter((e) => e.status === "pending")
              .slice(0, 5)
              .map((e) => (
                <li
                  key={e.id}
                  className="rounded border border-accent/30 bg-accent/5 p-1.5 space-y-1"
                >
                  <p className="text-[10px] text-gray-300 leading-snug line-clamp-2">
                    {e.observation}
                  </p>
                  <div className="flex items-center gap-1">
                    <button
                      onClick={() => cwd && void acceptLearning(e.id, cwd)}
                      className="text-[9px] px-1.5 py-0.5 rounded bg-accent text-white hover:bg-accent-hover"
                      title={e.kind === "preference" ? `更新偏好：${e.pref_key} = ${e.pref_value}` : "写入 memory.md"}
                    >
                      ✓ 采纳
                    </button>
                    <button
                      onClick={() => cwd && void rejectLearning(e.id, cwd)}
                      className="text-[9px] px-1.5 py-0.5 rounded text-gray-500 hover:bg-surface-3"
                    >
                      拒绝
                    </button>
                    <span className="ml-auto text-[8px] text-gray-600">
                      {e.kind === "preference" ? "偏好" : "记忆"}
                    </span>
                  </div>
                </li>
              ))}
            {learningEvents.filter((e) => e.status === "pending").length > 5 && (
              <li className="text-[9px] text-gray-600 text-center pt-1">
                还有 {learningEvents.filter((e) => e.status === "pending").length - 5} 条，
                到「我的画像 → 学习日志」全部审批
              </li>
            )}
            {learningEvents.filter((e) => e.status === "pending").length === 0 && (
              <li className="text-[10px] text-gray-600">
                所有学习事件已处理。
                <span className="text-gray-700"> 已采纳 {learningEvents.filter((e) => e.status === "accepted").length} · 拒绝 {learningEvents.filter((e) => e.status === "rejected").length}</span>
              </li>
            )}
          </ul>
        )}
      </div>
    </>
  );
}

function scanStatusText(status: string): string {
  switch (status) {
    case "ready": return "已索引";
    case "completed": return "已索引";
    case "completed_with_errors": return "部分失败";
    case "scanning": return "扫描中";
    case "failed": return "扫描失败";
    case "idle": return "待扫描";
    default: return "待扫描";
  }
}

function buildTaskConnectorContext(libraries: KnowledgeLibrary[]): TaskConnectorContext {
  return {
    knowledge_libraries: libraries.map((library) => ({
      id: library.id,
      name: library.name,
      root_path: library.root_path,
      scan_status: library.scan_status,
      last_scan_at: library.last_scan_at,
    })),
  };
}

function parseTaskConnectorContext(raw: string | null): TaskConnectorContext {
  if (!raw) return { knowledge_libraries: [] };
  try {
    const parsed = JSON.parse(raw) as Partial<TaskConnectorContext>;
    return {
      knowledge_libraries: Array.isArray(parsed.knowledge_libraries)
        ? parsed.knowledge_libraries.filter((library) => library && typeof library.id === "string")
        : [],
    };
  } catch {
    return { knowledge_libraries: [] };
  }
}

function KnowledgeContextPanel({
  knowledgeLibraries,
  compact = false,
}: {
  knowledgeLibraries: KnowledgeLibrary[];
  compact?: boolean;
}) {
  const hasWarnings = knowledgeLibraries.some((library) =>
    !["ready", "completed", "completed_with_errors"].includes(library.scan_status),
  );
  return (
    <section className={`${compact ? "mb-0" : "mb-3"} rounded border border-border bg-surface-2 px-3 py-2`}>
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex items-center gap-1.5 text-[11px] font-medium text-gray-300">
          <BookOpen size={11} className="text-accent" />
          任务上下文
        </div>
        <span className="inline-flex items-center gap-1 rounded border border-border bg-surface-1 px-2 py-0.5 text-[10px] text-gray-500">
          知识库 {knowledgeLibraries.length}
        </span>
        <span className="inline-flex items-center gap-1 rounded border border-border bg-surface-1 px-2 py-0.5 text-[10px] text-gray-500">
          <Puzzle size={10} className="text-accent" />
          kb_search / kb_get_chunk
        </span>
        {hasWarnings && (
          <span className="inline-flex items-center gap-1 text-[10px] text-amber-700 dark:text-amber-300">
            <AlertTriangle size={10} />
            有知识库尚未完成索引
          </span>
        )}
      </div>
      {knowledgeLibraries.length > 0 && (
        <ul className="mt-2 grid gap-1 sm:grid-cols-2">
          {knowledgeLibraries.map((library) => (
            <li
              key={library.id}
              className="min-w-0 rounded border border-border bg-surface-1 px-2 py-1"
              title={library.root_path}
            >
              <div className="truncate text-[11px] text-gray-300">{library.name}</div>
              <div className="flex items-center gap-1 text-[9px] text-gray-600">
                <span className="truncate">{scanStatusText(library.scan_status)}</span>
                {library.last_scan_at && <span className="shrink-0">已扫描</span>}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
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
