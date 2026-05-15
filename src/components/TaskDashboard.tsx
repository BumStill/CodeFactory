// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useRef, useState } from "react";
import {
  X,
  Play,
  Square,
  RefreshCw,
  Plus,
  Loader2,
  CheckCircle2,
  XCircle,
  Circle,
  AlertCircle,
  ChevronRight,
  ChevronDown,
  ShieldCheck,
} from "lucide-react";
import { invoke } from "../lib/tauri";
import { useTasksStore } from "../stores/tasks";
import type { TaskInput, TaskRun, TaskStatus, TaskDep, VerificationResult } from "../lib/tauri";

interface Props {
  sessionId: string;
  cwd: string;
  onClose: () => void;
}

export function TaskDashboard({ sessionId, cwd, onClose }: Props) {
  const {
    tasks,
    loading,
    error,
    running,
    loadTasks,
    createTaskTree,
    start,
    cancel,
    subscribe,
    subscribeEvidence,
  } = useTasksStore();

  const sessionTasks = tasks[sessionId] ?? [];
  const isLoading = loading[sessionId] ?? false;
  const isRunning = running[sessionId] ?? false;
  const sessionError = error[sessionId];
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [creatingDemo, setCreatingDemo] = useState(false);
  // Local cache of verification results so the dashboard updates immediately
  // after "Run Verification" without waiting for a full loadTasks() round-trip.
  const [verifCache, setVerifCache] = useState<Record<string, VerificationResult[]>>({});

  const handleVerificationRun = (taskId: string, results: VerificationResult[]) => {
    setVerifCache((prev) => ({ ...prev, [taskId]: results }));
  };

  // Subscribe to live task events for this session.
  const unsubRef = useRef<(() => void) | null>(null);
  const unsubEvidenceRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    loadTasks(sessionId);
    let mounted = true;
    subscribe(sessionId).then((un) => {
      if (!mounted) { un(); return; }
      unsubRef.current = un;
    });
    subscribeEvidence(sessionId).then((un) => {
      if (!mounted) { un(); return; }
      unsubEvidenceRef.current = un;
    });
    return () => {
      mounted = false;
      unsubRef.current?.();
      unsubRef.current = null;
      unsubEvidenceRef.current?.();
      unsubEvidenceRef.current = null;
    };
  }, [sessionId, loadTasks, subscribe, subscribeEvidence]);

  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const grouped = useMemo(() => groupTasks(sessionTasks), [sessionTasks]);

  const total = sessionTasks.length;
  const completed = sessionTasks.filter((t) => t.status === "completed").length;
  const failed = sessionTasks.filter((t) => t.status === "failed").length;
  const cancelled = sessionTasks.filter((t) => t.status === "cancelled").length;
  const settled = completed + failed + cancelled;
  const hasActive = sessionTasks.some(
    (t) => t.status === "pending" || t.status === "running",
  );

  const handleAddDemo = async () => {
    setCreatingDemo(true);
    try {
      const { tasks: demoTasks, deps: demoDeps } = buildDemoTaskTree(cwd);
      await createTaskTree(sessionId, demoTasks, demoDeps);
    } finally {
      setCreatingDemo(false);
    }
  };

  const handleStart = async () => {
    try {
      await start(sessionId);
    } catch (e) {
      // error already in store
    }
  };

  return (
    <div className="fixed right-0 top-0 bottom-0 z-40 w-[640px] max-w-[80vw] bg-surface-1 border-l border-border shadow-2xl flex flex-col">
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <span className="text-xs font-semibold text-gray-200 flex-1">Tasks</span>
        <button
          onClick={() => loadTasks(sessionId)}
          disabled={isLoading}
          className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors disabled:opacity-40"
          title="Refresh"
        >
          <RefreshCw size={12} className={isLoading ? "animate-spin" : ""} />
        </button>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors"
          title="Close"
        >
          <X size={14} />
        </button>
      </div>

      {/* Action / progress bar */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-border bg-surface-2 shrink-0">
        <button
          onClick={handleAddDemo}
          disabled={creatingDemo}
          className="flex items-center gap-1 px-2 py-1 text-[11px] rounded bg-surface-3 hover:bg-surface-4 text-gray-300 disabled:opacity-40 transition-colors"
          title="Insert a demo 5-task tree for testing"
        >
          <Plus size={11} />
          Add demo task tree
        </button>
        <span className="flex-1" />
        {total > 0 && (
          <span className="text-[11px] text-gray-500">
            {settled}/{total} done{failed > 0 ? `  · ${failed} failed` : ""}
          </span>
        )}
        {hasActive && !isRunning && (
          <button
            onClick={handleStart}
            className="flex items-center gap-1 px-2 py-1 text-[11px] rounded bg-accent hover:bg-accent-hover text-white transition-colors"
          >
            <Play size={11} />
            Start
          </button>
        )}
        {isRunning && (
          <button
            onClick={() => cancel(sessionId)}
            className="flex items-center gap-1 px-2 py-1 text-[11px] rounded bg-red-700/70 hover:bg-red-700 text-white transition-colors"
          >
            <Square size={11} />
            Cancel
          </button>
        )}
      </div>

      {/* Progress bar */}
      {total > 0 && (
        <div className="h-0.5 bg-surface-2 shrink-0">
          <div
            className="h-full bg-accent transition-all"
            style={{ width: `${(settled / total) * 100}%` }}
          />
        </div>
      )}

      {sessionError && (
        <div className="px-3 py-1.5 text-[11px] text-red-400 border-b border-border bg-red-950/20 shrink-0">
          {sessionError}
        </div>
      )}

      {/* Body */}
      <div className="flex-1 overflow-y-auto">
        {sessionTasks.length === 0 && !isLoading && (
          <div className="px-4 py-8 text-center text-[11px] text-gray-700">
            No tasks yet. Click "Add demo task tree" to insert a 5-task example.
          </div>
        )}

        {(["running", "pending", "completed", "failed", "cancelled"] as const).map(
          (group) => {
            const items = grouped[group];
            if (items.length === 0) return null;
            return (
              <div key={group}>
                <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-gray-600 bg-surface-2 sticky top-0 z-10">
                  {group} ({items.length})
                </div>
                {items.map((t) => {
                  // Merge local verification cache so the UI reflects just-run results.
                  const cached = verifCache[t.id];
                  const mergedTask = cached
                    ? { ...t, verification_results: JSON.stringify(cached) }
                    : t;
                  return (
                    <TaskRow
                      key={t.id}
                      task={mergedTask}
                      sessionId={sessionId}
                      expanded={expanded.has(t.id)}
                      onToggle={() => toggleExpand(t.id)}
                      onVerificationRun={handleVerificationRun}
                    />
                  );
                })}
              </div>
            );
          },
        )}
      </div>
    </div>
  );
}

