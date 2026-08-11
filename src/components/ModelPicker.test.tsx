// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ModelPicker } from "./ModelPicker";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  loadModels: vi.fn(),
  setModel: vi.fn(),
  updateActiveSessionModel: vi.fn(),
  updateActiveSessionModelConfig: vi.fn(),
  reloadSettings: vi.fn(),
  saveSettings: vi.fn(),
}));

const chatState = vi.hoisted(() => ({
  models: [
    { id: "gpt-5.5", name: "gpt-5.5", context_length: 272000 },
  ],
  activeModel: "gpt-5.5",
  loadModels: mocks.loadModels,
  setModel: mocks.setModel,
  updateActiveSessionModel: mocks.updateActiveSessionModel,
  updateActiveSessionModelConfig: mocks.updateActiveSessionModelConfig,
  activeSession: {
    id: "session-a",
    endpoint_id: "chatgpt",
    model_id: "gpt-5.5",
    model_policy: "prefer",
  },
}));

const settingsState = vi.hoisted(() => ({
  settings: {
    default_endpoint: "chatgpt",
    endpoints: {
      chatgpt: {
        base_url: "https://chatgpt.invalid",
        api_style: "chatgpt" as const,
        custom_models: [{ id: "gpt-5.5", name: "gpt-5.5" }],
        active_model: "gpt-5.5",
      },
      deepseek: {
        base_url: "https://api.deepseek.com",
        api_style: "openai" as const,
        custom_models: [{ id: "deepseek-v4-pro", name: "deepseek-v4-pro" }],
        active_model: "deepseek-v4-pro",
      },
    },
  },
  load: mocks.reloadSettings,
  save: mocks.saveSettings,
}));

vi.mock("../lib/tauri", () => ({
  invoke: mocks.invoke,
}));

vi.mock("../stores/chat", () => ({
  useChatStore: () => chatState,
}));

vi.mock("../stores/settings", () => ({
  useSettingsStore: () => settingsState,
}));

describe("ModelPicker", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.loadModels.mockReset();
    mocks.setModel.mockReset();
    mocks.updateActiveSessionModel.mockReset();
    mocks.updateActiveSessionModelConfig.mockReset();
    mocks.reloadSettings.mockReset();
    mocks.saveSettings.mockReset();
    mocks.saveSettings.mockResolvedValue(undefined);
    mocks.reloadSettings.mockResolvedValue(undefined);
    mocks.updateActiveSessionModel.mockResolvedValue(undefined);
    mocks.updateActiveSessionModelConfig.mockResolvedValue(undefined);
    mocks.invoke.mockImplementation((cmd: string, args: { endpointName?: string }) => {
      if (cmd === "get_endpoint_active_model") {
        return Promise.resolve(args.endpointName === "deepseek" ? "deepseek-v4-pro" : "gpt-5.5");
      }
      return Promise.resolve(undefined);
    });
  });

  it("hides stale model rows while the newly selected endpoint loads", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockImplementation((endpoint: string) => {
      if (endpoint === "deepseek") {
        return new Promise<void>(() => {});
      }
      return Promise.resolve();
    });

    render(<ModelPicker />);

    await user.click(screen.getByRole("button", { name: /chatgpt/ }));
    expect(screen.getByText("gpt-5.5")).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("模型端点"), "deepseek");

    await waitFor(() =>
      expect(mocks.updateActiveSessionModelConfig).toHaveBeenCalledWith({
        endpointId: "deepseek",
        modelId: "deepseek-v4-pro",
        policy: "prefer",
      }),
    );
    expect(mocks.saveSettings).not.toHaveBeenCalled();
    expect(screen.queryByText("gpt-5.5")).not.toBeInTheDocument();
    expect(screen.getByText("正在加载模型…")).toBeInTheDocument();
  });

  it("changes only the active session policy and explains next-turn semantics", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);

    render(<ModelPicker />);
    await user.click(screen.getByRole("button", { name: /chatgpt/ }));
    await user.selectOptions(screen.getByLabelText("模型策略"), "fixed");

    expect(mocks.updateActiveSessionModelConfig).toHaveBeenCalledWith({
      endpointId: "chatgpt",
      modelId: "gpt-5.5",
      policy: "fixed",
    });
    expect(mocks.saveSettings).not.toHaveBeenCalled();
    expect(
      screen.getByText(/会话策略更改只从下一轮开始生效/),
    ).toBeInTheDocument();
  });

  it("does not replace an existing session model with the endpoint default", async () => {
    mocks.loadModels.mockResolvedValue(undefined);
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_endpoint_active_model") {
        return Promise.resolve("anthropic/claude-opus-4-7");
      }
      return Promise.resolve(undefined);
    });

    render(<ModelPicker />);

    await waitFor(() => expect(mocks.loadModels).toHaveBeenCalledWith("chatgpt"));
    expect(mocks.setModel).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: /chatgpt.*gpt-5\.5.*首选/ }),
    ).toBeInTheDocument();
  });

  it("exposes an owned popup, opens upward from the composer, and returns focus on Escape", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);
    const rectSpy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 600,
      y: 600,
      top: 600,
      right: 820,
      bottom: 632,
      left: 600,
      width: 220,
      height: 32,
      toJSON: () => ({}),
    });

    try {
      render(<ModelPicker portal />);

      const trigger = screen.getByRole("button", { name: /选择下一回合模型.*chatgpt.*gpt-5\.5.*首选/ });
      expect(trigger).toHaveClass("min-h-11", "lg:min-h-9");
      expect(trigger).toHaveAttribute("aria-expanded", "false");
      expect(trigger).toHaveAttribute("aria-controls");

      await user.click(trigger);
      expect(trigger).toHaveAttribute("aria-expanded", "true");
      const controlledId = trigger.getAttribute("aria-controls");
      expect(controlledId).toBeTruthy();
      expect(document.getElementById(controlledId as string)).toBeInTheDocument();
      const portal = screen.getByTestId("model-picker-portal-menu");
      expect(Number.parseFloat(portal.style.top)).toBeLessThan(600);

      await user.keyboard("{Escape}");
      expect(screen.queryByTestId("model-picker-portal-menu")).not.toBeInTheDocument();
      expect(trigger).toHaveAttribute("aria-expanded", "false");
      expect(trigger).toHaveFocus();
    } finally {
      rectSpy.mockRestore();
    }
  });
});
