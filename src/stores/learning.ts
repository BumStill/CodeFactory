// SPDX-License-Identifier: Apache-2.0
//
// Shared learning-event store.
//
// Replaces the per-component invoke calls scattered across ProfilePage +
// (soon) Workspace right column. One store means:
//   1. Workspace's "记忆增量" rail and Profile's "学习日志" stay in sync
//      automatically — accept/reject in one updates the other.
//   2. Live updates via the backend `learning_events_updated:{cwd}` event
//      fired by run_postmortem / accept / reject — no polling.
//
// Per-cwd scoping mirrors the backend table layout. Two open projects
// don't trample each other.

import { create } from "zustand";
import { invoke } from "../lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface LearningEvent {
  id: string;
  session_id: string;
  cwd: string;
  observation: string;
  suggestion: string;
  status: "pending" | "accepted" | "rejected";
  created_at: string;
  decided_at: string | null;
  /** "memory" → suggestion appended to .codefactory/memory.md on accept.
   *  "preference" → pref_key→pref_value upserted into user_preferences. */
  kind: "memory" | "preference";
  pref_key: string | null;
  pref_value: string | null;
}

interface LearningStore {
  /** Per-cwd cache of recent events. */
  events: Record<string, LearningEvent[]>;
  /** Per-cwd loading flag for UI skeleton states. */
  loading: Record<string, boolean>;
  /** Per-cwd active unlisten handle. We hold ONE listener per cwd so
   *  duplicate subscribe() calls are no-ops. */
  _unlisten: Record<string, UnlistenFn>;

  load: (cwd: string) => Promise<LearningEvent[]>;
  subscribe: (cwd: string) => Promise<() => void>;
  accept: (id: string, cwd: string) => Promise<void>;
  reject: (id: string, cwd: string) => Promise<void>;
}

export const useLearningStore = create<LearningStore>((set, get) => ({
  events: {},
  loading: {},
  _unlisten: {},

  load: async (cwd) => {
    set((s) => ({ loading: { ...s.loading, [cwd]: true } }));
    try {
      const list = await invoke<LearningEvent[]>("list_learning_events", { cwd });
      set((s) => ({
        events: { ...s.events, [cwd]: list },
        loading: { ...s.loading, [cwd]: false },
      }));
      return list;
    } catch (e) {
      set((s) => ({ loading: { ...s.loading, [cwd]: false } }));
      throw e;
    }
  },

  subscribe: async (cwd) => {
    // Single subscription per cwd — don't double-listen.
    if (get()._unlisten[cwd]) {
      return () => {
        const u = get()._unlisten[cwd];
        u?.();
        set((s) => {
          const { [cwd]: _drop, ...rest } = s._unlisten;
          return { _unlisten: rest };
        });
      };
    }
    const un = await listen<LearningEvent[] | null>(
      `learning_events_updated:${cwd}`,
      () => {
        // Always re-fetch to keep status changes (accept/reject) honest.
        // Payload may contain only newly-created events, not updated ones.
        void get().load(cwd);
      },
    );
    set((s) => ({ _unlisten: { ...s._unlisten, [cwd]: un } }));
    return () => {
      un();
      set((s) => {
        const { [cwd]: _drop, ...rest } = s._unlisten;
        return { _unlisten: rest };
      });
    };
  },

  accept: async (id, cwd) => {
    await invoke("accept_learning_event", { eventId: id });
    // Backend emits learning_events_updated — subscribers refresh.
    // For pages without an active subscription, also patch optimistically.
    set((s) => ({
      events: {
        ...s.events,
        [cwd]: (s.events[cwd] ?? []).map((e) =>
          e.id === id ? { ...e, status: "accepted", decided_at: new Date().toISOString() } : e,
        ),
      },
    }));
  },

  reject: async (id, cwd) => {
    await invoke("reject_learning_event", { eventId: id });
    set((s) => ({
      events: {
        ...s.events,
        [cwd]: (s.events[cwd] ?? []).map((e) =>
          e.id === id ? { ...e, status: "rejected", decided_at: new Date().toISOString() } : e,
        ),
      },
    }));
  },
}));
