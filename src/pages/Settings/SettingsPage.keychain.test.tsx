// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { SettingsPage } from "./SettingsPage";

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  saveApiKey: vi.fn(),
  loadModels: vi.fn(),
  codexLogin: vi.fn(),
  codexLogout: vi.fn(),
  codexAccount: vi.fn(),
  codexModels: vi.fn(),
  applyCodexModels: vi.fn(),
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
  applyCodexModels: mocks.applyCodexModels,
}));

describe("SettingsPage keychain handling", () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) {
      mock.mockReset();
    }
    mocks.load.mockResolvedValue(undefined);
    mocks.save.mockResolvedValue(undefined);
    mocks.saveApiKey.mockResolvedValue(undefined);
    mocks.codexAccount.mockResolvedValue(null);
    mocks.applyCodexModels.mockResolvedValue(undefined);
    delete (settingsState.settings.endpoints as Record<string, unknown>).chatgpt;
    settingsState.settings.default_endpoint = "deepseek";
    settingsState.settings.default_model = "deepseek-chat";
    settingsState.settings.theme = "dark";
  });

  it("shows a masked placeholder for a saved key instead of reading it back", async () => {
    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => {
      expect(screen.getByDisplayValue("https://api.deepseek.com")).toBeInTheDocument();
    });

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

    await waitFor(() => expect(mocks.applyCodexModels).toHaveBeenCalled());
    const models = mocks.applyCodexModels.mock.calls[0]?.[0];
    expect(models[0]).toMatchObject({
      id: "gpt-5.6-sol",
      supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    });
  });

  it("keeps subscription models available when live catalog refresh fails", async () => {
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "plus" });
    mocks.codexModels.mockRejectedValue(new Error("offline"));

    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => expect(mocks.applyCodexModels).toHaveBeenCalled());
    expect(mocks.applyCodexModels.mock.calls[0]?.[0]?.[0]?.id).toBe("gpt-5.6-sol");
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
    expect(mocks.applyCodexModels).not.toHaveBeenCalled();
  });

  it("repairs retired selections even when the catalog itself is unchanged", async () => {
    const catalog = [
      {
        id: "gpt-5.6-sol",
        default_reasoning_effort: "low",
        supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
      },
    ];
    (settingsState.settings.endpoints as Record<string, unknown>).chatgpt = {
      base_url: "https://chatgpt.com/backend-api/codex",
      api_style: "chatgpt",
      active_model: "retired-model",
      custom_models: catalog,
    };
    settingsState.settings.default_endpoint = "chatgpt";
    settingsState.settings.default_model = "retired-model";
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "pro" });
    mocks.codexModels.mockResolvedValue(catalog);

    render(<SettingsPage onBack={() => {}} />);

    await waitFor(() => expect(mocks.applyCodexModels).toHaveBeenCalledWith(catalog));
  });

  it("applies a delayed catalog without saving a whole settings snapshot", async () => {
    let resolveCatalog: (models: unknown[]) => void = () => {};
    mocks.codexAccount.mockResolvedValue({ email: "user@example.test", plan: "pro" });
    mocks.codexModels.mockReturnValue(
      new Promise<unknown[]>((resolve) => {
        resolveCatalog = resolve;
      }),
    );

    render(<SettingsPage onBack={() => {}} />);
    await waitFor(() => expect(mocks.codexModels).toHaveBeenCalled());
    resolveCatalog([{ id: "future-model", supported_reasoning_efforts: ["high"] }]);

    await waitFor(() =>
      expect(mocks.applyCodexModels).toHaveBeenCalledWith([
        { id: "future-model", supported_reasoning_efforts: ["high"] },
      ]),
    );
    expect(mocks.save).not.toHaveBeenCalled();
  });
});
