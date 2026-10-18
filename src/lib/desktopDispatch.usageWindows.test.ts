import { beforeEach, describe, expect, it, vi } from "vitest";
import { applyDispatchRequest, type DispatchHandlers } from "./desktopDispatch";
import { getUsageWindows, setUsageWindow, setSessionUsageWindowOverride } from "./modelUsageWindows";
import { applyPreset, modelWindowKey } from "./usageWindows";
import { useSettingsStore } from "../stores/settings";
import type { Settings } from "./tauri";
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("./tauri", () => ({ invoke: mocks.invoke }));
const handlers = { getUsageWindows, setUsageWindow } as unknown as DispatchHandlers;
const dispatch = (operation: string, request: Record<string, unknown> = {}) => applyDispatchRequest({request_id:"window-test",operation,request}, handlers);
describe("CF-WINDOW-R1/R7 model settings dispatch", () => {
  beforeEach(() => {
    useSettingsStore.setState({ settings: { theme:"dark", font_size:14, endpoints:{a:{api_style:"openai",base_url:"https://synthetic.example",custom_models:[{id:"pro"},{id:"flash"}]}}, subscription_quota_caps:{a:{five_hour_percent:80,weekly_percent:70}} } as unknown as Settings });
    mocks.invoke.mockReset();
    mocks.invoke.mockImplementation(async (_command, args) => args.newSettings);
  });
  it("writes independent model rules through save_settings and reads both back", async () => {
    expect((await dispatch("set_usage_window",{endpoint:"a",model:"pro",rule:applyPreset("deepseek_off_peak")})).ok).toBe(true);
    expect((await dispatch("set_usage_window",{endpoint:"a",model:"flash",rule:{mode:"anytime"}})).ok).toBe(true);
    const config = useSettingsStore.getState().settings!;
    expect(config.model_usage_windows?.[modelWindowKey("a","pro")]?.mode).toBe("official_off_peak");
    expect(config.model_usage_windows?.[modelWindowKey("a","flash")]?.mode).toBe("anytime");
    expect(config.subscription_quota_caps?.a.weekly_percent).toBe(70);
    expect(await dispatch("get_usage_windows")).toMatchObject({ok:true,result:[{endpoint:"a",model:"pro"},{endpoint:"a",model:"flash"}]});
    expect(mocks.invoke.mock.calls.every(([command]) => command === "save_settings")).toBe(true);
  });
  it("R1/R3/R6 endpoint default covers new models and model exceptions override it", async () => {
    const rule = applyPreset("deepseek_off_peak");
    expect(await dispatch("set_usage_window", { endpoint: "a", rule })).toMatchObject({ ok: true });
    const current = useSettingsStore.getState().settings!;
    useSettingsStore.setState({ settings: { ...current, endpoints: { ...current.endpoints, a: { ...current.endpoints.a, custom_models: [...(current.endpoints.a.custom_models ?? []), { id: "new-model", name: "New" }] } } } });
    expect(await dispatch("get_usage_windows")).toMatchObject({ ok: true, result: expect.arrayContaining([expect.objectContaining({ endpoint: "a", model: "new-model", source: "endpoint", rule })]) });
    expect(await dispatch("set_usage_window", { endpoint: "a", model: "pro", rule: { mode: "anytime" } })).toMatchObject({ ok: true });
    expect(getUsageWindows().find(row => row.model === "pro")).toMatchObject({ source: "model", rule: { mode: "anytime" } });
    expect(await dispatch("set_usage_window", { endpoint: "a", model: "pro", rule: null })).toMatchObject({ ok: true });
    expect(getUsageWindows().find(row => row.model === "pro")).toMatchObject({ source: "endpoint", rule });
  });
  it.each([{mode:"bad"},{mode:"custom",timezone:"bad-zone"},{mode:"custom",periods:[{start:"99:00",end:"12:00"}]}])("rejects invalid rules %j", async rule => {
    expect(await dispatch("set_usage_window",{endpoint:"a",model:"pro",rule})).toMatchObject({ok:false,error:{code:"invalid_request"}});
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

// CF-WINDOW-R6/R7：新增的用例，覆盖此前没有入口的「本会话忽略时段限制」。
describe("CF-WINDOW-R6/R7 background entry for the per-session exemption", () => {
  const overrideHandlers = { setSessionUsageWindowOverride } as unknown as DispatchHandlers;
  const override = (request: Record<string, unknown>) =>
    applyDispatchRequest({ request_id: "override-test", operation: "set_session_usage_window_override", request }, overrideHandlers);
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.invoke.mockImplementation(async (_command, args) => args);
  });
  it("reaches the same tauri command the model picker uses", async () => {
    expect(await override({ session_id: "session-1", ignore: true })).toMatchObject({ ok: true, result: { session_id: "session-1", ignore: true } });
    expect(mocks.invoke).toHaveBeenCalledWith("set_session_usage_window_override", { sessionId: "session-1", ignore: true });
  });
  it.each([[{ session_id: "session-1", ignore: "yes" }], [{ session_id: "   ", ignore: true }]])("refuses %j without touching the app", async request => {
    expect(await override(request)).toMatchObject({ ok: false, error: { code: "invalid_request" } });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
