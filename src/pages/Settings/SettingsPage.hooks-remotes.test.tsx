// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
  setTheme: vi.fn(),
  setFontFamily: vi.fn(),
  setFontSize: vi.fn(),
}));

const chatState = vi.hoisted(() => ({
  loadModels: mocks.loadModels,
}));

const gitRemoteState = vi.hoisted(() => ({
  remotes: [] as Array<{
    id: string;
    name: string;
    provider: "github" | "gitlab";
    base_url: string;
    default_repo: string | null;
  }>,
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

const existingHook = {
  id: "hook-1",
  name: "Auto commit",
  event: "post_task",
  action: { type: "auto_git_commit", message_template: "chore: {task_title}" },
  enabled: true,
  filter: null,
};

function setupInvoke() {
  let hooks = [existingHook];
  mocks.invoke.mockImplementation(async (cmd: string, args?: any) => {
    switch (cmd) {
      case "github_cli_credential_status":
        return { installed: true, authenticated: true };
      case "list_hooks":
        return hooks;
      case "update_hook":
        hooks = hooks.map((h) => h.id === args.id ? args.config : h);
        return undefined;
      case "delete_hook":
        hooks = hooks.filter((h) => h.id !== args.id);
        return undefined;
      case "test_hook":
        return "hook ok";
      case "add_hook":
        hooks = [...hooks, args.config];
        return undefined;
      default:
        return undefined;
    }
  });
}

describe("SettingsPage Hooks and Remotes tabs", () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) {
      mock.mockReset();
    }
    mocks.load.mockResolvedValue(undefined);
    mocks.save.mockResolvedValue(undefined);
    mocks.saveApiKey.mockResolvedValue(undefined);
    mocks.codexAccount.mockResolvedValue(null);
    mocks.loadRemotes.mockResolvedValue(undefined);
    mocks.addRemote.mockResolvedValue(undefined);
    mocks.deleteRemote.mockResolvedValue(undefined);
    mocks.testRemote.mockResolvedValue("octocat");
    gitRemoteState.remotes = [];
    setupInvoke();
  });

  it("loads hooks and exposes accessible toggle, test, and delete actions", async () => {
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    await user.click(screen.getByRole("tab", { name: "钩子" }));

    expect(await screen.findByText("Auto commit")).toBeInTheDocument();
    expect(mocks.invoke).toHaveBeenCalledWith("list_hooks");

    await user.click(screen.getByRole("button", { name: "禁用钩子 Auto commit" }));
    expect(mocks.invoke).toHaveBeenCalledWith("update_hook", {
      id: "hook-1",
      config: expect.objectContaining({ id: "hook-1", enabled: false }),
    });

    await user.click(screen.getByRole("button", { name: "测试钩子 Auto commit" }));
    expect(mocks.invoke).toHaveBeenCalledWith("test_hook", { id: "hook-1" });
    expect(await screen.findByText("hook ok")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "删除钩子 Auto commit" }));
    expect(mocks.invoke).toHaveBeenCalledWith("delete_hook", { id: "hook-1" });
    await waitFor(() => expect(screen.queryByText("Auto commit")).not.toBeInTheDocument());
  });

  it("adds a hook from labeled fields and sends the selected action payload", async () => {
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    await user.click(screen.getByRole("tab", { name: "钩子" }));
    await user.click(await screen.findByRole("button", { name: /添加钩子/ }));

    await user.type(screen.getByLabelText("名称"), "Log shell commands");
    await user.selectOptions(screen.getByLabelText("事件"), "pre_tool");
    await user.selectOptions(screen.getByLabelText("动作类型"), "run_command");
    await user.type(screen.getByLabelText("过滤器(可选)"), "bash");
    await user.type(screen.getByLabelText("运行命令"), "echo hook fired");
    await user.click(screen.getByRole("button", { name: "添加钩子" }));

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("add_hook", {
      config: expect.objectContaining({
        name: "Log shell commands",
        event: "pre_tool",
        enabled: true,
        filter: "bash",
        action: { type: "run_command", command: "echo hook fired", cwd: null },
      }),
    }));
  });

  it("loads remotes and exposes accessible test and delete actions", async () => {
    gitRemoteState.remotes = [{
      id: "remote-1",
      name: "origin",
      provider: "github",
      base_url: "https://api.github.com",
      default_repo: "BumStill/CodeFactory",
    }];
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    await user.click(screen.getByRole("tab", { name: "远程仓库" }));

    expect(await screen.findByText("origin")).toBeInTheDocument();
    expect(mocks.loadRemotes).toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "测试远程仓库 origin" }));
    expect(mocks.testRemote).toHaveBeenCalledWith("remote-1");
    expect(await screen.findByText("✓ @octocat")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "删除远程仓库 origin" }));
    expect(mocks.deleteRemote).toHaveBeenCalledWith("remote-1");
  });

  it("shows that an authenticated GitHub CLI is automatically reused", async () => {
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    await user.click(screen.getByRole("tab", { name: "远程仓库" }));

    expect(await screen.findByText(/已登录 GitHub CLI/)).toBeInTheDocument();
    expect(screen.getByText(/无需重复配置 token/)).toBeInTheDocument();
    expect(mocks.invoke).toHaveBeenCalledWith("github_cli_credential_status");
    expect(screen.queryByText("尚未配置远程仓库。")).not.toBeInTheDocument();
  });

  it("adds a remote from labeled fields and auto-fills provider API URLs", async () => {
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    await user.click(screen.getByRole("tab", { name: "远程仓库" }));
    await user.click(await screen.findByRole("button", { name: /添加远程仓库/ }));

    await user.type(screen.getByLabelText("名称"), "GitLab CI");
    await user.selectOptions(screen.getByLabelText("提供商"), "gitlab");
    expect(screen.getByLabelText("基础 URL")).toHaveValue("https://gitlab.com/api/v4");
    await user.type(screen.getByLabelText("个人访问令牌"), "glpat-secret");
    await user.type(screen.getByLabelText("默认仓库(可选)"), "group/project");
    await user.click(screen.getByRole("button", { name: "添加远程仓库" }));

    await waitFor(() => expect(mocks.addRemote).toHaveBeenCalledWith({
      name: "GitLab CI",
      provider: "gitlab",
      base_url: "https://gitlab.com/api/v4",
      token: "glpat-secret",
      default_repo: "group/project",
    }));
  });

  it("exposes settings navigation as grouped tabs with a current-page purpose", async () => {
    const user = userEvent.setup();
    render(<SettingsPage onBack={() => {}} />);

    const nav = screen.getByRole("tablist", { name: "设置分类" });
    expect(nav).toBeInTheDocument();
    expect(screen.getByText("工作流")).toBeInTheDocument();
    expect(screen.getByText("模型与连接")).toBeInTheDocument();

    expect(screen.getByRole("tab", { selected: true })).toHaveAccessibleName("端点");
    expect(screen.getByText("配置模型登录、API 端点和新会话默认路由策略。")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "远程仓库" }));
    expect(screen.getByRole("tab", { selected: true })).toHaveAccessibleName("远程仓库");
    expect(screen.getByText("配置代码交付使用的 GitHub/GitLab 凭据或复用 GitHub CLI。") ).toBeInTheDocument();
  });

});
