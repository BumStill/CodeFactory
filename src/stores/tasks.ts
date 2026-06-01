// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "../lib/tauri";
import type {
  TaskConnectorContext,
  TaskDep,
  TaskEventPayload,
  TaskInput,
  TaskRun,
  VerificationResult,
} from "../lib/tauri";

// ── Evidence pack notification store ─────────────────────────────────────────

export interface EvidenceNotification {
  id: string;
  spec_req_id: string;
  spec_title: string;
  path: string;
  timestamp: number;
}

interface ToastState {
  notifications: EvidenceNotification[];
  addNotification: (n: Omit<EvidenceNotification, "id" | "timestamp">) => void;
  dismissNotification: (id: string) => void;
}

export const useToastStore = create<ToastState>((set) => ({
  notifications: [],
  addNotification: (n) => {
    const id = Math.random().toString(36).slice(2);
    const notification: EvidenceNotification = { ...n, id, timestamp: Date.now() };
    set((s) => ({ notifications: [...s.notifications, notification] }));
    // Auto-dismiss after 5 seconds
    setTimeout(() => {
      set((s) => ({ notifications: s.notifications.filter((x) => x.id !== id) }));
    }, 5000);
  },
  dismissNotification: (id) => {
    set((s) => ({ notifications: s.notifications.filter((n) => n.id !== id) }));
  },
}));

const POLL_INTERVAL_MS = 2000;

/** A single live event emitted by the scheduler during task execution. */
export interface ExecutionEvent {
  id: string;
  kind: "task_started" | "task_progress" | "task_completed" | "task_failed" | "task_retry" | "task_verification";
  taskId: string;
  title?: string;
  message?: string;
  result?: string;
  error?: string;
  /** Files the sub-agent touched. Populated on task_completed when the
   *  agent ran write/edit tools. Undefined on other event kinds. */
  filesChanged?: string[];
  /** Working dir of the task. Surfaced alongside filesChanged so the UI
   *  can call `git diff` in the right place when expanding the panel. */
  cwd?: string;
  /** Per-criterion verification results — present on task_verification events
   *  (the backend emits them; we capture them for a live pass/fail summary). */
  verification?: VerificationResult[];
  at: number;
}

interface TasksState {
  /** Tasks keyed by session_id. */
  tasks: Record<string, TaskRun[]>;
  /** Whether the scheduler is active for the given session (best-effort). */
  running: Record<string, boolean>;
  /** Loading flag per session. */
  loading: Record<string, boolean>;
  /** Last error per session. */
  error: Record<string, string | null>;
  /** Live execution event log keyed by session_id, append-only within a run. */
  executionLog: Record<string, ExecutionEvent[]>;

  loadTasks: (sessionId: string) => Promise<void>;
  createTaskTree: (
    sessionId: string,
    tasks: TaskInput[],
    deps: TaskDep[],
    context?: TaskConnectorContext,
  ) => Promise<string[]>;
  start: (sessionId: string, specReqId?: string, specTitle?: string) => Promise<void>;
  cancel: (sessionId: string) => Promise<void>;
  subscribe: (sessionId: string) => Promise<() => void>;
  subscribeEvidence: (sessionId: string) => Promise<() => void>;
  getTaskDependencies: (taskId: string) => Promise<string[]>;
  clearExecutionLog: (sessionId: string) => void;
  reset: (sessionId: string) => void;
}

