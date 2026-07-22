// SPDX-License-Identifier: Apache-2.0
// Lock-safe browser acceptance entry. This HTML is not part of the production
// bundle; it mounts the real usage components against bounded Tauri mock IPC.

import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import "../styles/globals.css";
import type { Settings } from "../lib/tauri";
import type { UsageDashboard, UsageSummary } from "../components/UsageDashboardSection";
import type { UsageHeatmapDay } from "../components/TokenUsageHeatmap";

const today = "2026-07-22";
const selectedDate = "2026-07-21";

function dateDays(rangeDays: number): UsageHeatmapDay[] {
  const end = new Date(`${today}T00:00:00Z`);
  return Array.from({ length: rangeDays }, (_, index) => {
    const date = new Date(end);
    date.setUTCDate(end.getUTCDate() - (rangeDays - index - 1));
    const localDate = date.toISOString().slice(0, 10);
    if (localDate === "2026-07-19") {
      return { local_date: localDate, status: "missing", total_tokens: null, requests: null };
    }
    if (localDate === "2026-07-20") {
      return { local_date: localDate, status: "partial", total_tokens: 24_000, requests: 3 };
    }
    const total = localDate === today ? 80_000 : localDate === selectedDate ? 46_000 : index % 9 === 0 ? 0 : 1_200 + ((index * 7_919) % 31_000);
    return {
      local_date: localDate,
      status: "recorded",
      total_tokens: total,
      input_tokens: Math.round(total * 0.72),
      output_tokens: Math.round(total * 0.28),
      reasoning_tokens: Math.round(total * 0.08),
      cached_tokens: Math.round(total * 0.15),
      requests: total === 0 ? 0 : 1 + (index % 7),
    };
  });
}

const summary: UsageSummary = {
  input_tokens: 1_420_000,
  output_tokens: 380_000,
  reasoning_tokens: 145_000,
  cached_tokens: 512_000,
  requests: 186,
  actual_cost_usd: null,
  estimated_cost_usd: null,
  cost_source: "subscription",
};

function dashboard(rangeDays: number): UsageDashboard {
  return {
    range_days: rangeDays,
    summary,
    heatmap: dateDays(rangeDays),
    breakdowns: [
      { surface: "interactive", total_tokens: 1_220_000, requests: 132 },
      { surface: "autonomous", total_tokens: 580_000, requests: 54 },
    ],
    top_sessions: [],
  };
}

const settings: Settings = {
  endpoints: {},
  default_endpoint: "chatgpt",
  default_model: "gpt-5.4",
  permissions: { allow: ["*"], ask: [], deny: [], full_access: true },
  shell: { shell: "zsh" },
  auto_create_pr: false,
  theme: "dark",
  font_family: "inter",
  font_size: 14,
  usage_budget: {
    daily_token_limit: 100_000,
    monthly_token_limit: 2_500_000,
    alert_thresholds: [0.5, 0.8, 1],
    alerts_enabled: true,
  },
};

mockWindows("main");
mockIPC((command, args) => {
  const payload = (args ?? {}) as Record<string, unknown>;
  switch (command) {
    case "get_usage_dashboard":
      return dashboard(Number(payload.rangeDays ?? 365));
    case "get_usage_day_detail":
      return {
        local_date: String(payload.localDate),
        summary: { ...summary, input_tokens: 35_000, output_tokens: 11_000, requests: 6 },
        breakdowns: [
          { surface: "interactive", total_tokens: 30_000, requests: 4 },
          { surface: "autonomous", total_tokens: 16_000, requests: 2 },
        ],
        top_sessions: [
          { session_id: "usage-session-1", job_session_id: "project-session", title: "修复图片识别与上传链路", surface: "subagent", task_id: "task-1", total_tokens: 30_000, requests: 4, share: 0.652 },
          { session_id: "usage-session-2", job_session_id: "project-session", title: "Evals 自动激活回归", surface: "autonomous", task_id: "task-2", total_tokens: 16_000, requests: 2, share: 0.348 },
        ],
      };
    case "save_settings":
      return payload.newSettings;
    default:
      return null;
  }
}, { shouldMockEvents: true });

const { useSettingsStore } = await import("../stores/settings");
useSettingsStore.setState({ settings });
const { UsageDashboardSection } = await import("../components/UsageDashboardSection");
const { WelcomeUsageCard } = await import("../components/WelcomeUsageCard");

function AcceptancePage() {
  const [surface, setSurface] = useState<"welcome" | "settings">("welcome");
  const [openedSession, setOpenedSession] = useState<string | null>(null);
  const [openedJobLog, setOpenedJobLog] = useState<string | null>(null);
  return (
    <main className="min-h-screen bg-surface-0 p-4 text-gray-200">
      <nav aria-label="验收界面" className="mx-auto mb-4 flex max-w-6xl gap-2">
        <button type="button" onClick={() => setSurface("welcome")} className="rounded border border-border px-3 py-1.5 text-xs">新会话</button>
        <button type="button" onClick={() => setSurface("settings")} className="rounded border border-border px-3 py-1.5 text-xs">设置 / 用量与预算</button>
      </nav>
      {surface === "welcome" ? (
        <div className="mx-auto max-w-3xl space-y-3">
          <h1 className="text-lg font-semibold">开始新会话</h1>
          <WelcomeUsageCard anonymous={false} onOpenUsage={() => setSurface("settings")} />
        </div>
      ) : (
        <UsageDashboardSection
          onOpenSession={setOpenedSession}
          onOpenJobLog={(sessionId, taskId) => setOpenedJobLog(`${sessionId}/${taskId}`)}
        />
      )}
      {openedSession && <p role="status" className="mx-auto mt-4 max-w-6xl text-xs text-accent">已打开会话：{openedSession}</p>}
      {openedJobLog && <p role="status" className="mx-auto mt-4 max-w-6xl text-xs text-accent">已打开作业日志：{openedJobLog}</p>}
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode><AcceptancePage /></React.StrictMode>,
);
