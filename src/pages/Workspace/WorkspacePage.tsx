// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import {
  ChevronLeft,
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
  Sparkles,
  Trash2,
  Wand2,
  X,
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { ModelPicker } from "../../components/ModelPicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { ContextUsageBar } from "../../components/ContextUsageBar";
import { invoke } from "../../lib/tauri";
import { useChatStore } from "../../stores/chat";
import { useSettingsStore } from "../../stores/settings";
import { useTasksStore } from "../../stores/tasks";
import { useSkillsStore } from "../../stores/skills";
import type { Theme, TaskRun, TaskInput, TaskDep } from "../../lib/tauri";

interface DecomposedTask {
  tmp_id: string;
  title: string;
  description: string;
  dependencies: string[];
}

interface WorkspacePageProps {
  sessionId: string;
  onBackHome: () => void;
  onOpenSettings: () => void;
}

/**
 * Workspace — the new primary working surface (replaces ChatPage as default).
 *
 * Three-column layout:
 *   Left   — Task tree for this project (persistent unit)
 *   Center — Execution stream (AI work in progress + chat input)
 *   Right  — Active skills + memory increments (transparency surface)
 */
export function WorkspacePage({ sessionId, onBackHome, onOpenSettings }: WorkspacePageProps) {
  const {
    activeSession, messages, streaming,
    selectSession, sendMessage, cancelStream,
    pendingPermission, respondPermission,
  } = useChatStore();
  const { settings, setTheme } = useSettingsStore();
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);

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
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium text-gray-200 truncate">
            {activeSession?.title || "..."}
          </div>
          <div className="text-[10px] text-gray-600 font-mono truncate">
            {activeSession?.cwd}
          </div>
        </div>
        <ModelPicker />

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
          onClick={onOpenSettings}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="设置"
        >
          <SettingsIcon size={14} />
        </button>
      </header>

      {/* ── Body: 3 columns ──────────────────────────────────────────────── */}
      <div className="flex-1 flex min-h-0">

        {/* ─── Left: Task tree ────────────────────────────────────────── */}
        <aside className="w-64 shrink-0 border-r border-border bg-surface-1 flex flex-col">
          <TasksColumn sessionId={sessionId} />
        </aside>

        {/* ─── Center: Execution stream + input ──────────────────────── */}
        <main className="flex-1 flex flex-col min-w-0">
          <MessageList
            messages={messages}
            streaming={streaming}
            cwd={activeSession?.cwd ?? null}
            onUsePrompt={(text) => setPendingInsert(text)}
          />
          <ContextUsageBar sessionId={activeSession?.id} />
          <MessageInput
            onSend={sendMessage}
            onCancel={cancelStream}
            streaming={streaming}
            disabled={!activeSession}
            pendingInsert={pendingInsert}
            onInsertConsumed={() => setPendingInsert(undefined)}
          />
        </main>

        {/* ─── Right: Active skills + memory ─────────────────────────── */}
        <aside className="w-60 shrink-0 border-l border-border bg-surface-1 flex flex-col">
          <SkillsColumn />
        </aside>
      </div>

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
  const { tasks, loadTasks, subscribe, createTaskTree } = useTasksStore();
  const { activeSession } = useChatStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const [creatorOpen, setCreatorOpen] = useState(false);

  useEffect(() => {
    loadTasks(sessionId);
    let unsub: (() => void) | undefined;
    subscribe(sessionId).then((u) => { unsub = u; });
    return () => { unsub?.(); };
  }, [sessionId]);

  const handleConfirm = async (decomposed: DecomposedTask[]) => {
    const cwd = activeSession?.cwd ?? "";
    const inputs: TaskInput[] = decomposed.map((d) => ({
      tmp_id: d.tmp_id,
      title: d.title,
      description: d.description,
      cwd,
    }));
    const deps: TaskDep[] = decomposed.flatMap((d) =>
      d.dependencies.map((depId) => ({
        task_tmp_id: d.tmp_id,
        depends_on_tmp_id: depId,
      }))
    );
    await createTaskTree(sessionId, inputs, deps);
    setCreatorOpen(false);
  };

  return (
    <>
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">任务</h2>
        <button
          onClick={() => setCreatorOpen(true)}
          className="p-0.5 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="AI 拆解需求为任务"
        >
          <Plus size={12} />
        </button>
      </div>
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
      {creatorOpen && (
        <TaskCreatorModal
          onCancel={() => setCreatorOpen(false)}
          onConfirm={handleConfirm}
        />
      )}
    </>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// TaskCreatorModal — describe → AI decompose → review → create
// ─────────────────────────────────────────────────────────────────────────────

interface TaskCreatorModalProps {
  onCancel: () => void;
  onConfirm: (tasks: DecomposedTask[]) => Promise<void>;
}

function TaskCreatorModal({ onCancel, onConfirm }: TaskCreatorModalProps) {
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
      const result = await invoke<DecomposedTask[]>("decompose_request_to_tasks", {
        request: request.trim(),
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
  return (
    <li
      className="group flex items-start gap-2 px-1.5 py-1 rounded hover:bg-surface-3 transition-colors cursor-default"
      style={{ paddingLeft: `${0.375 + depth * 0.875}rem` }}
    >
      <Icon
        size={11}
        className={`mt-1 shrink-0 ${statusColor(task.status)} ${
          task.status === "running" ? "animate-spin" : ""
        }`}
      />
      <span className="text-[11px] text-gray-300 leading-snug line-clamp-2 flex-1">
        {task.title}
      </span>
    </li>
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
// SkillsColumn — shows what capabilities are currently active
// ─────────────────────────────────────────────────────────────────────────────

function SkillsColumn() {
  const { skills, loadSkills } = useSkillsStore();

  useEffect(() => { loadSkills(); }, []);

  const enabled = skills.filter((s) => s.enabled);

  return (
    <>
      <div className="flex items-center gap-1.5 px-3 py-2 border-b border-border">
        <Puzzle size={11} className="text-gray-500" />
        <h2 className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">
          激活的能力
        </h2>
        <span className="ml-auto text-[10px] text-gray-600">{enabled.length}</span>
      </div>
      <div className="flex-1 overflow-y-auto p-2">
        {enabled.length === 0 ? (
          <div className="text-[11px] text-gray-600 text-center py-8 leading-relaxed">
            没有激活的能力<br />
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
      </div>

      {/* Memory placeholder — to be filled when self-evolution lands */}
      <div className="border-t border-border px-3 py-3">
        <div className="text-[10px] font-semibold uppercase tracking-wider text-gray-500 mb-1.5">
          记忆增量
        </div>
        <p className="text-[10px] text-gray-600 leading-relaxed">
          AI 在本次任务中学到的事会出现在这里。<br />
          <span className="text-gray-700">（自进化能力开发中）</span>
        </p>
      </div>
    </>
  );
}