export const useTasksStore = create<TasksState>((set, get) => ({
  tasks: {},
  running: {},
  loading: {},
  error: {},
  executionLog: {},

  clearExecutionLog: (sessionId) => {
    set((s) => ({ executionLog: { ...s.executionLog, [sessionId]: [] } }));
  },

  loadTasks: async (sessionId) => {
    set((s) => ({ loading: { ...s.loading, [sessionId]: true } }));
    try {
      const rows = await invoke<TaskRun[]>("list_tasks", { sessionId });
      set((s) => ({
        tasks: { ...s.tasks, [sessionId]: rows },
        loading: { ...s.loading, [sessionId]: false },
        error: { ...s.error, [sessionId]: null },
      }));
    } catch (e) {
      set((s) => ({
        loading: { ...s.loading, [sessionId]: false },
        error: { ...s.error, [sessionId]: String(e) },
      }));
    }
  },

  createTaskTree: async (sessionId, tasks, deps, context) => {
    const ids = await invoke<string[]>("create_task_tree", {
      sessionId,
      tasksIn: tasks,
      dependencies: deps,
      context: context ?? null,
    });
    await get().loadTasks(sessionId);
    return ids;
  },

  start: async (sessionId, specReqId, specTitle) => {
    set((s) => ({
      running: { ...s.running, [sessionId]: true },
      // Fresh log each run — otherwise the UI shows stale events from past runs.
      executionLog: { ...s.executionLog, [sessionId]: [] },
    }));
    try {
      await invoke("start_implementation", {
        sessionId,
        specReqId: specReqId ?? null,
        specTitle: specTitle ?? null,
      });
      await get().loadTasks(sessionId);
    } catch (e) {
      set((s) => ({
        running: { ...s.running, [sessionId]: false },
        error: { ...s.error, [sessionId]: String(e) },
      }));
      throw e;
    }
  },

  cancel: async (sessionId) => {
    try {
      await invoke("cancel_implementation", { sessionId });
    } finally {
      set((s) => ({ running: { ...s.running, [sessionId]: false } }));
      await get().loadTasks(sessionId);
    }
  },

  subscribe: async (sessionId) => {
    const refresh = () => {
      get().loadTasks(sessionId);
    };

    const kinds = [
      "task_started",
      "task_progress",
      "task_completed",
      "task_failed",
      "task_retry",
      "task_verification",
    ] as const;

    const unsubs: UnlistenFn[] = [];
    for (const k of kinds) {
      const u = await listen<TaskEventPayload>(`${k}:${sessionId}`, (event) => {
        // Append to the execution log so the UI can stream progress.
        const payload = event.payload;
        // Backend may attach files_changed + cwd on task_completed only.
        // Cast through Record because TaskEventPayload may not yet declare
        // these fields in the local type; the wire format does include them.
        const extra = payload as unknown as Record<string, unknown>;
        const filesChanged = Array.isArray(extra.files_changed)
          ? (extra.files_changed as string[])
          : undefined;
        const cwd = typeof extra.cwd === "string" ? extra.cwd : undefined;
        // task_verification carries structured per-criterion results on the
        // wire (same Record cast as files_changed); capture them here.
        const verification = Array.isArray(extra.results)
          ? (extra.results as VerificationResult[])
          : undefined;

        const entry: ExecutionEvent = {
          id: `${k}-${payload.task_id}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          kind: k,
          taskId: payload.task_id,
          title: payload.title,
          message: payload.message,
          result: payload.result,
          error: payload.error,
          filesChanged,
          cwd,
          verification,
          at: Date.now(),
        };
        set((s) => {
          const prev = s.executionLog[sessionId] ?? [];
          // Cap log size to keep memory bounded even for very long runs.
          // 500 entries × ~200 bytes ≈ 100 KB worst case, plenty for UX.
          const next = [...prev, entry].slice(-500);
          return { executionLog: { ...s.executionLog, [sessionId]: next } };
        });
        refresh();
        if (k === "task_completed" || k === "task_failed") {
          // After a task ends, check if anything's still pending or running.
          // We rely on loadTasks to update store; the running flag is best-effort.
          setTimeout(() => {
            const all = get().tasks[sessionId] ?? [];
            const stillBusy = all.some(
              (t) => t.status === "pending" || t.status === "running",
            );
            if (!stillBusy) {
              set((s) => ({
                running: { ...s.running, [sessionId]: false },
              }));
              // Self-evolution: one post-mortem pass per session run.
              // Best-effort + capped at 500 tokens server-side — see
              // src-tauri/src/commands/learning.rs for the token economy
              // rationale. Failure is silent; the next session will retry.
              const cwd = all[0]?.cwd;
              if (cwd) {
                invoke("run_postmortem", { sessionId, cwd }).catch((e) => {
                  // eslint-disable-next-line no-console
                  console.warn("postmortem failed (non-fatal)", e);
                });
              }
            }
          }, 100);
        }
      });
      unsubs.push(u);
    }

    // Polling fallback in case events miss.
    const interval = window.setInterval(() => {
      const all = get().tasks[sessionId] ?? [];
      const busy = all.some(
        (t) => t.status === "pending" || t.status === "running",
      );
      if (busy) refresh();
    }, POLL_INTERVAL_MS);

    return () => {
      for (const u of unsubs) u();
      window.clearInterval(interval);
    };
  },

  subscribeEvidence: async (sessionId) => {
    const u = await listen<{ spec_req_id: string; spec_title: string; path: string }>(
      `evidence_pack_ready:${sessionId}`,
      (e) => {
        useToastStore.getState().addNotification({
          spec_req_id: e.payload.spec_req_id,
          spec_title: e.payload.spec_title,
          path: e.payload.path,
        });
      }
    );
    return u;
  },

  getTaskDependencies: async (taskId) => {
    return invoke<string[]>("get_task_dependencies", { taskId });
  },

  reset: (sessionId) => {
    set((s) => ({
      tasks: { ...s.tasks, [sessionId]: [] },
      running: { ...s.running, [sessionId]: false },
      error: { ...s.error, [sessionId]: null },
    }));
  },
}));
