// SPDX-License-Identifier: Apache-2.0
// Synthetic IPC only; mounts the production App, stores and dispatch handlers.
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import App from "../App";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import type { Session, Settings } from "../lib/tauri";
import "../styles/globals.css";

const theme = new URLSearchParams(location.search).get("theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;
const session: Session = { id: "synthetic-existing", title: "合成派单验收", cwd: "/synthetic/project", model_id: "synthetic-model", kind: "project", created_at: 1, updated_at: 1, total_input_tokens: 0, total_output_tokens: 0, permission_mode: "trusted" };
const sessions = [session];
const replies: Record<string, unknown>[] = [];
const sends: Record<string, unknown>[] = [];
const settings = { endpoints: {}, default_model: "synthetic-model", default_endpoint: "synthetic", theme, onboarded: true, permissions: { allow: [], ask: [], deny: [], full_access: true }, shell: { shell: "bash" }, auto_create_pr: false, font_size: 14, font_family: "inter", mono_font_family: "jetbrains-mono" } as Settings;
mockWindows("main");
mockIPC((command, payload) => {
  const args = payload as Record<string, unknown>;
  switch (command) {
    case "get_settings": return settings;
    case "list_sessions": return sessions;
    case "get_session": return sessions.find((item) => item.id === args.sessionId);
    case "get_message_page": return { messages: [], plans: [], next_before_rowid: null, has_more: false, truncated: false };
    case "create_session": {
      const created = { ...session, id: `synthetic-created-${sessions.length}`, cwd: String(args.cwd), model_id: String(args.modelId ?? args.model ?? "synthetic-model") };
      sessions.push(created); return created;
    }
    case "send_message": sends.push(args); return null;
    case "dispatch_reply": replies.push(args); return true;
    case "subscription_quota_status": return [{ endpoint: "synthetic", label: "合成订阅", source: "本地估算", five_hour_percent: 12, weekly_percent: 9, five_hour_cap_percent: 80, weekly_cap_percent: 80, over_cap: false }];
    case "is_chat_running": return false;
    case "list_models": case "list_models_for_endpoint": return [];
    case "list_tasks": case "list_checkpoints": return [];
    default: return null;
  }
}, { shouldMockEvents: true });
useSettingsStore.setState({ settings, load: async () => { useSettingsStore.setState({ settings }); } });
useChatStore.setState({ sessions, activeSession: session, draftSession: null, runtime: {}, activeModel: "synthetic-model" });
Object.assign(window, { __dispatchProbe: { replies, sends, dispatch: (request_id: string, operation: string, request: Record<string, unknown>) => emit("dispatch:request", { request_id, operation, request }) } });
createRoot(document.getElementById("root")!).render(<App />);
