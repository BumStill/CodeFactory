// SPDX-License-Identifier: Apache-2.0
//
// ExecutionStream — live timeline of scheduler events during autonomous
// task execution. Renders above the chat MessageList so users can watch
// what the AI is doing in real time and intervene via the chat input
// below if they want to redirect.
//
// Events are pulled from useTasksStore.executionLog which the subscribe
// callback appends to as the Tauri events arrive.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  Play,
  CheckCircle2,
  XCircle,
  RefreshCw,
  ShieldCheck,
  Activity,
  FileDiff,
} from "lucide-react";
import { useTasksStore, type ExecutionEvent } from "../stores/tasks";
import { invoke } from "../lib/tauri";

interface Props {
  sessionId: string;
  /** Hide when there's no execution log to show (default). */
  hideWhenEmpty?: boolean;
}

export function ExecutionStream({ sessionId, hideWhenEmpty = true }: Props) {
  // Select the raw map slot — passing `s => s.executionLog[sessionId] ?? []`
  // creates a fresh array reference each render and triggers infinite
  // re-renders. Read the map, then memoise the fallback below.
  const logMap = useTasksStore((s) => s.executionLog);
  const log = useMemo(() => logMap[sessionId] ?? [], [logMap, sessionId]);
  const running = useTasksStore((s) => s.running[sessionId] ?? false);
  const tailRef = useRef<HTMLDivElement | null>(null);

  // Auto-scroll to the latest event when the log grows. We pin to bottom
  // unconditionally here because the chat MessageList below has its own
  // sticky-scroll logic; the stream box itself is short and append-only.
  // jsdom doesn't implement scrollIntoView — guard so the test env doesn't
  // throw before any assertions can fire.
  useEffect(() => {
    if (typeof tailRef.current?.scrollIntoView === "function") {
      tailRef.current.scrollIntoView({ block: "end", behavior: "smooth" });
    }
  }, [log.length]);

  if (hideWhenEmpty && log.length === 0) return null;

  // Group by taskId so each task gets a contiguous block, easier to skim.
  const byTask = groupByTask(log);

  return (
    <div className="border-b border-border bg-surface-1 max-h-64 overflow-y-auto shrink-0">
      <div className="sticky top-0 z-10 flex items-center justify-between gap-2 px-3 py-1.5 border-b border-border bg-surface-1">
        <div className="flex items-center gap-1.5">
          <Activity size={14} className={running ? "text-accent animate-pulse" : "text-gray-500"} />
          <span className="text-caption font-semibold text-gray-500">
            执行流
          </span>
          {running && <span className="text-caption text-accent">运行中</span>}
        </div>
        <span className="text-caption text-gray-600">{log.length} 条事件</span>
      </div>
      <ol className="px-3 py-2 space-y-2">
        {byTask.map(({ taskId, events }) => (
          <TaskBlock key={taskId} taskId={taskId} events={events} />
        ))}
      </ol>
      <div ref={tailRef} />
    </div>
  );
}

function groupByTask(log: ExecutionEvent[]): { taskId: string; events: ExecutionEvent[] }[] {
  const order: string[] = [];
  const buckets = new Map<string, ExecutionEvent[]>();
  for (const e of log) {
    if (!buckets.has(e.taskId)) {
      order.push(e.taskId);
      buckets.set(e.taskId, []);
    }
    buckets.get(e.taskId)!.push(e);
  }
  return order.map((taskId) => ({ taskId, events: buckets.get(taskId)! }));
}

