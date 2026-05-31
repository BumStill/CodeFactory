// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { reasoningPickerVisible } from "./ReasoningEffortPicker";
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
});
