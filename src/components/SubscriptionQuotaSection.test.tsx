// SPDX-License-Identifier: Apache-2.0
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SubscriptionQuotaSection, resetClockHint } from "./SubscriptionQuotaSection";
import type { SubscriptionQuotaStatus } from "../lib/tauri";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  save: vi.fn(),
  settings: {
    endpoints: { chatgpt: {} },
    subscription_quota_caps: {},
  } as Record<string, unknown>,
}));

vi.mock("../lib/tauri", () => ({ invoke: mocks.invoke }));
vi.mock("../stores/settings", () => ({
  useSettingsStore: () => ({ settings: mocks.settings, save: mocks.save }),
}));

function status(overrides: Partial<SubscriptionQuotaStatus> = {}): SubscriptionQuotaStatus {
  return {
    endpoint: "chatgpt",
    label: "ChatGPT",
    source: "服务端读数",
    five_hour_percent: 85,
    weekly_percent: 20,
    five_hour_cap_percent: 80,
    weekly_cap_percent: 80,
    five_hour_resets_at_ms: Date.now() + 3_600_000,
    weekly_resets_at_ms: null,
    over_cap: true,
    ...overrides,
  };
}

describe("SubscriptionQuotaSection (CF-QUOTA-R4)", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.save.mockReset();
    mocks.settings = { endpoints: { chatgpt: {} }, subscription_quota_caps: {} };
  });

  it("shows the used share, the cap, the source and the reset clock", async () => {
    mocks.invoke.mockResolvedValue([status()]);
    render(<SubscriptionQuotaSection />);

    expect(await screen.findByText("本窗口已用 85%（上限 80%）", { exact: false })).toBeTruthy();
    expect(screen.getByText("本周已用 20%（上限 80%）", { exact: false })).toBeTruthy();
    expect(screen.getByText("服务端读数")).toBeTruthy();
    expect(screen.getByText("已达上限，已切到其它端点，为你保留余量")).toBeTruthy();
    expect(screen.getByText(/约 \d{2}:\d{2} 恢复/)).toBeTruthy();
  });

  it("labels an estimated reading as an estimate", async () => {
    mocks.invoke.mockResolvedValue([status({ source: "本地估算", over_cap: false })]);
    render(<SubscriptionQuotaSection />);

    expect(await screen.findByText("本地估算")).toBeTruthy();
    expect(screen.queryByText("已达上限，已切到其它端点，为你保留余量")).toBeNull();
  });

  it("saves a changed window cap through the settings store", async () => {
    mocks.invoke.mockResolvedValue([status()]);
    mocks.save.mockResolvedValue(undefined);
    render(<SubscriptionQuotaSection />);

    const input = await screen.findByLabelText("ChatGPT 五小时窗口上限百分比");
    fireEvent.change(input, { target: { value: "60" } });

    await waitFor(() => expect(mocks.save).toHaveBeenCalledTimes(1));
    const saved = mocks.save.mock.calls[0][0] as {
      subscription_quota_caps: Record<string, { five_hour_percent: number; weekly_percent: number }>;
    };
    expect(saved.subscription_quota_caps.chatgpt).toEqual({
      five_hour_percent: 60,
      weekly_percent: 80,
    });
  });

  it("renders nothing when no subscription endpoint is configured", async () => {
    mocks.invoke.mockResolvedValue([]);
    const { container } = render(<SubscriptionQuotaSection />);
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalled());
    await waitFor(() => expect(container.querySelector("section")).toBeNull());
  });

  it("hides a reset hint once the window has passed", () => {
    expect(resetClockHint(Date.now() - 1000)).toBeNull();
    expect(resetClockHint(undefined)).toBeNull();
    expect(resetClockHint(Date.now() + 3_600_000)).toMatch(/^约 \d{2}:\d{2} 恢复$/);
  });
});
