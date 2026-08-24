// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ModelPicker } from "./ModelPicker";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  loadModels: vi.fn(),
  setModel: vi.fn(),
  updateActiveSessionModel: vi.fn(),
  updateActiveSessionModelConfig: vi.fn(),
  updateActiveSessionReasoningEffort: vi.fn(),
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
  updateActiveSessionReasoningEffort: mocks.updateActiveSessionReasoningEffort,
  activeSession: {
    id: "session-a",
    endpoint_id: "chatgpt",
    model_id: "gpt-5.5",
    model_policy: "prefer",
    reasoning_effort: "medium",
  },
}));

const settingsState = vi.hoisted(() => ({
  settings: {
    default_endpoint: "chatgpt",
    reasoning_effort: "medium" as const,
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
  useChatStore: <T,>(selector?: (state: typeof chatState) => T) =>
    selector ? selector(chatState) : chatState,
}));

vi.mock("../stores/settings", () => ({
  useSettingsStore: <T,>(selector?: (state: typeof settingsState) => T) =>
    selector ? selector(settingsState) : settingsState,
}));

describe("ModelPicker", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.loadModels.mockReset();
    mocks.setModel.mockReset();
    mocks.updateActiveSessionModel.mockReset();
    mocks.updateActiveSessionModelConfig.mockReset();
    mocks.updateActiveSessionReasoningEffort.mockReset();
    mocks.reloadSettings.mockReset();
    mocks.saveSettings.mockReset();
    mocks.saveSettings.mockResolvedValue(undefined);
    mocks.reloadSettings.mockResolvedValue(undefined);
    mocks.updateActiveSessionModel.mockResolvedValue(undefined);
    mocks.updateActiveSessionModelConfig.mockResolvedValue(undefined);
    mocks.updateActiveSessionReasoningEffort.mockResolvedValue(undefined);
    chatState.activeSession.model_policy = "prefer";
    chatState.activeSession.reasoning_effort = "medium";
    mocks.invoke.mockImplementation((cmd: string, args: { endpointName?: string }) => {
      if (cmd === "get_endpoint_active_model") {
        return Promise.resolve(args.endpointName === "deepseek" ? "deepseek-v4-pro" : "gpt-5.5");
      }
      return Promise.resolve(undefined);
    });
  });

  it("shows only the model on the default trigger and keeps reasoning inside the model panel", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);
    render(<ModelPicker />);

    const trigger = screen.getByRole("button", { name: /选择下一回合模型/ });
    expect(trigger).toHaveTextContent(/^gpt-5\.5$/);
    expect(trigger).not.toHaveTextContent("chatgpt");
    expect(trigger).not.toHaveTextContent("首选");
    expect(screen.queryByRole("combobox", { name: "下一回合思考强度" })).not.toBeInTheDocument();

    await user.click(trigger);
    const panel = screen.getByRole("dialog", { name: "选择下一回合模型" });
    expect(panel).toContainElement(screen.getByRole("combobox", { name: "下一回合思考强度" }));
    expect(screen.getByRole("combobox", { name: "模型策略" })).toHaveClass(
      "focus-visible:ring-accent",
    );
    expect(screen.getByRole("textbox", { name: "搜索模型" })).toHaveClass(
      "focus-visible:ring-accent",
    );
  });

  it("promotes a non-default fixed policy beside the model", async () => {
    mocks.loadModels.mockResolvedValue(undefined);
    chatState.activeSession.model_policy = "fixed";
    render(<ModelPicker />);

    const trigger = screen.getByRole("button", { name: /选择下一回合模型/ });
    expect(trigger).toHaveTextContent("gpt-5.5");
    expect(trigger).toHaveTextContent("固定");
    expect(trigger).not.toHaveTextContent("chatgpt");
    await waitFor(() => expect(mocks.loadModels).toHaveBeenCalledWith("chatgpt"));
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
    const panel = screen.getByRole("dialog", { name: "选择下一回合模型" });
    expect(within(panel).getByRole("button", { name: "gpt-5.5" })).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("模型端点"), "deepseek");

    await waitFor(() =>
      expect(mocks.updateActiveSessionModelConfig).toHaveBeenCalledWith({
        endpointId: "deepseek",
        modelId: "deepseek-v4-pro",
        policy: "prefer",
      }),
    );
    expect(mocks.saveSettings).not.toHaveBeenCalled();
    expect(within(panel).queryByRole("button", { name: "gpt-5.5" })).not.toBeInTheDocument();
    expect(within(panel).getByText("正在加载模型…")).toBeInTheDocument();
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
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.getAttribute("role") === "dialog") {
          return {
            x: 0,
            y: 0,
            top: 0,
            right: 288,
            bottom: 300,
            left: 0,
            width: 288,
            height: 300,
            toJSON: () => ({}),
          };
        }
        return {
          x: 600,
          y: 600,
          top: 600,
          right: 820,
          bottom: 632,
          left: 600,
          width: 220,
          height: 32,
          toJSON: () => ({}),
        };
      });

    try {
      render(<ModelPicker portal />);

      const trigger = screen.getByRole("button", { name: /选择下一回合模型.*chatgpt.*gpt-5\.5.*首选/ });
      expect(trigger).toHaveClass("min-h-[44px]", "lg:min-h-[36px]");
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

  it("keeps the portal above the whole composer card instead of covering the input row", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);
    const originalWidth = window.innerWidth;
    const originalHeight = window.innerHeight;
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.getAttribute("role") === "dialog") {
          return {
            x: 0,
            y: 0,
            top: 0,
            right: 288,
            bottom: 300,
            left: 0,
            width: 288,
            height: 300,
            toJSON: () => ({}),
          };
        }
        if (this.dataset.testid === "message-input-control-row") {
          return {
            x: 160,
            y: 500,
            top: 500,
            right: 872,
            bottom: 660,
            left: 160,
            width: 712,
            height: 160,
            toJSON: () => ({}),
          };
        }
        return {
          x: 600,
          y: 620,
          top: 620,
          right: 820,
          bottom: 652,
          left: 600,
          width: 220,
          height: 32,
          toJSON: () => ({}),
        };
      });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1044 });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 720 });

    try {
      render(
        <div data-testid="message-input-control-row">
          <ModelPicker portal />
        </div>,
      );
      await user.click(screen.getByRole("button", { name: /选择下一回合模型/ }));

      await waitFor(() => {
        const portal = screen.getByTestId("model-picker-portal-menu");
        const menuBottom = Number.parseFloat(portal.style.top) + 300;
        expect(menuBottom).toBeLessThanOrEqual(496);
      });
    } finally {
      rectSpy.mockRestore();
      Object.defineProperty(window, "innerWidth", { configurable: true, value: originalWidth });
      Object.defineProperty(window, "innerHeight", { configurable: true, value: originalHeight });
    }
  });

  it("opens below a trigger near the viewport top instead of crushing the menu above it", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);
    const originalWidth = window.innerWidth;
    const originalHeight = window.innerHeight;
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.getAttribute("role") === "dialog") {
          return {
            x: 0,
            y: 0,
            top: 0,
            right: 288,
            bottom: 300,
            left: 0,
            width: 288,
            height: 300,
            toJSON: () => ({}),
          };
        }
        return {
          x: 20,
          y: 40,
          top: 40,
          right: 200,
          bottom: 72,
          left: 20,
          width: 180,
          height: 32,
          toJSON: () => ({}),
        };
      });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 800 });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 720 });

    try {
      render(<ModelPicker portal />);
      await user.click(screen.getByRole("button", { name: /选择下一回合模型/ }));

      await waitFor(() => {
        const portal = screen.getByTestId("model-picker-portal-menu");
        expect(Number.parseFloat(portal.style.top)).toBeGreaterThanOrEqual(76);
      });
    } finally {
      rectSpy.mockRestore();
      Object.defineProperty(window, "innerWidth", { configurable: true, value: originalWidth });
      Object.defineProperty(window, "innerHeight", { configurable: true, value: originalHeight });
    }
  });
});