function groupTasks(tasks: TaskRun[]): Record<TaskStatus, TaskRun[]> {
  const out: Record<TaskStatus, TaskRun[]> = {
    running: [],
    pending: [],
    completed: [],
    failed: [],
    cancelled: [],
  };
  for (const t of tasks) {
    out[t.status].push(t);
  }
  return out;
}

interface RowProps {
  task: TaskRun;
  sessionId: string;
  expanded: boolean;
  onToggle: () => void;
  onVerificationRun: (taskId: string, results: VerificationResult[]) => void;
}

function TaskRow({ task, sessionId, expanded, onToggle, onVerificationRun }: RowProps) {
  const dur = computeDuration(task);
  const result = parseResult(task.result);
  const verificationResults = parseVerification(task.verification_results);
  const [runningVerif, setRunningVerif] = useState(false);
  const [expandedChecks, setExpandedChecks] = useState<Set<number>>(new Set());

  const verifBadge = verificationResults
    ? verificationResults.every((r) => r.passed)
      ? "pass"
      : "fail"
    : null;

  const handleRunVerif = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setRunningVerif(true);
    try {
      const results = await invoke<VerificationResult[]>("run_verification_now", {
        sessionId,
        taskId: task.id,
      });
      onVerificationRun(task.id, results);
    } catch (err) {
      console.error("Verification failed:", err);
    } finally {
      setRunningVerif(false);
    }
  };

  const toggleCheck = (idx: number) => {
    setExpandedChecks((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  };

  return (
    <div className="border-b border-border/50">
      <div
        className="flex items-start gap-2 px-2 py-1.5 cursor-pointer hover:bg-surface-2 transition-colors"
        onClick={onToggle}
      >
        <span className="mt-0.5 shrink-0">
          {expanded ? <ChevronDown size={12} className="text-gray-500" /> : <ChevronRight size={12} className="text-gray-500" />}
        </span>
        <span className="mt-0.5 shrink-0">{statusIcon(task.status)}</span>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-[12px] text-gray-200 truncate">{task.title}</span>
            {verifBadge === "pass" && (
              <span title="All verification checks passed"><CheckCircle2 size={11} className="text-green-400 shrink-0" /></span>
            )}
            {verifBadge === "fail" && (
              <span title="One or more verification checks failed"><XCircle size={11} className="text-red-400 shrink-0" /></span>
            )}
          </div>
          <div className="flex items-center gap-2 text-[10px] text-gray-600">
            {dur && <span>{dur}</span>}
            {task.attempt_count > 1 && (
              <span className="text-yellow-500">attempt #{task.attempt_count}</span>
            )}
            {task.status === "failed" && task.error && (
              <span className="text-red-400 truncate">{task.error}</span>
            )}
          </div>
        </div>
      </div>

      {expanded && (
        <div className="px-3 pb-2 pt-1 bg-surface-1/50 text-[11px] space-y-2">
          <div>
            <span className="text-gray-600">Description: </span>
            <span className="text-gray-300 whitespace-pre-wrap">{task.description}</span>
          </div>
          <div>
            <span className="text-gray-600">cwd: </span>
            <code className="text-gray-400">{task.cwd}</code>
          </div>
          {task.sub_session_id && (
            <div>
              <span className="text-gray-600">Sub-session: </span>
              <code className="text-gray-500 truncate">{task.sub_session_id}</code>
            </div>
          )}
          {task.error && (
            <div className="text-red-400">
              <span className="text-gray-600">Error: </span>
              <span className="whitespace-pre-wrap">{task.error}</span>
            </div>
          )}
          {result && (
            <div className="space-y-1">
              {result.summary && (
                <div>
                  <span className="text-gray-600">Summary:</span>
                  <div className="mt-1 p-2 rounded bg-surface-2 text-gray-300 whitespace-pre-wrap font-mono text-[10px] max-h-48 overflow-y-auto">
                    {result.summary}
                  </div>
                </div>
              )}
              {result.files_changed && result.files_changed.length > 0 && (
                <div>
                  <span className="text-gray-600">Files changed:</span>
                  <ul className="mt-1 ml-3 list-disc text-gray-400">
                    {result.files_changed.map((f) => (
                      <li key={f}>
                        <code>{f}</code>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {typeof result.tool_calls_count === "number" && (
                <div className="text-gray-600">
                  Tool calls: <span className="text-gray-400">{result.tool_calls_count}</span>
                </div>
              )}
            </div>
          )}

          {/* ── Verification section ── */}
          <div className="pt-1 border-t border-border/30">
            <div className="flex items-center gap-2 mb-1">
              <span className="text-gray-600 flex items-center gap-1">
                <ShieldCheck size={11} />
                Verification
              </span>
              <button
                onClick={handleRunVerif}
                disabled={runningVerif}
                className="flex items-center gap-1 px-1.5 py-0.5 text-[10px] rounded bg-surface-3 hover:bg-surface-4 text-gray-400 hover:text-gray-200 disabled:opacity-40 transition-colors"
                title="Run verification checks now"
              >
                {runningVerif
                  ? <Loader2 size={10} className="animate-spin" />
                  : <RefreshCw size={10} />
                }
                Run
              </button>
            </div>

            {verificationResults && verificationResults.length > 0 ? (
              <div className="space-y-1">
                {verificationResults.map((r, idx) => (
                  <div key={idx} className="rounded bg-surface-2">
                    <div
                      className="flex items-center gap-2 px-2 py-1 cursor-pointer"
                      onClick={() => toggleCheck(idx)}
                    >
                      {r.passed
                        ? <CheckCircle2 size={11} className="text-green-400 shrink-0" />
                        : <XCircle size={11} className="text-red-400 shrink-0" />
                      }
                      <span className="flex-1 text-gray-300 truncate">{r.check}</span>
                      <span className="text-gray-600">{r.duration_ms}ms</span>
                      {expandedChecks.has(idx)
                        ? <ChevronDown size={10} className="text-gray-600" />
                        : <ChevronRight size={10} className="text-gray-600" />
                      }
                    </div>
                    {expandedChecks.has(idx) && r.output && (
                      <pre className="px-2 pb-2 text-[10px] text-gray-400 whitespace-pre-wrap font-mono max-h-40 overflow-y-auto">
                        {r.output}
                      </pre>
                    )}
                  </div>
                ))}
              </div>
            ) : (
              <span className="text-gray-700 text-[10px]">Not run yet</span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function statusIcon(status: TaskStatus) {
  switch (status) {
    case "pending":
      return <Circle size={12} className="text-gray-500" />;
    case "running":
      return <Loader2 size={12} className="text-blue-400 animate-spin" />;
    case "completed":
      return <CheckCircle2 size={12} className="text-green-400" />;
    case "failed":
      return <XCircle size={12} className="text-red-400" />;
    case "cancelled":
      return <AlertCircle size={12} className="text-yellow-500" />;
  }
}

function computeDuration(task: TaskRun): string | null {
  if (!task.started_at) return null;
  const start = Date.parse(task.started_at);
  const end = task.completed_at ? Date.parse(task.completed_at) : Date.now();
  if (Number.isNaN(start)) return null;
  const ms = Math.max(0, end - start);
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const min = Math.floor(ms / 60_000);
  const sec = Math.floor((ms % 60_000) / 1000);
  return `${min}m${sec}s`;
}

interface ParsedResult {
  summary?: string;
  files_changed?: string[];
  tool_calls_count?: number;
  completed?: boolean;
  sub_session_id?: string;
}

function parseResult(raw: string | null): ParsedResult | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as ParsedResult;
  } catch {
    return { summary: raw };
  }
}

function parseVerification(raw: string | null): VerificationResult[] | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as VerificationResult[];
  } catch {
    return null;
  }
}

/**
 * Phase 2 demo task tree. Five tasks with two dependency edges so we can
 * eyeball the parallel scheduler: T1/T3/T4 should kick off together,
 * T2 waits for T1, T5 waits for T2/T3/T4.
 */
function buildDemoTaskTree(cwd: string): { tasks: TaskInput[]; deps: TaskDep[] } {
  const tasks: TaskInput[] = [
    {
      tmp_id: "T1",
      title: "Read project structure",
      description:
        "Use the `glob` tool to list the top-level files in this repository. Briefly describe what kind of project it appears to be.",
      cwd,
    },
    {
      tmp_id: "T2",
      title: "Identify main entry points",
      description:
        "Find the main entry point(s) for this project (e.g. main.rs, index.ts, App.tsx, lib.rs). Use `glob` and `read_file`. Report the paths and a one-line summary of each.",
      cwd,
    },
    {
      tmp_id: "T3",
      title: "Count lines of Rust code",
      description:
        "Use `glob` to find all `**/*.rs` files (excluding `target/`), then read a few of them and estimate the total lines of Rust code in the project. Report your estimate and method.",
      cwd,
    },
    {
      tmp_id: "T4",
      title: "List all TypeScript components",
      description:
        "Use `glob` to find all `**/components/**/*.tsx` files. Report the file list and a one-line description of what each component appears to do (based on its filename).",
      cwd,
    },
    {
      tmp_id: "T5",
      title: "Summarize findings",
      description:
        "Based on a quick re-read of the project's top-level files (e.g. README, package.json, Cargo.toml), produce a short summary of the project: name, primary languages, purpose, and notable subsystems.",
      cwd,
    },
  ];

  const deps: TaskDep[] = [
    { task_tmp_id: "T2", depends_on_tmp_id: "T1" },
    { task_tmp_id: "T5", depends_on_tmp_id: "T2" },
    { task_tmp_id: "T5", depends_on_tmp_id: "T3" },
    { task_tmp_id: "T5", depends_on_tmp_id: "T4" },
  ];

  return { tasks, deps };
}
