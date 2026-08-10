// SPDX-License-Identifier: Apache-2.0
//
// Centralised updater state — used by both the floating UpdaterBanner and
// the always-visible header status pill (UpdateStatusPill). Putting this in
// a Zustand store means: only one /latest.json poll runs at a time no matter
// how many components observe it, and the "is there an update?" indicator
// stays in sync across the whole UI.

import { create } from "zustand";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "../lib/tauri";

const POLL_INTERVAL_MS = 30 * 60 * 1000; // 30 min — light on the GitHub CDN, fresh enough for users
const SAFE_RESTART_RETRY_MS = 5_000;

export interface UpdateSafetyStatus {
  safe_to_restart: boolean;
  restart_reserved: boolean;
  active_chat_turns: number;
  active_task_schedulers: number;
  active_delivery_leases: number;
  pending_permissions: number;
  managed_browser_sessions: number;
  terminal_sessions: number;
}

export function countUpdateBlockers(status: UpdateSafetyStatus | null): number {
  if (!status) return 1;
  return status.active_chat_turns
    + status.active_task_schedulers
    + status.active_delivery_leases
    + status.pending_permissions
    + status.managed_browser_sessions
    + status.terminal_sessions;
}

export type UpdaterPhase =
  | { kind: "idle" }                                                // first ever, no check yet
  | { kind: "checking" }
  | { kind: "up_to_date"; checkedAt: number }
  | { kind: "available"; update: Update; checkedAt: number }
  | { kind: "downloading"; received: number; total: number | null }
  | {
      kind: "waiting_for_safe_restart";
      update: Update;
      blockers: UpdateSafetyStatus | null;
      safetyCheckError: string | null;
      checkedAt: number;
    }
  | { kind: "installing" }
  | { kind: "ready" }
  | { kind: "error"; message: string; checkedAt: number };

interface UpdaterStore {
  phase: UpdaterPhase;
  currentVersion: string | null;
  pollHandle: number | null;
  safeRetryHandle: number | null;
  dismissedVersion: string | null;        // user dismissed this version's banner; new versions still show

  initialize: () => Promise<void>;        // start polling, fetch current version
  checkNow: () => Promise<void>;          // user-triggered check
  install: () => Promise<void>;
  dismiss: () => void;                    // hide banner for this version
}

export const useUpdaterStore = create<UpdaterStore>((set, get) => ({
  phase: { kind: "idle" },
  currentVersion: null,
  pollHandle: null,
  safeRetryHandle: null,
  dismissedVersion: null,

  initialize: async () => {
    // Resolve current installed version once.
    try {
      const v = await getVersion();
      set({ currentVersion: v });
    } catch (e) {
      console.warn("[updater] getVersion failed:", e);
    }

    // In dev, the unsigned bundle has no updater pubkey — checking would
    // always error. Skip the schedule entirely so DevTools stays quiet.
    if ((import.meta as { env?: { DEV?: boolean } }).env?.DEV) return;

    // Schedule the recurring poll, then kick one off immediately.
    if (get().pollHandle === null) {
      const handle = window.setInterval(() => { void get().checkNow(); }, POLL_INTERVAL_MS);
      set({ pollHandle: handle });
    }
    void get().checkNow();
  },

  checkNow: async () => {
    const phase = get().phase;
    // Don't disturb an in-flight install/download with a new check.
    if (
      phase.kind === "downloading" ||
      phase.kind === "waiting_for_safe_restart" ||
      phase.kind === "installing" ||
      phase.kind === "ready"
    ) {
      return;
    }
    set({ phase: { kind: "checking" } });
    try {
      const update = await check();
      if (update?.available) {
        set({ phase: { kind: "available", update, checkedAt: Date.now() } });
      } else {
        set({ phase: { kind: "up_to_date", checkedAt: Date.now() } });
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      set({ phase: { kind: "error", message: msg, checkedAt: Date.now() } });
      console.warn("[updater] check failed:", err);
    }
  },

  install: async () => {
    const phase = get().phase;
    if (phase.kind !== "available" && phase.kind !== "waiting_for_safe_restart") return;
    const update = phase.update;
    try {
      if (phase.kind === "available") {
        set({ phase: { kind: "downloading", received: 0, total: null } });
        await update.download((event) => {
          switch (event.event) {
            case "Started":
              set({ phase: { kind: "downloading", received: 0, total: event.data.contentLength ?? null } });
              break;
            case "Progress":
              set((s) =>
                s.phase.kind === "downloading"
                  ? { phase: { ...s.phase, received: s.phase.received + event.data.chunkLength } }
                  : s,
              );
              break;
            case "Finished":
              break;
          }
        });
      }

      let safety: UpdateSafetyStatus | null = null;
      let safetyCheckError: string | null = null;
      try {
        safety = await invoke<UpdateSafetyStatus>("reserve_update_install");
      } catch (error) {
        // Unknown safety is unsafe. Keep retrying locally instead of asking the
        // user to click install again or risking an in-flight session.
        safetyCheckError = error instanceof Error ? error.message : String(error);
      }

      if (!safety?.safe_to_restart || !safety.restart_reserved) {
        set({
          phase: {
            kind: "waiting_for_safe_restart",
            update,
            blockers: safety,
            safetyCheckError,
            checkedAt: Date.now(),
          },
        });
        if (get().safeRetryHandle === null) {
          const handle = window.setTimeout(() => {
            set({ safeRetryHandle: null });
            void get().install();
          }, SAFE_RESTART_RETRY_MS);
          set({ safeRetryHandle: handle });
        }
        return;
      }

      if (get().safeRetryHandle !== null) {
        window.clearTimeout(get().safeRetryHandle!);
        set({ safeRetryHandle: null });
      }
      set({ phase: { kind: "installing" } });
      await update.install();
      set({ phase: { kind: "ready" } });
      setTimeout(() => {
        void relaunch().catch(async (error) => {
          await invoke("release_update_install_reservation").catch(console.error);
          const message = error instanceof Error ? error.message : String(error);
          set({ phase: { kind: "error", message, checkedAt: Date.now() } });
        });
      }, 800);
    } catch (err) {
      await invoke("release_update_install_reservation").catch(console.error);
      const msg = err instanceof Error ? err.message : String(err);
      set({ phase: { kind: "error", message: msg, checkedAt: Date.now() } });
    }
  },

  dismiss: () => {
    const phase = get().phase;
    if (phase.kind === "available") {
      set({ dismissedVersion: phase.update.version });
    }
  },
}));
