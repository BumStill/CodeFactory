// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the M239 stalled update exit.
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Update } from "@tauri-apps/plugin-updater";
import "../styles/globals.css";
import { UpdaterBanner } from "../components/UpdaterBanner";
import { useUpdaterStore } from "../stores/updater";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.theme = params.get("theme") === "light" ? "light" : "dark";
const idle = { update_install_state: "queued", active_chat_turns: 0, active_task_schedulers: 0, active_delivery_leases: 0, active_objective_leases: 0, objective_blocker_owners: [], pending_permissions: 0, managed_browser_sessions: 0, terminal_sessions: 0 };
const calls: string[] = [];
(window as unknown as { updateCalls: string[] }).updateCalls = calls;
mockIPC((command) => {
  calls.push(command);
  return command === "reserve_update_install" ? idle : null;
});
const failed = params.get("case") === "error";
const now = Date.now();
useUpdaterStore.setState({
  updateMinimized: true,
  waitingSince: now - 61_000,
  phase: {
    kind: "waiting_for_safe_restart",
    update: { version: "1.92.0", rawJson: { build_git_sha: "b".repeat(40) } } as unknown as Update,
    blockers: failed ? null : (idle as never),
    safetyCheckError: failed ? "update target cannot replace the system-owned target" : null,
    checkedAt: now,
  },
});
createRoot(document.getElementById("root")!).render(<main className="min-h-screen bg-surface-0 p-4"><UpdaterBanner /></main>);
