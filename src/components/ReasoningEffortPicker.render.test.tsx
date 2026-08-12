// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ReasoningEffortPicker } from "./ReasoningEffortPicker";

const setEffort = vi.hoisted(() => vi.fn());

const settingsState = vi.hoisted((): { settings: Record<string, any> } => ({
  settings: {},
}));

const chatState = vi.hoisted((): { activeSession: Record<string, any>; updateActiveSessionReasoningEffort: typeof setEffort } => ({
  activeSession: {},
  updateActiveSessionReasoningEffort: setEffort,
}));

vi.mock("../stores/settings", () => ({
  useSettingsStore: (selector: (state: unknown) => unknown) => selector(settingsState),
}));

vi.mock("../stores/chat", () => ({
  useChatStore: (selector: (state: unknown) => unknown) => selector(chatState),
}));

describe("ReasoningEffortPicker model capabilities", () => {
  beforeEach(() => {
    setEffort.mockReset();
    settingsState.settings = {
      default_endpoint: "chatgpt",
      default_model: "gpt-5.6-sol",
      reasoning_effort: "medium",
      endpoints: {
        chatgpt: {
          base_url: "https://chatgpt.invalid",
          api_style: "chatgpt",
          active_model: "gpt-5.6-sol",
          custom_models: [{
            id: "gpt-5.6-sol",
            default_reasoning_effort: "low",
            supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max"],
          }],
        },
      },
    };
    chatState.activeSession = {
      id: "s1",
      endpoint_id: "chatgpt",
      model_id: "gpt-5.6-sol",
      reasoning_effort: "ultra",
    };
  });

  it("renders transport-supported efforts and maps a legacy ultra selection to max", async () => {
    const user = userEvent.setup();
    render(<ReasoningEffortPicker />);

    const picker = screen.getByRole("combobox", { name: "下一回合思考强度" });
    expect(picker).toHaveClass("min-h-[44px]", "lg:min-h-[36px]");
    expect(picker).toHaveValue("max");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "思考·低",
      "思考·中",
      "思考·高",
      "思考·超高",
      "思考·最大",
    ]);

    await user.selectOptions(picker, "high");
    expect(setEffort).toHaveBeenCalledWith("high");
  });

  it("uses the active session endpoint and model capabilities instead of the global default endpoint", () => {
    settingsState.settings = {
      default_endpoint: "openrouter",
      default_model: "fallback-model",
      reasoning_effort: "medium",
      endpoints: {
        openrouter: {
          base_url: "https://openrouter.invalid",
          api_style: "openai",
          active_model: "fallback-model",
          custom_models: [{
            id: "fallback-model",
            supported_reasoning_efforts: ["low", "high"],
          }],
        },
        chatgpt: {
          base_url: "https://chatgpt.invalid",
          api_style: "chatgpt",
          active_model: "gpt-session",
          custom_models: [{
            id: "gpt-session",
            default_reasoning_effort: "max",
            supported_reasoning_efforts: ["minimal", "max"],
          }],
        },
      },
    };
    chatState.activeSession = {
      id: "s1",
      endpoint_id: "chatgpt",
      model_id: "gpt-session",
      reasoning_effort: null,
    };

    render(<ReasoningEffortPicker />);

    const picker = screen.getByRole("combobox", { name: "下一回合思考强度" });
    expect(picker).toHaveValue("max");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "思考·最简",
      "思考·最大",
    ]);
  });
});
