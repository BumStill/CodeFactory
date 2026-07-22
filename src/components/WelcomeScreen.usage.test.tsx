// SPDX-License-Identifier: Apache-2.0

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WelcomeScreen } from "./WelcomeScreen";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), selectSession: vi.fn() }));
const chatState = vi.hoisted(() => ({
  sessions: [],
  activeSession: {
    id: "new-project-session",
    title: "Untitled",
    cwd: "/tmp/codefactory",
    model_id: "gpt-5.6-sol",
    created_at: 0,
    updated_at: 0,
    total_input_tokens: 0,
    total_output_tokens: 0,
    kind: "project" as "project" | "anonymous",
  },
  activeModel: "gpt-5.6-sol",
  selectSession: mocks.selectSession,
}));

vi.mock("../stores/chat", () => ({ useChatStore: () => chatState }));
vi.mock("../lib/tauri", () => ({ invoke: mocks.invoke }));

function welcomeDashboard() {
  return {
    range_days: 28,
    summary: {
      input_tokens: 100_000,
      output_tokens: 20_000,
      reasoning_tokens: 4_000,
      cached_tokens: 60_000,
      requests: 12,
      actual_cost_usd: null,
      estimated_cost_usd: 0.16,
      cost_source: "subscription",
    },
    heatmap: Array.from({ length: 28 }, (_, index) => {
      const date = new Date(Date.UTC(2026, 6, 22));
      date.setUTCDate(date.getUTCDate() - (27 - index));
      const localDate = date.toISOString().slice(0, 10);
      return {
        local_date: localDate,
        status: localDate === "2026-07-19" ? "missing" : "recorded",
        total_tokens: localDate === "2026-07-19"
          ? null
          : localDate === "2026-07-20"
            ? 0
            : localDate === "2026-07-22"
              ? 27_000
              : 1_000,
        requests: localDate === "2026-07-19"
          ? null
          : localDate === "2026-07-20"
            ? 0
            : localDate === "2026-07-22"
              ? 4
              : 1,
      };
    }),
    breakdowns: [],
    top_sessions: [],
  };
}

describe("new-session token usage summary", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.selectSession.mockReset();
    chatState.activeSession.kind = "project";
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_usage_dashboard") return welcomeDashboard();
      return undefined;
    });
  });

  it("shows today's usage beside an accessible 28-day trend and exposes the full dashboard", async () => {
    const onOpenUsage = vi.fn();
    const user = userEvent.setup();
    render(<WelcomeScreen onOpenUsage={onOpenUsage} />);

    const region = await screen.findByRole("region", { name: "今日用量与过去 4 周趋势" });
    const trend = within(region).getByRole("grid", { name: "过去 4 周 Token 趋势" });
    await waitFor(() => expect(within(trend).getAllByRole("gridcell")).toHaveLength(28));
    expect(trend).toHaveAttribute("aria-rowcount", "1");
    expect(trend).toHaveAttribute("aria-colcount", "28");
    expect(within(region).getByText("今日用量")).toBeInTheDocument();
    expect(within(region).getByText("27K")).toBeInTheDocument();
    expect(within(region).getByText("4 次请求")).toBeInTheDocument();
    expect(region).toHaveTextContent("过去 4 周");
    expect(region).toHaveTextContent("订阅流量");
    expect(region.textContent).not.toMatch(/\$\s*0\.16/);
    expect(mocks.invoke).toHaveBeenCalledWith("get_usage_dashboard", {
      rangeDays: 28,
      timezoneOffsetMinutes: expect.any(Number),
    });

    await user.click(within(region).getByRole("button", { name: "查看用量详情" }));
    expect(onOpenUsage).toHaveBeenCalledTimes(1);
  });

  it("makes missing and zero days understandable without tiny status glyphs", async () => {
    render(<WelcomeScreen />);
    const trend = await screen.findByRole("grid", { name: "过去 4 周 Token 趋势" });
    const missing = within(trend).getByRole("gridcell", { name: /2026-07-19.*数据缺失/ });
    const zero = within(trend).getByRole("gridcell", { name: /2026-07-20.*0 Tokens.*已记录/ });
    expect(missing).not.toHaveClass("border-dashed");
    expect(missing).toHaveClass("bg-accent/20");
    expect(zero).toHaveClass("bg-accent/20");
    expect(zero).not.toHaveClass("outline");
    expect(trend).not.toHaveTextContent(/[×·!]/);
  });

  it("keeps an anonymous session transient without querying or displaying persisted history", async () => {
    chatState.activeSession.kind = "anonymous";
    render(<WelcomeScreen />);

    const region = await screen.findByRole("region", { name: "今日用量与过去 4 周趋势" });
    expect(within(region).getByText(/匿名会话.*不计入今日统计/)).toBeInTheDocument();
    expect(mocks.invoke).not.toHaveBeenCalledWith("get_usage_dashboard", expect.anything());
    expect(within(region).queryByRole("grid", { name: "过去 4 周 Token 趋势" })).not.toBeInTheDocument();
  });
});
