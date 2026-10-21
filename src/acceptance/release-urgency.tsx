// SPDX-License-Identifier: Apache-2.0
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { ReleaseUrgencyPicker } from "../components/ReleaseUrgencyPicker";
import { useChatStore } from "../stores/chat";
import type { Session } from "../lib/tauri";
import "../styles/globals.css";
document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") ?? "dark";
let urgency = "immediate";
mockIPC((command, payload) => {
  if (command === "get_session_release_urgency") return urgency;
  if (command === "update_session_release_urgency") { urgency = String((payload as { urgency: string }).urgency); return null; }
  throw new Error(`Unexpected IPC: ${command}`);
});
useChatStore.setState({ activeSession: { id: "synthetic-release-session", kind: "project" } as Session });
createRoot(document.getElementById("root")!).render(<main className="min-h-screen bg-surface-1 p-4 text-gray-200"><section aria-label="会话设置" className="w-full max-w-sm rounded-lg border border-border bg-surface-2"><h1 className="px-2 py-2 text-label">会话设置</h1><ReleaseUrgencyPicker /></section></main>);
