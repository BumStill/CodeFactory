// SPDX-License-Identifier: Apache-2.0
import { useSettingsStore } from "../stores/settings";
import type { Settings } from "./tauri";
import { evaluateWindow, modelWindowKey, type UsageWindow } from "./usageWindows";
import { invoke } from "./tauri";
import { DispatchRequestError } from "./dispatchErrors";
export function getUsageWindows() {
  const settings = useSettingsStore.getState().settings;
  const rows = new Map<string, { endpoint: string; model: string }>();
  for (const [endpoint, config] of Object.entries(settings?.endpoints ?? {})) {
    for (const model of [...(config.custom_models ?? []).map(m => m.id), ...(config.active_model ? [config.active_model] : [])]) rows.set(modelWindowKey(endpoint, model), { endpoint, model });
  }
  for (const key of Object.keys(settings?.model_usage_windows ?? {})) {
    const [endpoint, model] = JSON.parse(key) as [string, string];
    rows.set(key, { endpoint, model });
  }
  return [...rows.values()].map(({ endpoint, model }) => {
    const own = settings?.model_usage_windows?.[modelWindowKey(endpoint, model)];
    const inherited = settings?.endpoint_usage_windows?.[endpoint];
    const rule = own ?? inherited ?? { mode: "anytime" as const };
    return { endpoint, model, source: own ? "model" : inherited ? "endpoint" : "none", rule, ...evaluateWindow(rule, Date.now()) };
  });
}
/**
 * CF-WINDOW-R6/R7：会话级「忽略时段限制」的一次性豁免。
 * 界面在模型选择器里已经走过这条路径，后台入口必须落到同一个命令上，
 * 不允许另造一条只对派单生效的判断。
 */
export async function setSessionUsageWindowOverride(input: { sessionId: string; ignore: boolean }) {
  if (typeof input.sessionId !== "string" || !input.sessionId.trim()) {
    throw new DispatchRequestError("invalid_request", "session_id must be a non-empty string");
  }
  if (typeof input.ignore !== "boolean") {
    throw new DispatchRequestError("invalid_request", "ignore must be a boolean");
  }
  await invoke("set_session_usage_window_override", { sessionId: input.sessionId, ignore: input.ignore });
  return { session_id: input.sessionId, ignore: input.ignore };
}

export async function setUsageWindow(input: { endpoint: string; model?: string; rule: UsageWindow | null }, context?: {settings: Settings | null; save: (settings: Settings) => Promise<unknown>}) {
  const store = context ?? useSettingsStore.getState();
  if (!store.settings) throw new Error("settings are not loaded");
  const endpoint = store.settings.endpoints[input.endpoint];
  if (!endpoint || (input.model !== undefined && !input.model.trim())) throw new DispatchRequestError("not_found", "unknown endpoint/model");
  const rule = input.rule;
  const field = input.model === undefined ? "endpoint_usage_windows" : "model_usage_windows";
  const key = input.model === undefined ? input.endpoint : modelWindowKey(input.endpoint, input.model);
  if (rule === null) {
    const rules = { ...store.settings[field] };
    delete rules[key];
    await store.save({ ...store.settings, [field]: rules });
    return { endpoint: input.endpoint, model: input.model, inherited: true };
  }
  if (!rule || !["anytime", "official_off_peak", "custom"].includes(rule.mode)) throw new DispatchRequestError("invalid_request", "unknown usage window mode");
  try {
    new Intl.DateTimeFormat("en", { timeZone: rule.timezone ?? "UTC" });
    if ((rule.weekdays ?? []).some(day => !Number.isInteger(day) || day < 0 || day > 6)) throw new Error("weekday must be 0..6");
    for (const period of rule.periods ?? []) {
      if (![period.start, period.end].every(clock => /^([01]\d|2[0-3]):[0-5]\d$/.test(clock))) throw new Error("invalid HH:MM period");
    }
  } catch (error) { throw new DispatchRequestError("invalid_request", String(error)); }
  await store.save({ ...store.settings, [field]: { ...store.settings[field], [key]: structuredClone(rule) } });
  return { endpoint: input.endpoint, model: input.model, rule, ...evaluateWindow(rule, Date.now()) };
}
