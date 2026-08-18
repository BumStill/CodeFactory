// SPDX-License-Identifier: Apache-2.0
//
// Centralised updater state — used by both the floating UpdaterBanner and
// the always-visible header status pill (UpdateStatusPill). Putting this in
// a Zustand store means: only one /latest.json poll runs at a time no matter
// how many components observe it, and the "is there an update?" indicator
// stays in sync across the whole UI.

import { create } from "zustand";
import { check, type Update } from "@tauri-apps/plugin-updater";
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
  active_objective_leases: number;
  objective_blocker_owners: string[];
  pending_permissions: number;
  managed_browser_sessions: number;
  terminal_sessions: number;
  update_objective_id?: string | null;
  update_install_state?:
    | "observe_only" // legacy read-only projection; never grants install authority
    | "queued"
    | "install_permitted"
    | "definitely_not_applied"
    | "still_unknown"
    | "conflict"
    | "applied"
    | null;
  update_receipt_id?: string | null;
  target_version?: string | null;
  target_build?: string | null;
}

export interface UpdateInstallObservation {
  id: string;
  objective_id: string | null;
  target_version: string;
  target_build: string;
  state:
    | "install_permitted"
    | "definitely_not_applied"
    | "still_unknown"
    | "conflict"
    | "applied";
  recovery_replay_count: number;
  observed_at: number;
}

export function countUpdateBlockers(status: UpdateSafetyStatus | null): number {
  if (!status) return 1;
  return status.active_chat_turns
    + status.active_task_schedulers
    + status.active_delivery_leases
    + (status.active_objective_leases ?? 0)
    + status.pending_permissions
    + status.managed_browser_sessions
    + status.terminal_sessions;
}

export function describeUpdateObjectiveBlockers(status: UpdateSafetyStatus | null): string | null {
  const count = status?.active_objective_leases ?? 0;
  if (count === 0) return null;
  const owners = [...new Set(status?.objective_blocker_owners ?? [])];
  const ownerText = owners.length > 0 ? owners.join("、") : "系统恢复控制面";
  return `${count} 个目标仍由 ${ownerText} 持有`;
}

function targetBuildIdentity(update: Update): string | null {
  const raw = (update as Update & { rawJson?: Record<string, unknown> }).rawJson ?? {};
  const value = raw.build_git_sha;
  return typeof value === "string" && value.trim() ? value.trim() : null;
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
    try {
      const observation = await invoke<UpdateInstallObservation | null>("observe_update_install");
      if (observation?.state === "applied") {
        set({
          currentVersion: observation.target_version,
          phase: { kind: "up_to_date", checkedAt: Date.now() },
        });
      }
    } catch (e) {
      // Observation failure is fail-closed by reserve_update_install: an
      // unresolved receipt still cannot become a second install admission.
      console.warn("[updater] prior install observation failed:", e);
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
    const targetBuild = targetBuildIdentity(update);
    if (!targetBuild) {
      set({
        phase: {
          kind: "waiting_for_safe_restart",
          update,
          blockers: null,
          safetyCheckError: "更新清单缺少 build_git_sha；系统不会安装或重放该更新。",
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
    try {
      let safety: UpdateSafetyStatus | null = null;
      let safetyCheckError: string | null = null;
      try {
        safety = await invoke<UpdateSafetyStatus>("reserve_update_install", {
          targetVersion: update.version,
          targetBuild,
        });
      } catch (error) {
        // Unknown safety is unsafe. Keep retrying locally instead of asking the
        // user to click install again or risking an in-flight session.
        safetyCheckError = error instanceof Error ? error.message : String(error);
      }

      if (safety?.update_install_state === "applied") {
        if (get().safeRetryHandle !== null) {
          window.clearTimeout(get().safeRetryHandle!);
          set({ safeRetryHandle: null });
        }
        set({
          currentVersion: safety.target_version ?? update.version,
          phase: { kind: "up_to_date", checkedAt: Date.now() },
        });
        return;
      }

      // The renderer only requests durable recovery. It never owns an Update
      // mutation permit and therefore never downloads, installs, or relaunches
      // the app. The backend supervisor observes the exact target binding and
      // may install only with its current owner+epoch permit.
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
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      set({
        phase: {
          kind: "waiting_for_safe_restart",
          update,
          blockers: null,
          safetyCheckError: msg,
          checkedAt: Date.now(),
        },
      });
    }
  },

  dismiss: () => {
    const phase = get().phase;
    if (phase.kind === "available") {
      set({ dismissedVersion: phase.update.version });
    }
  },
}));
