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
    mocks.reloadSettings.mockReset();
    mocks.saveSettings.mockReset();
    mocks.saveSettings.mockResolvedValue(undefined);
    mocks.reloadSettings.mockResolvedValue(undefined);
    mocks.updateActiveSessionModel.mockResolvedValue(undefined);
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

    await user.selectOptions(screen.getByRole("combobox"), "deepseek");

    await waitFor(() => expect(mocks.saveSettings).toHaveBeenCalled());
    expect(screen.queryByText("gpt-5.5")).not.toBeInTheDocument();
    expect(screen.getByText("正在加载模型…")).toBeInTheDocument();
  });
});
