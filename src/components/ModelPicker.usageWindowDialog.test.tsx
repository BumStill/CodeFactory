// SPDX-License-Identifier: Apache-2.0
// CF-WINDOW-R6（M86 返修）：用户在会话里手动指定一个当前不在时段内的模型时，
// 提示必须真的**弹得出来**——不能再依赖 Tauri WebView 里不保证可用的
// window.alert / window.confirm，那正是"点了却毫无反应"的原因。
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
  updateActiveSessionReasoningEffort: vi.fn(),
  reloadSettings: vi.fn(),
  saveSettings: vi.fn(),
}));

const chatState = vi.hoisted(() => ({
  models: [{ id: "gpt-5.5", name: "gpt-5.5", context_length: 272000 }],
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
    },
    // Deterministic "never inside the window, and it never rolls around":
    // no permitted weekday, so the rule is never satisfied.
    model_usage_windows: {
      [JSON.stringify(["chatgpt", "gpt-5.5"])]: {
        mode: "custom" as const,
        timezone: "UTC",
        weekdays: [] as number[],
        periods: [{ start: "00:00", end: "23:59" }],
        invert: false,
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

describe("ModelPicker usage-window prompt", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.loadModels.mockReset();
    mocks.setModel.mockReset();
    mocks.updateActiveSessionModel.mockReset();
    mocks.updateActiveSessionModelConfig.mockReset();
    mocks.updateActiveSessionReasoningEffort.mockReset();
    mocks.reloadSettings.mockReset();
    mocks.saveSettings.mockReset();
    mocks.loadModels.mockResolvedValue(undefined);
    mocks.reloadSettings.mockResolvedValue(undefined);
    mocks.saveSettings.mockResolvedValue(undefined);
    mocks.updateActiveSessionModel.mockResolvedValue(undefined);
    mocks.updateActiveSessionModelConfig.mockResolvedValue(undefined);
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_session_usage_window_overrides") return Promise.resolve([]);
      if (cmd === "get_endpoint_active_model") return Promise.resolve("gpt-5.5");
      return Promise.resolve(undefined);
    });
  });

  it("never falls back to the native browser dialogs for CF-WINDOW-R6", async () => {
    const alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
    const confirmSpy = vi.spyOn(window, "confirm").mockImplementation(() => true);
    const user = userEvent.setup();
    render(<ModelPicker />);

    await user.click(screen.getByRole("button", { name: /选择下一回合模型/ }));
    await user.click(await screen.findByRole("button", { name: "gpt-5.5" }));

    expect(alertSpy).not.toHaveBeenCalled();
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(await screen.findByRole("dialog", { name: /不在允许使用时段/ })).toBeInTheDocument();
    alertSpy.mockRestore();
    confirmSpy.mockRestore();
  });

  it("explains the reason in the app and offers the one-time per-session override", async () => {
    const user = userEvent.setup();
    render(<ModelPicker />);

    await user.click(screen.getByRole("button", { name: /选择下一回合模型/ }));
    await user.click(await screen.findByRole("button", { name: "gpt-5.5" }));

    const dialog = await screen.findByRole("dialog", { name: /不在允许使用时段/ });
    expect(dialog).toHaveTextContent("chatgpt");
    expect(dialog).toHaveTextContent("gpt-5.5");
    // The model must not be swapped silently while the prompt is open.
    expect(mocks.updateActiveSessionModelConfig).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /本会话忽略时段限制/ }));

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("set_session_usage_window_override", {
        sessionId: "session-a",
        ignore: true,
      }),
    );
    await waitFor(() =>
      expect(mocks.updateActiveSessionModelConfig).toHaveBeenCalledWith({
        endpointId: "chatgpt",
        modelId: "gpt-5.5",
        policy: "prefer",
      }),
    );
  });

  it("leaves the model untouched when the user declines the override", async () => {
    const user = userEvent.setup();
    render(<ModelPicker />);

    await user.click(screen.getByRole("button", { name: /选择下一回合模型/ }));
    await user.click(await screen.findByRole("button", { name: "gpt-5.5" }));
    await user.click(await screen.findByRole("button", { name: /取消/ }));

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "set_session_usage_window_override",
      expect.anything(),
    );
    expect(mocks.updateActiveSessionModelConfig).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: /不在允许使用时段/ })).not.toBeInTheDocument(),
    );
  });

  it("lets the user revoke a persisted session override from the same menu", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_session_usage_window_overrides") return Promise.resolve(["session-a"]);
      if (cmd === "get_endpoint_active_model") return Promise.resolve("gpt-5.5");
      return Promise.resolve(undefined);
    });
    const user = userEvent.setup();
    render(<ModelPicker />);

    await user.click(screen.getByRole("button", { name: /选择下一回合模型/ }));
    await user.click(await screen.findByRole("button", { name: /恢复本会话的时段限制/ }));

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("set_session_usage_window_override", {
        sessionId: "session-a",
        ignore: false,
      }),
    );
  });
});
