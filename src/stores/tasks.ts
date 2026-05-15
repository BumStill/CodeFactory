// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "../lib/tauri";
import type {
  TaskDep,
  TaskEventPayload,
  TaskInput,
  TaskRun,
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

interface TasksState {
  /** Tasks keyed by session_id. */
  tasks: Record<string, TaskRun[]>;
  /** Whether the scheduler is active for the given session (best-effort). */
  running: Record<string, boolean>;
  /** Loading flag per session. */
  loading: Record<string, boolean>;
  /** Last error per session. */
  error: Record<string, string | null>;

  loadTasks: (sessionId: string) => Promise<void>;
  createTaskTree: (
    sessionId: string,
    tasks: TaskInput[],
    deps: TaskDep[],
  ) => Promise<string[]>;
  start: (sessionId: string, specReqId?: string, specTitle?: string) => Promise<void>;
  cancel: (sessionId: string) => Promise<void>;
  subscribe: (sessionId: string) => Promise<() => void>;
  subscribeEvidence: (sessionId: string) => Promise<() => void>;
  getTaskDependencies: (taskId: string) => Promise<string[]>;
  reset: (sessionId: string) => void;
}

export const useTasksStore = create<TasksState>((set, get) => ({
  tasks: {},
  running: {},
  loading: {},
  error: {},

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

  createTaskTree: async (sessionId, tasks, deps) => {
    const ids = await invoke<string[]>("create_task_tree", {
      sessionId,
      tasksIn: tasks,
      dependencies: deps,
    });
    await get().loadTasks(sessionId);
    return ids;
  },

  start: async (sessionId, specReqId, specTitle) => {
    set((s) => ({ running: { ...s.running, [sessionId]: true } }));
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
      const u = await listen<TaskEventPayload>(`${k}:${sessionId}`, () => {
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
