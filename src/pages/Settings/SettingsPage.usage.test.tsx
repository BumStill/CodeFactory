// SPDX-License-Identifier: Apache-2.0
//
// Token-usage dashboard acceptance contract. These tests intentionally land
// before the production UI: they pin the truthful, drillable, accessible
// behaviour instead of the implementation's component boundaries.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SettingsPage } from "./SettingsPage";

// This file renders the full Settings page and repeatedly materializes a
// 365-cell accessible heatmap. Keep the acceptance assertions bounded while
// allowing loaded CI workers more than Vitest's 5-second unit-test default.
vi.setConfig({ testTimeout: 15_000 });

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  saveApiKey: vi.fn(),
  loadModels: vi.fn(),
  invoke: vi.fn(),
  codexLogin: vi.fn(),
  codexLogout: vi.fn(),
  codexAccount: vi.fn(),
  codexModels: vi.fn(),
  applyCodexModels: vi.fn(),
  loadRemotes: vi.fn(),
  addRemote: vi.fn(),
  deleteRemote: vi.fn(),
  testRemote: vi.fn(),
  updaterInitialize: vi.fn(),
  updaterCheckNow: vi.fn(),
  updaterInstall: vi.fn(),
  openSession: vi.fn(),
  openJobLog: vi.fn(),
}));

const settingsState = vi.hoisted(() => ({
  settings: {
    endpoints: {
      chatgpt: {
        base_url: "https://chatgpt.com/backend-api/codex",
        api_style: "chatgpt" as const,
        custom_models: [],
        active_model: "gpt-5.6-sol",
      },
    },
    default_endpoint: "chatgpt",
    default_model: "gpt-5.6-sol",
    permissions: { allow: [], ask: [], deny: [], full_access: true },
    shell: { shell: "zsh" },
    auto_create_pr: false,
    theme: "dark" as const,
    font_family: "inter",
    font_size: 14,
    reasoning_effort: "medium" as const,
    onboarded: true,
  },
  load: mocks.load,
  save: mocks.save,
  saveApiKey: mocks.saveApiKey,
}));

const chatState = vi.hoisted(() => ({ loadModels: mocks.loadModels }));
const gitRemoteState = vi.hoisted(() => ({
  remotes: [],
  loadRemotes: mocks.loadRemotes,
  addRemote: mocks.addRemote,
  deleteRemote: mocks.deleteRemote,
  testRemote: mocks.testRemote,
}));
const updaterState = vi.hoisted(() => ({
  phase: { kind: "idle" as const },
  currentVersion: "dev",
  initialize: mocks.updaterInitialize,
  checkNow: mocks.updaterCheckNow,
  install: mocks.updaterInstall,
}));

vi.mock("../../stores/settings", () => {
  function useSettingsStore<T>(selector?: (state: typeof settingsState) => T) {
    return selector ? selector(settingsState) : settingsState;
  }
  useSettingsStore.getState = () => settingsState;
  return { useSettingsStore };
});

vi.mock("../../stores/chat", () => {
  function useChatStore() { return chatState; }
  useChatStore.getState = () => chatState;
  return { useChatStore };
});

vi.mock("../../stores/gitRemote", () => ({ useGitRemoteStore: () => gitRemoteState }));
vi.mock("../../stores/updater", () => ({
  useUpdaterStore: <T,>(selector: (state: typeof updaterState) => T) => selector(updaterState),
}));
vi.mock("../../lib/tauri", () => ({
  invoke: mocks.invoke,
  codexLogin: mocks.codexLogin,
  codexLogout: mocks.codexLogout,
  codexAccount: mocks.codexAccount,
  codexModels: mocks.codexModels,
  applyCodexModels: mocks.applyCodexModels,
}));

function localDateDaysAgo(daysAgo: number): string {
  const date = new Date(Date.UTC(2026, 6, 22));
  date.setUTCDate(date.getUTCDate() - daysAgo);
  return date.toISOString().slice(0, 10);
}

function dashboard(rangeDays: number) {
  return {
    range_days: rangeDays,
    summary: {
      input_tokens: 23_870_930,
      output_tokens: 204_456,
      reasoning_tokens: 80_000,
      cached_tokens: 12_000_000,
      requests: 195,
      actual_cost_usd: null,
      estimated_cost_usd: 24.48,
      cost_source: "subscription",
    },
    heatmap: Array.from({ length: rangeDays }, (_, index) => {
      const localDate = localDateDaysAgo(rangeDays - index - 1);
      if (localDate === "2026-07-21") {
        return { local_date: localDate, status: "missing", total_tokens: null, requests: null };
      }
      if (localDate === "2026-07-20") {
        return { local_date: localDate, status: "recorded", total_tokens: 0, requests: 0 };
      }
      if (localDate === "2026-07-19") {
        return { local_date: localDate, status: "partial", total_tokens: 48_000, requests: 3 };
      }
      if (localDate === "2026-07-22") {
        return { local_date: localDate, status: "recorded", total_tokens: 24_075_386, requests: 195 };
      }
      return { local_date: localDate, status: "recorded", total_tokens: 12_000, requests: 2 };
    }),
    breakdowns: [
      { surface: "interactive", total_tokens: 1_000_000, requests: 20 },
      { surface: "autonomous", total_tokens: 23_075_386, requests: 175 },
    ],
    top_sessions: [
      {
        session_id: "session-loop",
        job_session_id: "project-session",
        task_id: "task-loop",
        surface: "subagent",
        title: "修复重复循环回复",
        total_tokens: 19_000_000,
        requests: 150,
        share: 0.79,
      },
    ],
  };
}

