// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for startup latest-session selection.

import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import App from "../App";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import type { MessagePage, Session, Settings } from "../lib/tauri";

type Scenario = "with-history" | "empty";

const scenario: Scenario = new URLSearchParams(location.search).get("scenario") === "empty" ? "empty" : "with-history";

const latestSession: Session = {
  id: "latest-session",
  title: "现在查看未完成项，准备继续开发",
  cwd: "/tmp/CodeFactory",
  endpoint_id: "openrouter",
  model_id: "test-model",
  model_policy: "prefer",
  permission_mode: "standard",
  created_at: 2,
  updated_at: 2,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};
const olderSession: Session = {
  ...latestSession,
  id: "older-session",
  title: "你好",
  created_at: 1,
  updated_at: 1,
};

const settings: Settings = {
  endpoints: {
    openrouter: {
      base_url: "https://openrouter.ai/api/v1",
      api_style: "openai",
      custom_models: [{ id: "test-model", name: "Test Model" }],
      active_model: "test-model",
    },
  },
  default_endpoint: "openrouter",
  default_model: "test-model",
  default_model_policy: "prefer",
  permissions: { allow: [], ask: [], deny: [], full_access: false },
  shell: { shell: "bash" },
  auto_create_pr: false,
  theme: "dark",
  font_family: "inter",
  font_size: 14,
  reasoning_effort: "medium",
  onboarded: true,
};

const emptyPage: MessagePage = {
  messages: [],
  plans: [],
  next_before_rowid: null,
  has_more: false,
  truncated: false,
};

function installTauriMock() {
  const sessions = scenario === "with-history" ? [latestSession, olderSession] : [];
  (window as typeof window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: () => {},
  };
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
    invoke: async (_cmd: string, args?: { message?: { cmd?: string; payload?: Record<string, unknown> } }) => {
      const cmd = args?.message?.cmd ?? _cmd;
      const payload = args?.message?.payload ?? {};
      switch (cmd) {
        case "plugin:event|listen": return 1;
        case "plugin:event|unlisten": return null;
        case "get_settings": return settings;
        case "list_sessions": return sessions;
        case "get_session": return sessions.find((session) => session.id === payload.sessionId) ?? latestSession;
        case "get_message_page": return emptyPage;
        case "is_chat_running": return false;
        case "list_models": return [{ id: "test-model", name: "Test Model" }];
        case "list_models_for_endpoint": return [{ id: "test-model", name: "Test Model" }];
        case "get_turn_timing_profile": return null;
        case "get_git_status": return null;
        case "list_tasks": return [];
        case "subscribe_tasks": return null;
        default: return null;
      }
    },
    transformCallback: () => 1,
  };
}

function seedStores() {
  useSettingsStore.setState({
    settings: null,
    load: async () => { useSettingsStore.setState({ settings }); },
    save: async (next) => { useSettingsStore.setState({ settings: next }); },
  });
  useChatStore.setState({
    sessions: [],
    activeSession: null,
    draftSession: null,
    runtime: {},
    activeModel: "test-model",
  });
}

installTauriMock();
seedStores();

function Probe() {
  const open = useChatStore((state) => state.draftSession?.id ?? state.activeSession?.id ?? "none");
  const activeTitle = useChatStore((state) => state.activeSession?.title ?? "");
  const draft = useChatStore((state) => state.draftSession != null);
  return (
    <>
      <App />
      <div aria-label="Startup session probe" data-open-session={open} data-active-title={activeTitle} data-draft={String(draft)} />
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Probe />);
