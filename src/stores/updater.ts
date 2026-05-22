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

const POLL_INTERVAL_MS = 30 * 60 * 1000; // 30 min — light on the GitHub CDN, fresh enough for users

export type UpdaterPhase =
  | { kind: "idle" }                                                // first ever, no check yet
  | { kind: "checking" }
  | { kind: "up_to_date"; checkedAt: number }
  | { kind: "available"; update: Update; checkedAt: number }
  | { kind: "downloading"; received: number; total: number | null }
  | { kind: "installing" }
  | { kind: "ready" }
  | { kind: "error"; message: string; checkedAt: number };

interface UpdaterStore {
  phase: UpdaterPhase;
  currentVersion: string | null;
  pollHandle: number | null;
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
    if (phase.kind === "downloading" || phase.kind === "installing" || phase.kind === "ready") {
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
    if (phase.kind !== "available") return;
    const update = phase.update;
    try {
      set({ phase: { kind: "downloading", received: 0, total: null } });
      await update.downloadAndInstall((event) => {
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
            set({ phase: { kind: "installing" } });
            break;
        }
      });
      set({ phase: { kind: "ready" } });
      setTimeout(() => relaunch().catch(console.error), 800);
    } catch (err) {
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
