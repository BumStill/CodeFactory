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

  it("anchors the portal above a trigger near the viewport bottom", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);
    const rectSpy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if ((this as HTMLElement).getAttribute("aria-label") === "选择模型") {
        return { left: 600, right: 780, top: 650, bottom: 682, width: 180, height: 32, x: 600, y: 650, toJSON: () => ({}) } as DOMRect;
      }
      return { left: 0, right: 288, top: 0, bottom: 300, width: 288, height: 300, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
    });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 800 });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 720 });

    render(<ModelPicker portal prominent />);
    await user.click(screen.getByRole("button", { name: "选择模型" }));

    await waitFor(() => {
      const menu = screen.getByTestId("model-picker-portal-menu");
      expect(Number.parseFloat(menu.style.top)).toBeLessThan(650);
      expect(Number.parseFloat(menu.style.left)).toBeGreaterThanOrEqual(8);
      expect(Number.parseFloat(menu.style.left) + 288).toBeLessThanOrEqual(792);
    });
    rectSpy.mockRestore();
  });

  it("anchors the portal below a trigger near the viewport top", async () => {
    const user = userEvent.setup();
    mocks.loadModels.mockResolvedValue(undefined);
    const rectSpy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if ((this as HTMLElement).getAttribute("aria-label") === "选择模型") {
        return { left: 20, right: 200, top: 40, bottom: 72, width: 180, height: 32, x: 20, y: 40, toJSON: () => ({}) } as DOMRect;
      }
      return { left: 0, right: 288, top: 0, bottom: 300, width: 288, height: 300, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
    });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 800 });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 720 });

    render(<ModelPicker portal prominent />);
    await user.click(screen.getByRole("button", { name: "选择模型" }));

    await waitFor(() => {
      const menu = screen.getByTestId("model-picker-portal-menu");
      expect(Number.parseFloat(menu.style.top)).toBeGreaterThanOrEqual(76);
    });
    rectSpy.mockRestore();
  });
});
