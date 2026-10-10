// SPDX-License-Identifier: Apache-2.0
// Lock-safe browser acceptance entry for CF-QUOTA-R4. This HTML is not part of
// the production bundle; it mounts the real SubscriptionQuotaSection against
// bounded Tauri mock IPC so the settings surface can be inspected in a real
// browser in both light and dark themes.

import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import "../styles/globals.css";
import type { Settings, SubscriptionQuotaStatus } from "../lib/tauri";
import { useSettingsStore } from "../stores/settings";
import { SubscriptionQuotaSection } from "../components/SubscriptionQuotaSection";

const theme = new URLSearchParams(window.location.search).get("theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;

const now = Date.now();

// Synthetic fixture only — no real account, no real usage, no credentials.
const statuses: SubscriptionQuotaStatus[] = [
  {
    endpoint: "chatgpt",
    label: "ChatGPT",
    source: "服务端读数",
    five_hour_percent: 82,
    weekly_percent: 34,
    five_hour_cap_percent: 80,
    weekly_cap_percent: 80,
    five_hour_resets_at_ms: now + 2 * 60 * 60 * 1000,
    weekly_resets_at_ms: now + 3 * 24 * 60 * 60 * 1000,
    over_cap: true,
  },
  {
    endpoint: "chatgpt-work",
    label: "ChatGPT Work",
    source: "本地估算",
    five_hour_percent: 12,
    weekly_percent: 9,
    five_hour_cap_percent: 80,
    weekly_cap_percent: 80,
    five_hour_resets_at_ms: now + 60 * 60 * 1000,
    weekly_resets_at_ms: null,
    over_cap: false,
  },
];

const settings: Settings = {
  endpoints: {
    chatgpt: {
      base_url: "https://chatgpt.com/backend-api/codex",
      api_style: "chatgpt",
      custom_models: [],
      active_model: "gpt-5.5",
    } as Settings["endpoints"][string],
    "chatgpt-work": {
      base_url: "https://chatgpt.example/work",
      api_style: "chatgpt",
      custom_models: [],
      active_model: "gpt-5.5",
    } as Settings["endpoints"][string],
  },
  default_endpoint: "chatgpt",
  default_model: "gpt-5.5",
  permissions: { allow: [], ask: [], deny: [], full_access: false },
  shell: { shell: "zsh" },
  auto_create_pr: false,
  theme: theme === "light" ? "light" : "dark",
  font_family: "inter",
  mono_font_family: "jetbrains-mono",
  font_size: 14,
  subscription_quota_caps: {},
};

mockWindows("acceptance");
mockIPC((command) => {
  if (command === "get_settings") return settings;
  if (command === "save_settings") return settings;
  if (command === "subscription_quota_status") return statuses;
  return null;
});

useSettingsStore.setState({ settings });

function AcceptancePage() {
  return (
    <main aria-label="订阅用量上限验收" className="min-h-screen bg-surface-0 p-4">
      <div className="mx-auto max-w-3xl">
        <SubscriptionQuotaSection />
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptancePage />
  </React.StrictMode>,
);
