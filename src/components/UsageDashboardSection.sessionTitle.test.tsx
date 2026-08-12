// SPDX-License-Identifier: Apache-2.0
import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { UsageDashboardSection, type UsageDashboard } from "./UsageDashboardSection";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  save: vi.fn(),
  handlers: {} as Record<string, () => void>,
}));

vi.mock("../lib/tauri", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("../stores/settings", () => ({
  useSettingsStore: () => ({ settings: null, save: mocks.save }),
}));
vi.mock("./TokenUsageHeatmap", () => ({
  formatUsageTokens: (value: number) => String(value),
  TokenUsageHeatmap: () => null,
  UsageHeatmapLegend: () => null,
}));

const dashboard: UsageDashboard = {
  range_days: 365,
  summary: {
    input_tokens: 0,
    output_tokens: 0,
    reasoning_tokens: 0,
    cached_tokens: 0,
    requests: 0,
    actual_cost_usd: null,
    estimated_cost_usd: null,
    cost_source: "unknown",
  },
  heatmap: [],
  breakdowns: [],
  top_sessions: [],
};

describe("UsageDashboardSection session title updates", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
    mocks.save.mockReset();
    mocks.handlers = {};
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    mocks.listen.mockImplementation(async (eventName: string, handler: () => void) => {
      mocks.handlers[eventName] = handler;
      return () => {};
    });
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_usage_dashboard") return dashboard;
      if (command === "get_usage_budget_status") return null;
      return undefined;
    });
  });

  it("reloads when the global session-title-updated event arrives", async () => {
    render(<UsageDashboardSection />);

    await waitFor(() => {
      expect(mocks.handlers["session-title-updated"]).toBeTypeOf("function");
    });
    const initialDashboardCalls = mocks.invoke.mock.calls.filter(
      ([command]) => command === "get_usage_dashboard",
    ).length;

    act(() => mocks.handlers["session-title-updated"]?.());

    await waitFor(() => {
      expect(
        mocks.invoke.mock.calls.filter(([command]) => command === "get_usage_dashboard").length,
      ).toBeGreaterThan(initialDashboardCalls);
    });
  });
});