const dayDetail = {
  local_date: "2026-07-22",
  summary: dashboard(1).summary,
  breakdowns: dashboard(1).breakdowns,
  top_sessions: dashboard(1).top_sessions,
};

function detailFor(localDate: string, title = `会话 ${localDate}`) {
  const empty = localDate === "2026-07-21";
  return {
    local_date: localDate,
    summary: empty
      ? { ...dashboard(1).summary, input_tokens: 0, output_tokens: 0, requests: 0 }
      : dashboard(1).summary,
    breakdowns: empty ? [] : dashboard(1).breakdowns,
    top_sessions: empty ? [] : [{
      session_id: `session-${localDate}`,
      title,
      total_tokens: 12_000,
      requests: 2,
      share: 1,
    }],
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => { resolve = next; });
  return { promise, resolve };
}

async function openUsageTab() {
  const user = userEvent.setup();
  render(
    <SettingsPage
      onBack={() => {}}
      onOpenSession={mocks.openSession}
      onOpenJobLog={mocks.openJobLog}
    />,
  );
  await user.click(screen.getByRole("tab", { name: "用量与预算" }));
  return user;
}

describe("Settings token usage dashboard", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
    mocks.load.mockResolvedValue(undefined);
    mocks.save.mockResolvedValue(undefined);
    mocks.codexAccount.mockResolvedValue({ email: "subscriber@example.com", plan_type: "plus" });
    mocks.invoke.mockImplementation(async (command: string, args?: { rangeDays?: number; localDate?: string }) => {
      if (command === "get_usage_dashboard") return dashboard(args?.rangeDays ?? 365);
      if (command === "get_usage_day_detail") {
        if (args?.localDate === "2026-07-22") return dayDetail;
        return detailFor(args?.localDate ?? "2026-07-22");
      }
      return undefined;
    });
  });

  it("exposes a first-class settings entry and switches the heatmap between 90, 180, and 365 days", async () => {
    const user = await openUsageTab();
    const region = await screen.findByRole("region", { name: "用量与预算" });
    const currentCells = () => within(
      within(region).getByRole("grid", { name: /Token 消耗地图/ }),
    ).getAllByRole("gridcell");

    await waitFor(() => expect(currentCells()).toHaveLength(365));
    expect(mocks.invoke).toHaveBeenCalledWith("get_usage_dashboard", {
      rangeDays: 365,
      timezoneOffsetMinutes: expect.any(Number),
    });

    await user.click(within(region).getByRole("button", { name: "近 90 天" }));
    await waitFor(() => expect(currentCells()).toHaveLength(90));

    await user.click(within(region).getByRole("button", { name: "近 180 天" }));
    await waitFor(() => expect(currentCells()).toHaveLength(180));

    await user.click(within(region).getByRole("button", { name: "近 365 天" }));
    await waitFor(() => expect(currentCells()).toHaveLength(365));
  });

  it("distinguishes missing telemetry from a recorded zero without relying on color", async () => {
    await openUsageTab();
    const region = await screen.findByRole("region", { name: "用量与预算" });
    const map = within(region).getByRole("grid", { name: /Token 消耗地图/ });

    // Both states need explicit accessible text. A grey square alone cannot
    // tell a legitimate rest day from broken collection.
    expect(within(map).getByRole("gridcell", { name: /2026-07-21.*数据缺失/ })).toBeInTheDocument();
    expect(within(map).getByRole("gridcell", { name: /2026-07-20.*0 Tokens.*已记录/ })).toBeInTheDocument();
    expect(within(region).getByText("数据缺失")).toBeInTheDocument();
    expect(within(region).getByText("0 用量")).toBeInTheDocument();
  });

  it("drills a selected day into surface usage and the end-to-end session log", async () => {
    const user = await openUsageTab();
    const map = await screen.findByRole("grid", { name: /Token 消耗地图/ });
    await user.click(within(map).getByRole("gridcell", { name: /2026-07-22.*24.*Tokens/ }));

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_usage_day_detail", {
      localDate: "2026-07-22",
      timezoneOffsetMinutes: expect.any(Number),
    }));
    const detail = await screen.findByRole("region", { name: "2026-07-22 用量明细" });
    expect(within(detail).getByText("自主任务")).toBeInTheDocument();
    expect(within(detail).getByText("修复重复循环回复")).toBeInTheDocument();
    const sessionEntry = within(detail).getByRole("button", { name: "查看会话" });
    await user.click(sessionEntry);
    expect(mocks.openSession).toHaveBeenCalledWith("session-loop");

    const logEntry = within(detail).getByRole("button", { name: "查看作业日志" });
    await user.click(logEntry);
    expect(mocks.openJobLog).toHaveBeenCalledWith("project-session", "task-loop");
  });

  it("does not advertise a task log for an ordinary interactive session", async () => {
    mocks.invoke.mockImplementation(async (command: string, args?: { rangeDays?: number; localDate?: string }) => {
      if (command === "get_usage_dashboard") return dashboard(args?.rangeDays ?? 365);
      if (command === "get_usage_day_detail") {
        return {
          ...detailFor(args?.localDate ?? "2026-07-22"),
          top_sessions: [{
            session_id: "interactive-session",
            title: "普通对话",
            surface: "interactive",
            task_id: null,
            job_session_id: null,
            total_tokens: 1_000,
            requests: 1,
            share: 1,
          }],
        };
      }
      return undefined;
    });

    const user = await openUsageTab();
    const map = await screen.findByRole("grid", { name: /Token 消耗地图/ });
    await user.click(within(map).getByRole("gridcell", { name: /2026-07-22/ }));
    const detail = await screen.findByRole("region", { name: "2026-07-22 用量明细" });
    expect(within(detail).getByRole("button", { name: "查看会话" })).toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: "查看作业日志" })).not.toBeInTheDocument();
  });

  it("preserves missing and historical-partial provenance after drilling into a day", async () => {
    const user = await openUsageTab();
    const map = await screen.findByRole("grid", { name: /Token 消耗地图/ });

    await user.click(within(map).getByRole("gridcell", { name: /2026-07-21.*数据缺失/ }));
    const missing = await screen.findByRole("region", { name: "2026-07-21 用量明细" });
    expect(within(missing).getByText(/数据缺失.*(?:不等于|不是).*0/)).toBeInTheDocument();

    await user.click(within(map).getByRole("gridcell", { name: /2026-07-19.*历史回填/ }));
    const partial = await screen.findByRole("region", { name: "2026-07-19 用量明细" });
    expect(within(partial).getByText(/历史回填.*数据可能不完整/)).toBeInTheDocument();
  });

  it("does not let an older day-detail response overwrite the user's latest selected date", async () => {
    const olderRequest = deferred<ReturnType<typeof detailFor>>();
    const latestRequest = deferred<ReturnType<typeof detailFor>>();
    mocks.invoke.mockImplementation(async (command: string, args?: { rangeDays?: number; localDate?: string }) => {
      if (command === "get_usage_dashboard") return dashboard(args?.rangeDays ?? 365);
      if (command === "get_usage_day_detail" && args?.localDate === "2026-07-18") return olderRequest.promise;
      if (command === "get_usage_day_detail" && args?.localDate === "2026-07-22") return latestRequest.promise;
      return detailFor(args?.localDate ?? "2026-07-22");
    });

    const user = await openUsageTab();
    const map = await screen.findByRole("grid", { name: /Token 消耗地图/ });
    await user.click(within(map).getByRole("gridcell", { name: /2026-07-18/ }));
    await user.click(within(map).getByRole("gridcell", { name: /2026-07-22/ }));

    await act(async () => {
      latestRequest.resolve(detailFor("2026-07-22", "最新选择的会话"));
      await latestRequest.promise;
    });
    const latest = await screen.findByRole("region", { name: "2026-07-22 用量明细" });
    expect(within(latest).getByText("最新选择的会话")).toBeInTheDocument();

    await act(async () => {
      olderRequest.resolve(detailFor("2026-07-18", "过期响应中的会话"));
      await olderRequest.promise;
    });
    expect(within(latest).queryByText("过期响应中的会话")).not.toBeInTheDocument();
    expect(within(latest).getByText("最新选择的会话")).toBeInTheDocument();
  });

  it("labels subscription traffic without presenting an estimated dollar amount as real spend", async () => {
    await openUsageTab();
    const region = await screen.findByRole("region", { name: "用量与预算" });

    expect(within(region).getByText("订阅流量")).toBeInTheDocument();
    expect(within(region).queryByText(/\$\s*24\.48/)).not.toBeInTheDocument();
    expect(within(region).queryByText(/实际费用[^\n]*\$/)).not.toBeInTheDocument();
  });

  it("uses a fluid heatmap container at a 375px viewport instead of a fixed desktop width", async () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 375 });
    window.dispatchEvent(new Event("resize"));

    await openUsageTab();
    const map = await screen.findByRole("grid", { name: /Token 消耗地图/ });
    const styleContract = `${map.className} ${map.getAttribute("style") ?? ""}`;
    expect(styleContract).not.toMatch(/(?:^|\s)(?:w|min-w|max-w)-\[[0-9]+px\]/);
  });
});
