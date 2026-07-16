// SPDX-License-Identifier: Apache-2.0
import type { CustomModel } from "./tauri";

export const CHATGPT_ENDPOINT_KEY = "chatgpt";
export const CHATGPT_BASE_URL = "https://chatgpt.com/backend-api/codex";

// Display-safe fallback for offline startup. Signed-in sessions refresh this
// from the official Codex catalog; keep the snapshot current so a temporary
// catalog failure never removes subscription models from the product.
export const CHATGPT_FALLBACK_MODELS: CustomModel[] = [
  {
    id: "gpt-5.6-sol",
    name: "GPT-5.6 Sol",
    context_length: 272000,
    max_context_length: 272000,
    effective_context_window_percent: 95,
    default_reasoning_effort: "low",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max"],
  },
  {
    id: "gpt-5.6-terra",
    name: "GPT-5.6 Terra",
    context_length: 272000,
    max_context_length: 272000,
    effective_context_window_percent: 95,
    default_reasoning_effort: "medium",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max"],
  },
  {
    id: "gpt-5.6-luna",
    name: "GPT-5.6 Luna",
    context_length: 272000,
    max_context_length: 272000,
    effective_context_window_percent: 95,
    default_reasoning_effort: "medium",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max"],
  },
  {
    id: "gpt-5.5",
    name: "GPT-5.5",
    context_length: 272000,
    max_context_length: 272000,
    effective_context_window_percent: 95,
    default_reasoning_effort: "medium",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh"],
  },
  {
    id: "gpt-5.4",
    name: "GPT-5.4",
    context_length: 272000,
    max_context_length: 1000000,
    effective_context_window_percent: 95,
    default_reasoning_effort: "medium",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh"],
  },
  {
    id: "gpt-5.4-mini",
    name: "GPT-5.4 Mini",
    context_length: 272000,
    max_context_length: 272000,
    effective_context_window_percent: 95,
    default_reasoning_effort: "medium",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh"],
  },
];

export const CHATGPT_DEFAULT_MODEL = CHATGPT_FALLBACK_MODELS[0].id;

export function selectChatGptCatalog(live: CustomModel[] | null | undefined): CustomModel[] {
  return live && live.length > 0 ? live : CHATGPT_FALLBACK_MODELS;
}

export function selectChatGptDefaultModel(models: CustomModel[]): string {
  return models.some((model) => model.id === CHATGPT_DEFAULT_MODEL)
    ? CHATGPT_DEFAULT_MODEL
    : models[0]?.id ?? CHATGPT_DEFAULT_MODEL;
}
