// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { reasoningEffortsForModel, reasoningPickerVisible } from "./ReasoningEffortPicker";
import type { Settings } from "../lib/tauri";

function settingsWith(defaultEndpoint: string, endpoints: Settings["endpoints"]): Settings {
  return {
    endpoints,
    default_endpoint: defaultEndpoint,
    default_model: "m",
    permissions: { allow: [], ask: [], deny: [], full_access: false },
    shell: { shell: "bash" },
    auto_create_pr: false,
    theme: "dark",
    font_family: "inter",
    font_size: 14,
  } as Settings;
}

describe("reasoningPickerVisible", () => {
  it("is hidden when there are no settings", () => {
    expect(reasoningPickerVisible(null)).toBe(false);
  });

  it("is hidden (not crashing) when settings are partial / endpoints missing", () => {
    expect(reasoningPickerVisible({ default_endpoint: "x" } as unknown as Settings)).toBe(false);
    expect(reasoningPickerVisible({} as unknown as Settings)).toBe(false);
  });

  it("shows only for the active ChatGPT/Codex endpoint", () => {
    const chatgpt = settingsWith("chatgpt", {
      chatgpt: { base_url: "x", api_style: "chatgpt" },
    });
    expect(reasoningPickerVisible(chatgpt)).toBe(true);
  });

  it("is hidden for non-chatgpt endpoints", () => {
    const openai = settingsWith("openrouter", {
      openrouter: { base_url: "x", api_style: "openai" },
      chatgpt: { base_url: "y", api_style: "chatgpt" },
    });
    // chatgpt exists but isn't the active default → hidden
    expect(reasoningPickerVisible(openai)).toBe(false);
  });

  it("uses the active model catalog instead of a global hard-coded ceiling", () => {
    const chatgpt = settingsWith("chatgpt", {
      chatgpt: {
        base_url: "x",
        api_style: "chatgpt",
        custom_models: [
          {
            id: "gpt-5.6-sol",
            supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max"],
          },
          {
            id: "gpt-5.5",
            supported_reasoning_efforts: ["low", "medium", "high", "xhigh"],
          },
        ],
      },
    });

    expect(reasoningEffortsForModel(chatgpt, "gpt-5.6-sol")).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
    ]);
    expect(reasoningEffortsForModel(chatgpt, "gpt-5.5")).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
    ]);
  });

  it("keeps legacy ChatGPT settings usable when capability metadata is absent", () => {
    const chatgpt = settingsWith("chatgpt", {
      chatgpt: { base_url: "x", api_style: "chatgpt", custom_models: [{ id: "legacy" }] },
    });

    expect(reasoningEffortsForModel(chatgpt, "legacy")).toEqual([
      "minimal",
      "low",
      "medium",
      "high",
      "xhigh",
    ]);
  });
});
