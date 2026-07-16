// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { SettingsPage } from "./SettingsPage";

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  saveApiKey: vi.fn(),
  getApiKey: vi.fn(),
  loadModels: vi.fn(),
  codexLogin: vi.fn(),
  codexLogout: vi.fn(),
  codexAccount: vi.fn(),
  codexModels: vi.fn(),
  invoke: vi.fn(),
  loadRemotes: vi.fn(),
  addRemote: vi.fn(),
  deleteRemote: vi.fn(),
  testRemote: vi.fn(),
  updaterInitialize: vi.fn(),
  updaterCheckNow: vi.fn(),
  updaterInstall: vi.fn(),
}));

const settingsState = vi.hoisted(() => ({
  settings: {
    endpoints: {
      deepseek: {
        base_url: "https://api.deepseek.com",
        api_style: "openai" as const,
        key_ref: "codefactory.endpoint.deepseek",
        custom_models: [{ id: "deepseek-chat", name: "DeepSeek Chat" }],
        active_model: "deepseek-chat",
      },
    },
    default_endpoint: "deepseek",
    default_model: "deepseek-chat",
    permissions: {
      allow: [],
      ask: [],
      deny: [],
      full_access: false,
    },
    shell: { shell: "zsh" },
    auto_create_pr: false,
    theme: "dark" as const,
    font_family: "inter",
    font_size: 14,
    reasoning_effort: "medium" as const,
    onboarded: true,
  },
  load: mocks.load,
  save: mocks.save,
  saveApiKey: mocks.saveApiKey,
  getApiKey: mocks.getApiKey,
}));

const chatState = vi.hoisted(() => ({
  loadModels: mocks.loadModels,
}));

const gitRemoteState = vi.hoisted(() => ({
  remotes: [],
  loadRemotes: mocks.loadRemotes,
  addRemote: mocks.addRemote,
  deleteRemote: mocks.deleteRemote,
  testRemote: mocks.testRemote,
}));

const updaterState = vi.hoisted(() => ({
  phase: { kind: "idle" as const },
  currentVersion: "dev",
  initialize: mocks.updaterInitialize,
  checkNow: mocks.updaterCheckNow,
  install: mocks.updaterInstall,
}));

vi.mock("../../stores/settings", () => {
  function useSettingsStore<T>(selector?: (state: typeof settingsState) => T) {
    return selector ? selector(settingsState) : settingsState;
  }
  useSettingsStore.getState = () => settingsState;
  return { useSettingsStore };
});

vi.mock("../../stores/chat", () => {
  function useChatStore() {
    return chatState;
  }
  useChatStore.getState = () => chatState;
  return { useChatStore };
});

vi.mock("../../stores/gitRemote", () => ({
  useGitRemoteStore: () => gitRemoteState,
}));

vi.mock("../../stores/updater", () => ({
  useUpdaterStore: <T,>(selector: (state: typeof updaterState) => T) => selector(updaterState),
}));

vi.mock("../../lib/tauri", () => ({
  invoke: mocks.invoke,
  codexLogin: mocks.codexLogin,
  codexLogout: mocks.codexLogout,
  codexAccount: mocks.codexAccount,
  codexModels: mocks.codexModels,
}));

describe("SettingsPage keychain handling", () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) {
      mock.mockReset();
    }
    mocks.load.mockResolvedValue(undefined);
    mocks.save.mockResolvedValue(undefined);
    mocks.saveApiKey.mockResolvedValue(undefined);
    mocks.getApiKey.mockRejectedValue(new Error("settings page must not read saved API keys"));
    mocks.codexAccount.mockResolvedValue(null);
    delete (settingsState.settings.endpoints as Record<string, unknown>).chatgpt;
    settingsState.settings.default_endpoint = "deepseek";
    settingsState.settings.default_model = "deepseek-chat";
    settingsState.settings.theme = "dark";
  });

  it("does not read saved API keys when opening the endpoint settings", async () => {
    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => {
      expect(screen.getByDisplayValue("https://api.deepseek.com")).toBeInTheDocument();
    });

    expect(mocks.getApiKey).not.toHaveBeenCalled();
    expect(screen.getByPlaceholderText("已保存，输入新密钥以替换")).toBeInTheDocument();
  });

  it("hydrates the ChatGPT endpoint from the live Codex model catalog", async () => {
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "pro" });
    mocks.codexModels.mockResolvedValue([
      {
        id: "gpt-5.6-sol",
        name: "GPT-5.6 Sol",
        context_length: 272000,
        default_reasoning_effort: "low",
        supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
      },
    ]);

    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => expect(mocks.save).toHaveBeenCalled());
    const saved = mocks.save.mock.calls[mocks.save.mock.calls.length - 1]?.[0];
    expect(saved.endpoints.chatgpt.custom_models[0]).toMatchObject({
      id: "gpt-5.6-sol",
      supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    });
  });

  it("keeps subscription models available when live catalog refresh fails", async () => {
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "plus" });
    mocks.codexModels.mockRejectedValue(new Error("offline"));

    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => expect(mocks.save).toHaveBeenCalled());
    const saved = mocks.save.mock.calls[mocks.save.mock.calls.length - 1]?.[0];
    expect(saved.endpoints.chatgpt.custom_models[0].id).toBe("gpt-5.6-sol");
  });

  it("preserves the last successful catalog and model selection while offline", async () => {
    (settingsState.settings.endpoints as Record<string, unknown>).chatgpt = {
      base_url: "https://chatgpt.com/backend-api/codex",
      api_style: "chatgpt",
      active_model: "future-model",
      custom_models: [
        {
          id: "future-model",
          default_reasoning_effort: "high",
          supported_reasoning_efforts: ["high"],
        },
      ],
    };
    settingsState.settings.default_endpoint = "chatgpt";
    settingsState.settings.default_model = "future-model";
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "pro" });
    mocks.codexModels.mockRejectedValue(new Error("offline"));

    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => expect(mocks.codexModels).toHaveBeenCalled());
    expect(mocks.save).not.toHaveBeenCalled();
  });

  it("merges a delayed catalog response into the latest settings snapshot", async () => {
    let resolveCatalog: (models: unknown[]) => void = () => {};
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "pro" });
    mocks.codexModels.mockReturnValue(
      new Promise<unknown[]>((resolve) => {
        resolveCatalog = resolve;
      }),
    );

    render(<SettingsPage onBack={() => {}} />);
    await waitFor(() => expect(mocks.codexModels).toHaveBeenCalled());
    (settingsState.settings as { theme: "dark" | "light" }).theme = "light";
    resolveCatalog([{ id: "future-model", supported_reasoning_efforts: ["high"] }]);

    await waitFor(() => expect(mocks.save).toHaveBeenCalled());
    const saved = mocks.save.mock.calls[mocks.save.mock.calls.length - 1]?.[0];
    expect(saved.theme).toBe("light");
    expect(saved.endpoints.chatgpt.active_model).toBe("future-model");
  });
});
