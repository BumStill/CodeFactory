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
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { ModelPicker } from "../../components/ModelPicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { ContextUsageBar } from "../../components/ContextUsageBar";
import { useChatStore } from "../../stores/chat";
import { useSettingsStore } from "../../stores/settings";
import { useTasksStore } from "../../stores/tasks";
import { useSkillsStore } from "../../stores/skills";
import type { Theme, TaskRun } from "../../lib/tauri";

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
  const { tasks, loadTasks, subscribe } = useTasksStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];

  useEffect(() => {
    loadTasks(sessionId);
    let unsub: (() => void) | undefined;
    subscribe(sessionId).then((u) => { unsub = u; });
    return () => { unsub?.(); };
  }, [sessionId]);

  return (
    <>
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">任务</h2>
        <button
          className="p-0.5 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          title="加任务（即将上线）"
        >
          <Plus size={12} />
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-2">
        {sessionTasks.length === 0 ? (
          <div className="text-[11px] text-gray-600 text-center py-8 leading-relaxed">
            还没有任务<br />
            <span className="text-gray-700">通过下方对话框描述需求<br />AI 会自动拆解</span>
          </div>
        ) : (
          <ul className="space-y-0.5">
            {buildTaskTree(sessionTasks).map(({ task, depth }) => (
              <TaskRow key={task.id} task={task} depth={depth} />
            ))}
          </ul>
        )}
      </div>
    </>
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
