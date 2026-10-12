// SPDX-License-Identifier: Apache-2.0
import { invoke, type Settings, type SubscriptionQuotaStatus } from "./tauri";
import { useSettingsStore } from "../stores/settings";
import { DispatchRequestError } from "./dispatchErrors";

export const DEFAULT_QUOTA_CAP_PERCENT = 80;
export interface QuotaCapInput {
  endpoint: string;
  five_hour_percent?: number;
  weekly_percent?: number;
}

/** Shared by Settings and the local background entry; never returns credentials. */
export function getQuota(): Promise<SubscriptionQuotaStatus[]> {
  return invoke<SubscriptionQuotaStatus[]>("subscription_quota_status");
}

/** Validate before persistence. Partial updates preserve the other window. */
export async function setQuotaCap(
  input: QuotaCapInput,
  context?: { settings: Settings | null; save: (settings: Settings) => Promise<unknown> },
): Promise<SubscriptionQuotaStatus[]> {
  if (typeof input.endpoint !== "string" || !input.endpoint.trim()) {
    throw new DispatchRequestError("invalid_request", "endpoint must be a non-empty string");
  }
  const patch: Partial<Omit<QuotaCapInput, "endpoint">> = {};
  for (const key of ["five_hour_percent", "weekly_percent"] as const) {
    const value = input[key];
    if (value === undefined) continue;
    if (typeof value !== "number" || !Number.isInteger(value) || value < 1 || value > 100) {
      throw new DispatchRequestError("invalid_request", `${key} must be an integer from 1 to 100`);
    }
    patch[key] = value;
  }
  if (!Object.keys(patch).length) {
    throw new DispatchRequestError("invalid_request", "at least one quota cap must be supplied");
  }
  const store = context ?? useSettingsStore.getState();
  if (!store.settings) throw new Error("settings are not loaded");
  const endpoint = store.settings.endpoints[input.endpoint];
  if (!endpoint) throw new DispatchRequestError("not_found", `unknown endpoint: ${input.endpoint}`);
  if (endpoint.api_style !== "chatgpt") {
    throw new DispatchRequestError("invalid_request", "endpoint must be a subscription endpoint");
  }
  const caps = store.settings.subscription_quota_caps ?? {};
  await store.save({ ...store.settings, subscription_quota_caps: {
    ...caps,
    [input.endpoint]: {
      ...(caps[input.endpoint] ?? { five_hour_percent: DEFAULT_QUOTA_CAP_PERCENT, weekly_percent: DEFAULT_QUOTA_CAP_PERCENT }),
      ...patch,
    },
  } });
  return getQuota();
}
