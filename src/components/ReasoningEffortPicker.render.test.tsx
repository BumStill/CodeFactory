// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ReasoningEffortPicker } from "./ReasoningEffortPicker";

const setEffort = vi.hoisted(() => vi.fn());

vi.mock("../stores/settings", () => ({
  useSettingsStore: (selector: (state: unknown) => unknown) => selector({
    settings: {
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
            supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
          }],
        },
      },
    },
  }),
}));

vi.mock("../stores/chat", () => ({
  useChatStore: (selector: (state: unknown) => unknown) => selector({
    activeSession: { id: "s1", model_id: "gpt-5.6-sol", reasoning_effort: "high" },
    updateActiveSessionReasoningEffort: setEffort,
  }),
}));

describe("ReasoningEffortPicker model capabilities", () => {
  it("renders every supported GPT-5.6 Sol effort and persists ultra", async () => {
    setEffort.mockReset();
    const user = userEvent.setup();
    render(<ReasoningEffortPicker />);

    const picker = screen.getByRole("combobox");
    expect(picker).toHaveValue("high");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "思考·低",
      "思考·中",
      "思考·高",
      "思考·超高",
      "思考·最大",
      "思考·极致",
    ]);

    await user.selectOptions(picker, "ultra");
    expect(setEffort).toHaveBeenCalledWith("ultra");
  });
});
