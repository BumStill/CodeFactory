// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for per-session permission modes.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { PermissionModePicker } from "../components/PermissionModePicker";
import { SettingsPage } from "../pages/Settings/SettingsPage";
import { useChatStore, freshRuntime } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import type { PermissionMode, Session } from "../lib/tauri";

const session: Session = {
  id: "permission-mode-session",
  title: "权限模式验收会话",
  cwd: "/tmp/codefactory-permission-mode",
  endpoint_id: "openrouter",
  model_id: "test-model",
  model_policy: "prefer",
  permission_mode: "standard",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};

const settings = {
  endpoints: {
    openrouter: {
      base_url: "https://openrouter.ai/api/v1",
      api_style: "openai" as const,
      custom_models: [{ id: "test-model", name: "Test Model" }],
      active_model: "test-model",
    },
  },
  default_endpoint: "openrouter",
  default_model: "test-model",
  default_model_policy: "prefer" as const,
  permissions: { allow: [], ask: [], deny: [], full_access: false },
  shell: { shell: "bash" },
  auto_create_pr: false,
  theme: "dark" as const,
  font_family: "inter",
  font_size: 14,
  reasoning_effort: "medium" as const,
  onboarded: true,
};

function seedStore() {
  useSettingsStore.setState({
    settings,
    load: async () => {},
    save: async (next) => { useSettingsStore.setState({ settings: next }); },
    saveApiKey: async () => {},
  });
  useChatStore.setState({
    sessions: [session],
    activeSession: session,
    draftSession: null,
    runtime: { [session.id]: freshRuntime() },
    activeModel: session.model_id,
  });
}

function PermissionModeAcceptanceApp() {
  const [settingsOpen, setSettingsOpen] = React.useState(false);
  const [currentMode, setCurrentMode] = React.useState<PermissionMode>("standard");

  React.useEffect(() => {
    seedStore();
    (window as typeof window & { __codefactoryAcceptanceSettings?: unknown }).__codefactoryAcceptanceSettings = settings;
    const unsubscribe = useChatStore.subscribe((state) => {
      setCurrentMode((state.activeSession?.permission_mode ?? "standard") as PermissionMode);
    });
    return unsubscribe;
  }, []);

  return (
    <main className="min-h-screen bg-bg p-6 text-gray-200" aria-label="Permission mode acceptance">
      <h1 className="mb-4 text-heading font-semibold">Session permission mode acceptance</h1>
      <section aria-label="Workspace toolbar" className="mb-4 rounded-lg border border-border bg-surface-1 p-3">
        <h2 className="mb-2 text-body font-medium">Workspace toolbar</h2>
        <PermissionModePicker onChangeForAcceptance={(mode) => {
          const current = useChatStore.getState().activeSession;
          if (!current) return;
          const updated = { ...current, permission_mode: mode };
          useChatStore.setState((state) => ({
            activeSession: updated,
            sessions: state.sessions.map((item) => item.id === updated.id ? updated : item),
          }));
        }} />
        <div data-testid="current-permission-mode" className="mt-2 text-label text-gray-400">mode:{currentMode}</div>
      </section>
      <section aria-label="Settings tabs" className="rounded-lg border border-border bg-surface-1 p-3">
        <button
          type="button"
          className="rounded bg-accent px-3 py-1.5 text-label text-white"
          onClick={() => setSettingsOpen((open) => !open)}
        >
          {settingsOpen ? "关闭设置" : "打开设置"}
        </button>
        <div className="mt-3 h-[420px] overflow-auto rounded border border-border bg-surface-0">
          {settingsOpen && (
            <SettingsPage
              onBack={() => setSettingsOpen(false)}
              onOpenSession={() => {}}
            />
          )}
        </div>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<PermissionModeAcceptanceApp />);
