// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SettingsPage } from "./SettingsPage";

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  saveApiKey: vi.fn(),
  loadModels: vi.fn(),
  codexAccount: vi.fn(),
  codexLogin: vi.fn(),
  codexLoginStart: vi.fn(),
  codexLoginOpen: vi.fn(),
  codexLoginCancel: vi.fn(),
  codexLoginStatus: vi.fn(),
  codexLogout: vi.fn(),
  codexModels: vi.fn(),
  applyCodexModels: vi.fn(),
  invoke: vi.fn(),
  clipboardWrite: vi.fn(),
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
    permissions: { allow: [], ask: [], deny: [], full_access: false },
    shell: { shell: "zsh" },
    auto_create_pr: false,
    theme: "dark" as const,
    font_family: "inter",
    font_size: 14,
    onboarded: true,
  },
  load: mocks.load,
  save: mocks.save,
  saveApiKey: mocks.saveApiKey,
}));

const updaterState = vi.hoisted(() => ({
  phase: { kind: "idle" as const },
  currentVersion: "dev",
  initialize: vi.fn(),
  checkNow: vi.fn(),
  install: vi.fn(),
}));

vi.mock("../../stores/settings", () => {
  function useSettingsStore<T>(selector?: (state: typeof settingsState) => T) {
    return selector ? selector(settingsState) : settingsState;
  }
  useSettingsStore.getState = () => settingsState;
  return { useSettingsStore };
});

vi.mock("../../stores/chat", () => {
  const state = { loadModels: mocks.loadModels };
  function useChatStore() {
    return state;
  }
  useChatStore.getState = () => state;
  return { useChatStore };
});

vi.mock("../../stores/gitRemote", () => ({
  useGitRemoteStore: () => ({
    remotes: [],
    loadRemotes: vi.fn(),
    addRemote: vi.fn(),
    deleteRemote: vi.fn(),
    testRemote: vi.fn(),
  }),
}));

vi.mock("../../stores/updater", () => ({
  useUpdaterStore: <T,>(selector: (state: typeof updaterState) => T) => selector(updaterState),
}));

vi.mock("../../lib/tauri", () => ({
  // The Browser tab subscribes to Chromium download progress.
  onChromiumProgress: () => Promise.resolve(() => {}),
  invoke: mocks.invoke,
  codexAccount: mocks.codexAccount,
  codexLogin: mocks.codexLogin,
  codexLoginStart: mocks.codexLoginStart,
  codexLoginOpen: mocks.codexLoginOpen,
  codexLoginCancel: mocks.codexLoginCancel,
  codexLoginStatus: mocks.codexLoginStatus,
  codexLogout: mocks.codexLogout,
  codexModels: mocks.codexModels,
  applyCodexModels: mocks.applyCodexModels,
}));

describe("Settings ChatGPT authorization recovery", () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) mock.mockReset();
    mocks.load.mockResolvedValue(undefined);
    mocks.save.mockResolvedValue(undefined);
    mocks.codexAccount.mockResolvedValue(null);
    mocks.codexLogin.mockReturnValue(new Promise(() => {}));
    mocks.codexLoginStart.mockResolvedValue({
      flow_id: "flow-1",
      authorization_url: "https://auth.openai.test/authorize?state=stable",
      status: "waiting",
      expires_at: Date.now() + 300_000,
      browser_open_error: "系统未能打开浏览器",
    });
    mocks.clipboardWrite.mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: mocks.clipboardWrite },
    });
  });

  it("returns a recoverable flow immediately and always exposes open copy cancel", async () => {
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    await user.click(await screen.findByRole("button", { name: "登录" }));

    expect(await screen.findByRole("button", { name: "打开验证页面" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制链接" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消" })).toBeInTheDocument();
    expect(screen.queryByText(/已在浏览器中打开/)).not.toBeInTheDocument();
    expect(mocks.codexLoginStart).toHaveBeenCalledTimes(1);
  });

  it("copies the same URL even when the system opener failed", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(<SettingsPage onBack={() => {}} />);
    await user.click(await screen.findByRole("button", { name: "登录" }));
    await user.click(await screen.findByRole("button", { name: "复制链接" }));

    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        "https://auth.openai.test/authorize?state=stable",
      ),
    );
  });
});