function TaskBlock({ taskId, events }: { taskId: string; events: ExecutionEvent[] }) {
  // Pick the most recent terminal event (if any) for the block status.
  const lastTerminal = [...events]
    .reverse()
    .find((e) => e.kind === "task_completed" || e.kind === "task_failed");
  const title =
    events.find((e) => e.title)?.title ?? `Task ${taskId.slice(0, 6)}`;
  // task_completed may include the files the sub-agent touched + the cwd.
  // We surface them as a collapsible "N changed files" section so the user
  // can see what was actually written without spelunking through git.
  const completedEvent = events.find((e) => e.kind === "task_completed");
  const filesChanged = completedEvent?.filesChanged ?? [];
  const cwd = completedEvent?.cwd;

  const Icon = lastTerminal
    ? lastTerminal.kind === "task_completed"
      ? CheckCircle2
      : XCircle
    : Play;
  const iconColor = lastTerminal
    ? lastTerminal.kind === "task_completed"
      ? "text-green-700 dark:text-green-400"
      : "text-red-700 dark:text-red-400"
    : "text-accent";

  return (
    <li className="border border-border rounded bg-surface-2 overflow-hidden">
      <div className="flex items-center gap-1.5 px-2 py-1 border-b border-border bg-surface-3">
        <Icon size={14} className={`shrink-0 ${iconColor}`} />
        <span className="text-caption font-medium text-gray-200 truncate flex-1">{title}</span>
        <span className="text-caption text-gray-600 shrink-0">{events.length}</span>
      </div>
      <ul className="px-2 py-1 space-y-0.5">
        {events.map((e) => (
          <EventRow key={e.id} event={e} />
        ))}
      </ul>
      {filesChanged.length > 0 && cwd && (
        <ChangedFilesPanel files={filesChanged} cwd={cwd} />
      )}
    </li>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// ChangedFilesPanel — collapsible list of files the sub-agent touched, with
// per-file git diff fetched lazily on click. Keeps the AI's actual edits
// visible to the user immediately after task_completed, no spelunking
// required. Diff comes from the existing git_diff Tauri command and
// renders as a fenced-monospace block (the existing DiffViewer expects
// a structured parsed-diff format; we keep this lean and just show patch).
// ─────────────────────────────────────────────────────────────────────────────

function ChangedFilesPanel({ files, cwd }: { files: string[]; cwd: string }) {
  const [open, setOpen] = useState(false);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [diff, setDiff] = useState<string>("");
  const [loadingFile, setLoadingFile] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadDiff = async (file: string) => {
    if (activeFile === file) {
      setActiveFile(null);
      setDiff("");
      return;
    }
    setActiveFile(file);
    setLoadingFile(file);
    setError(null);
    try {
      const patch = await invoke<string>("git_diff", { cwd, file });
      setDiff(patch.trim() || "（无未提交改动；可能已被 checkpoint 提交）");
    } catch (e) {
      setError(String(e));
      setDiff("");
    } finally {
      setLoadingFile(null);
    }
  };

  return (
    <div className="border-t border-border bg-surface-1">
      <button
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-1.5 px-2 py-1 text-caption text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
      >
        <FileDiff size={14} />
        <span>改动 {files.length} 个文件</span>
        <span className="ml-auto">{open ? "收起" : "展开"}</span>
      </button>
      {open && (
        <div className="px-2 pb-2 space-y-1">
          {files.map((f) => (
            <div key={f}>
              <button
                onClick={() => void loadDiff(f)}
                className={`w-full flex items-center gap-1 px-1.5 py-0.5 text-caption rounded transition-colors text-left ${
                  activeFile === f
                    ? "bg-surface-3 text-accent"
                    : "text-gray-400 hover:text-gray-200 hover:bg-surface-3"
                }`}
              >
                <span className="font-mono truncate flex-1">{f}</span>
                {loadingFile === f && <span className="text-caption text-gray-600">读取中…</span>}
              </button>
              {activeFile === f && (
                <pre className="mt-1 mx-1 p-2 bg-surface-2 border border-border rounded text-caption font-mono text-gray-300 whitespace-pre-wrap break-all max-h-60 overflow-y-auto">
                  {diff}
                </pre>
              )}
            </div>
          ))}
          {error && (
            <p className="text-caption text-red-700 dark:text-red-300 px-1">{error}</p>
          )}
        </div>
      )}
    </div>
  );
}

function EventRow({ event }: { event: ExecutionEvent }) {
  const { Icon, color, text } = renderEvent(event);
  return (
    <li className="flex items-start gap-1.5 text-caption leading-snug">
      <Icon size={14} className={`mt-1 shrink-0 ${color}`} />
      <span className="text-gray-400 break-all">{text}</span>
      <span className="text-gray-600 shrink-0 text-caption ml-auto">
        {timeOnly(event.at)}
      </span>
    </li>
  );
}

function renderEvent(e: ExecutionEvent): {
  Icon: typeof Play;
  color: string;
  text: string;
} {
  switch (e.kind) {
    case "task_started":
      return { Icon: Play, color: "text-accent", text: `开始：${e.title ?? e.taskId}` };
    case "task_progress":
      return { Icon: Activity, color: "text-gray-500", text: e.message ?? "进度更新" };
    case "task_completed":
      return {
        Icon: CheckCircle2,
        color: "text-green-700 dark:text-green-400",
        text: e.result ? `完成：${e.result}` : "完成",
      };
    case "task_failed":
      return {
        Icon: XCircle,
        color: "text-red-700 dark:text-red-400",
        text: e.error ? `失败：${e.error}` : "失败",
      };
    case "task_retry":
      return {
        Icon: RefreshCw,
        color: "text-amber-700 dark:text-amber-300",
        text: e.message ?? "重试",
      };
    case "task_verification": {
      // Live per-criterion summary: green when all pass, red otherwise.
      const v = e.verification;
      if (v && v.length > 0) {
        const passed = v.filter((r) => r.passed).length;
        return {
          Icon: ShieldCheck,
          color:
            passed === v.length
              ? "text-green-700 dark:text-green-400"
              : "text-red-700 dark:text-red-400",
          text: `验证 · ${passed}/${v.length} 通过`,
        };
      }
      return {
        Icon: ShieldCheck,
        color: "text-blue-700 dark:text-blue-300",
        text: e.message ?? "验证",
      };
    }
  }
}

function timeOnly(ts: number): string {
  const d = new Date(ts);
  return d.toTimeString().slice(0, 8);
}
