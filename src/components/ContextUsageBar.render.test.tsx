// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ContextUsageBar } from "./ContextUsageBar";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

const chatState = vi.hoisted(() => ({
  runtime: {
    s1: {
      contextUsage: { used: 58_000, limit: 100_000, maxLimit: 100_000 } as {
        used: number;
        limit: number;
        maxLimit: number;
      } | null,
      compressionToast: null,
    },
    s2: {
      contextUsage: { used: 25_000, limit: 100_000, maxLimit: 100_000 } as {
        used: number;
        limit: number;
        maxLimit: number;
      } | null,
      compressionToast: null,
    },
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

vi.mock("../lib/tauri", () => ({
  invoke: mocks.invoke,
}));

vi.mock("../stores/chat", () => ({
  useChatStore: (selector: (state: typeof chatState) => unknown) => selector(chatState),
}));

const summary = (inputTokens: number, outputTokens: number) => ({
  input_tokens: inputTokens,
  output_tokens: outputTokens,
  reasoning_tokens: 0,
  cached_tokens: 0,
  requests: 1,
  actual_cost_usd: null,
  estimated_cost_usd: null,
  cost_source: "subscription",
});

describe("ContextUsageBar compact ring", () => {
  beforeEach(() => {
    chatState.runtime.s1.contextUsage = { used: 58_000, limit: 100_000, maxLimit: 100_000 };
    chatState.runtime.s2.contextUsage = { used: 25_000, limit: 100_000, maxLimit: 100_000 };
    mocks.listen.mockReset().mockResolvedValue(() => {});
    mocks.invoke.mockReset().mockImplementation((command: string) => {
      if (command === "get_session_usage") return Promise.resolve(summary(8_000, 2_000));
      if (command === "get_usage_dashboard") {
        return Promise.resolve({ summary: summary(32_000, 8_000) });
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
  });

  const waitForUsageRefresh = async (sessionId: string) => {
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_session_usage", { sessionId }));
    await act(async () => { await Promise.resolve(); });
  };

  it("shows only the neutral ring below 75%, then reveals cumulative usage from the keyboard", async () => {
    const user = userEvent.setup();
    render(<ContextUsageBar sessionId="s1" />);

    await waitForUsageRefresh("s1");
    const trigger = screen.getByRole("button", { name: /上下文.*详情/ });
    expect(trigger).toHaveClass("min-h-[44px]", "min-w-[44px]", "lg:min-h-[36px]", "lg:min-w-[36px]");
    const meter = within(trigger).getByRole("meter", { name: "上下文占用" });
    expect(meter).toHaveAttribute("aria-valuenow", "58");
    expect(meter).toHaveAttribute("aria-valuetext", expect.stringMatching(/58%/));
    expect(screen.queryByText("58%")).not.toBeInTheDocument();
    expect(screen.queryByText(/会话累计/)).not.toBeInTheDocument();
    expect(screen.queryByText(/今日累计/)).not.toBeInTheDocument();

    trigger.focus();
    await user.keyboard("{Enter}");
    const detail = await screen.findByRole("dialog", { name: /上下文.*用量详情/ });
    expect(within(detail).getByText(/会话累计/)).toBeInTheDocument();
    expect(within(detail).getByText(/今日累计/)).toBeInTheDocument();
    expect(within(detail).getByText(/剩余预算/)).toBeInTheDocument();
    expect(within(detail).getByText("42K")).toBeInTheDocument();
    expect(within(detail).getByText("10K")).toBeInTheDocument();
    expect(within(detail).getByText("40K")).toBeInTheDocument();
    expect(detail.parentElement).toBe(document.body);

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: /上下文.*用量详情/ })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("adds a percentage only from 75% through 89%", async () => {
    chatState.runtime.s1.contextUsage = { used: 76_000, limit: 100_000, maxLimit: 100_000 };
    render(<ContextUsageBar sessionId="s1" />);
    await waitForUsageRefresh("s1");

    const trigger = screen.getByRole("button", { name: /上下文.*详情/ });
    expect(within(trigger).getByText("76%")).toBeInTheDocument();
    expect(within(trigger).getByRole("meter", { name: "上下文占用" })).toHaveAttribute("aria-valuenow", "76");
  });

  it("uses explicit non-color copy at 90% and above", async () => {
    chatState.runtime.s1.contextUsage = { used: 91_000, limit: 100_000, maxLimit: 100_000 };
    render(<ContextUsageBar sessionId="s1" />);
    await waitForUsageRefresh("s1");

    const trigger = screen.getByRole("button", { name: /上下文.*详情/ });
    expect(within(trigger).getByText("接近上限")).toBeInTheDocument();
    expect(within(trigger).getByRole("meter", { name: "上下文占用" })).toHaveAttribute("aria-valuenow", "91");
  });

  it("keeps an unknown context visibly unknown without inventing zero percent", async () => {
    chatState.runtime.s1.contextUsage = null;
    render(<ContextUsageBar sessionId="s1" />);

    await waitForUsageRefresh("s1");
    const trigger = screen.getByRole("button", { name: /上下文.*详情/ });
    expect(within(trigger).queryByRole("meter")).not.toBeInTheDocument();
    expect(within(trigger).getByRole("img", { name: /上下文占用未知/ })).toBeInTheDocument();
    expect(trigger).not.toHaveTextContent(/0%/);
  });

  it("keeps the portal above the composer and returns focus after an outside click", async () => {
    const user = userEvent.setup();
    const rectSpy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.getAttribute("data-testid") === "context-usage-ring") {
        return {
          x: 600, y: 600, top: 600, right: 820, bottom: 644, left: 600,
          width: 220, height: 44, toJSON: () => ({}),
        };
      }
      return {
        x: 0, y: 0, top: 0, right: 352, bottom: 280, left: 0,
        width: 352, height: 280, toJSON: () => ({}),
      };
    });

    try {
      render(
        <>
          <button type="button">外部操作</button>
          <ContextUsageBar sessionId="s1" />
        </>,
      );
      await waitForUsageRefresh("s1");
      const trigger = screen.getByRole("button", { name: /上下文.*详情/ });
      await user.click(trigger);
      const detail = screen.getByRole("dialog", { name: /上下文.*用量详情/ });
      expect(Number.parseFloat(detail.style.top)).toBeGreaterThanOrEqual(8);
      expect(Number.parseFloat(detail.style.top)).toBeLessThan(600);
      expect(Number.parseFloat(detail.style.left)).toBeGreaterThanOrEqual(8);

      await user.click(screen.getByRole("button", { name: "外部操作" }));
      expect(screen.queryByRole("dialog", { name: /上下文.*用量详情/ })).not.toBeInTheDocument();
      expect(trigger).toHaveFocus();
    } finally {
      rectSpy.mockRestore();
    }
  });

  it("ignores a previous session response that arrives after a fast session switch", async () => {
    const user = userEvent.setup();
    let resolveFirstSession!: (value: ReturnType<typeof summary>) => void;
    const firstSession = new Promise<ReturnType<typeof summary>>((resolve) => {
      resolveFirstSession = resolve;
    });
    mocks.invoke.mockImplementation((command: string, args?: { sessionId?: string }) => {
      if (command === "get_usage_dashboard") {
        return Promise.resolve({ summary: summary(32_000, 8_000) });
      }
      if (command === "get_session_usage" && args?.sessionId === "s1") return firstSession;
      if (command === "get_session_usage" && args?.sessionId === "s2") {
        return Promise.resolve(summary(20_000, 5_000));
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { rerender } = render(<ContextUsageBar sessionId="s1" />);
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_session_usage", { sessionId: "s1" }));
    await user.click(screen.getByRole("button", { name: /上下文.*详情/ }));
    expect(screen.getByRole("dialog", { name: /上下文.*用量详情/ })).toBeInTheDocument();

    rerender(<ContextUsageBar sessionId="s2" />);
    expect(screen.queryByRole("dialog", { name: /上下文.*用量详情/ })).not.toBeInTheDocument();
    await waitForUsageRefresh("s2");
    await user.click(screen.getByRole("button", { name: /上下文.*详情/ }));
    expect(await screen.findByText("25K")).toBeInTheDocument();

    await act(async () => { resolveFirstSession(summary(90_000, 9_000)); });
    expect(screen.queryByText("99K")).not.toBeInTheDocument();
    expect(screen.getByText("25K")).toBeInTheDocument();
  });
});
