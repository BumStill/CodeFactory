// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { applyDispatchRequest, type DispatchHandlers } from "./desktopDispatch";
import { getQuota, setQuotaCap } from "./subscriptionQuota";
import { useSettingsStore } from "../stores/settings";
import type { Settings } from "./tauri";
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("./tauri", () => ({ invoke: mocks.invoke }));
const fixture = () => ({ theme: "dark", font_size: 14, endpoints: {
  subscription: { api_style: "chatgpt", base_url: "https://synthetic.example" },
  paid: { api_style: "openai", base_url: "https://paid.example" },
}, subscription_quota_caps: { other: { five_hour_percent: 40, weekly_percent: 50 } },
} as unknown as Settings);
const handlers = { getQuota, setQuotaCap } as unknown as DispatchHandlers;
const dispatch = (operation: string, request: Record<string, unknown> = {}) =>
  applyDispatchRequest({ request_id: "quota-test", operation, request }, handlers);
describe("CF-QUOTA-R2 background settings integration", () => {
  beforeEach(() => {
    useSettingsStore.setState({ settings: fixture() });
    mocks.invoke.mockReset();
    mocks.invoke.mockImplementation(async (command, args) => command === "save_settings" ? args.newSettings : []);
  });
  it("returns the same usage/caps/reset readings as settings without exposing credentials", async () => {
    const readings = [{ endpoint: "subscription", source: "本地估算", five_hour_percent: 37,
      weekly_percent: 12, five_hour_cap_percent: 80, weekly_cap_percent: 80,
      five_hour_resets_at_ms: 1900000000000, weekly_resets_at_ms: null, over_cap: false }];
    mocks.invoke.mockResolvedValue(readings);
    expect(await dispatch("get_quota")).toEqual({ ok: true, result: readings });
    expect(mocks.invoke).toHaveBeenCalledWith("subscription_quota_status");
  });
  it("persists both caps via the real settings store, preserves other configuration, then reads back", async () => {
    expect((await dispatch("set_quota_cap", { endpoint: "subscription", five_hour_percent: 63, weekly_percent: 71 })).ok).toBe(true);
    expect(mocks.invoke.mock.calls[0][0]).toBe("save_settings");
    expect(useSettingsStore.getState().settings?.subscription_quota_caps).toEqual({
      other: { five_hour_percent: 40, weekly_percent: 50 }, subscription: { five_hour_percent: 63, weekly_percent: 71 },
    });
    expect(mocks.invoke.mock.calls[1][0]).toBe("subscription_quota_status");
  });
  it("partial updates preserve the other window and default missing caps to 80", async () => {
    await dispatch("set_quota_cap", { endpoint: "subscription", weekly_percent: 55 });
    await dispatch("set_quota_cap", { endpoint: "subscription", five_hour_percent: 100 });
    expect(useSettingsStore.getState().settings?.subscription_quota_caps?.subscription).toEqual({ five_hour_percent: 100, weekly_percent: 55 });
  });
  it.each([{}, { five_hour_percent: 0 }, { weekly_percent: 101 }, { weekly_percent: 2.5 }, { weekly_percent: "60" }, { weekly_percent: null }, { five_hour_percent: NaN }])("rejects invalid caps without saving: %j", async (caps) => {
    const result = await dispatch("set_quota_cap", { endpoint: "subscription", ...caps });
    expect(result).toMatchObject({ ok: false, error: { code: "invalid_request" } });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
  it.each([ ["missing", "not_found"], ["paid", "invalid_request"] ])("refuses endpoint %s", async (endpoint, code) => {
    expect(await dispatch("set_quota_cap", { endpoint, weekly_percent: 50 })).toMatchObject({ ok: false, error: { code } });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
  it("reports persistence failure without updating in-memory caps", async () => {
    mocks.invoke.mockRejectedValue(new Error("synthetic disk failure"));
    expect(await dispatch("set_quota_cap", { endpoint: "subscription", weekly_percent: 60 })).toMatchObject({ ok: false, error: { code: "internal" } });
    expect(useSettingsStore.getState().settings?.subscription_quota_caps?.subscription).toBeUndefined();
  });
  it("native protocol, CLI, and mounted dispatch bridge advertise both operations", () => {
    const rust = readFileSync("src-tauri/src/headless_dispatch.rs", "utf8");
    const cli = readFileSync("scripts/dispatch-task.mjs", "utf8");
    const bridge = readFileSync("src/lib/useAppDispatchBridge.ts", "utf8");
    for (const operation of ["get_quota", "set_quota_cap"]) {
      expect(rust).toContain(`"${operation}"`);
      expect(cli).toContain(`"${operation}",`);
    }
    expect(bridge).toContain("getQuota,");
    expect(bridge).toContain("setQuotaCap,");
  });
});
