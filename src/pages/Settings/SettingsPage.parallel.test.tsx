// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
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
  listBrowserSessions: vi.fn(),
  closeBrowserSession: vi.fn(),
  invoke: vi.fn(),
  loadRemotes: vi.fn(),
  addRemote: vi.fn(),
  deleteRemote: vi.fn(),
  testRemote: vi.fn(),
  updaterInitialize: vi.fn(),
  updaterCheckNow: vi.fn(),
  updaterInstall: vi.fn(),
  openProfile: vi.fn(),
  openEvolution: vi.fn(),
  openBenchmarks: vi.fn(),
  openResources: vi.fn(),
  openControlPlane: vi.fn(),
}));

// Intentionally omits max_parallel_tasks / subagent_isolation: a settings.json
// written before those fields existed must hydrate the form with defaults.
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
  listBrowserSessions: mocks.listBrowserSessions,
  closeBrowserSession: mocks.closeBrowserSession,
}));

async function openGeneralTab() {
  render(<SettingsPage onBack={() => {}} />);
  fireEvent.click(screen.getByText("通用"));
  await waitFor(() => {
    expect(screen.getByText("并行任务上限")).toBeInTheDocument();
  });
}

describe("SettingsPage parallel-task controls", () => {
  it("does not expose a global permissions tab", () => {
    render(<SettingsPage onBack={() => {}} />);

    expect(screen.queryByRole("button", { name: "权限" })).toBeNull();
    expect(screen.queryByText("工具权限")).toBeNull();
  });

  beforeEach(() => {
    for (const mock of Object.values(mocks)) {
      mock.mockReset();
    }
    mocks.load.mockResolvedValue(undefined);
    mocks.save.mockResolvedValue(undefined);
    mocks.codexAccount.mockResolvedValue(null);
    mocks.listBrowserSessions.mockResolvedValue([]);
    mocks.closeBrowserSession.mockResolvedValue(undefined);
    // DataSection on the 通用 tab fetches the data dir on mount.
    mocks.invoke.mockResolvedValue("");
  });

  it("makes moved workspace capabilities readable and reachable from settings", async () => {
    render(
      <SettingsPage
        onBack={() => {}}
        initialTab="capabilities"
        onOpenProfile={mocks.openProfile}
        onOpenEvolution={mocks.openEvolution}
        onOpenBenchmarks={mocks.openBenchmarks}
        onOpenResources={mocks.openResources}
        onOpenControlPlane={mocks.openControlPlane}
      />,
    );

    expect(await screen.findByRole("heading", { name: "功能" })).toBeInTheDocument();
    const routes = [
      ["我的画像", mocks.openProfile],
      ["进化审查", mocks.openEvolution],
      ["能力评测", mocks.openBenchmarks],
      ["资源中心", mocks.openResources],
      ["AI Coding OS", mocks.openControlPlane],
    ] as const;
    for (const [label, callback] of routes) {
      fireEvent.click(screen.getByRole("button", { name: new RegExp(label) }));
      expect(callback).toHaveBeenCalledTimes(1);
    }
  });

  it("keeps permission policy out of global settings", () => {
    render(<SettingsPage onBack={() => {}} />);

    expect(screen.queryByRole("button", { name: "权限" })).toBeNull();
    expect(screen.queryByText(/减少常规工具确认/)).toBeNull();
    expect(screen.queryByText(/每条消息都直接执行到交付物/)).toBeNull();
  });

  it("shows and closes only CodeFactory-managed browser sessions", async () => {
    mocks.listBrowserSessions
      .mockResolvedValueOnce([
        {
          session_id: "codefactory-task-123",
          task_id: "task-123",
          owner_session_id: "session-123",
          updated_at_unix_secs: 1_780_000_000,
          expired: false,
        },
      ])
      .mockResolvedValueOnce([]);

    render(<SettingsPage onBack={() => {}} initialTab="browser" />);

    expect(
      await screen.findByRole("heading", { name: "受管浏览器会话" }),
    ).toBeInTheDocument();
    expect(screen.getByText("任务 task-123")).toBeInTheDocument();
    expect(screen.getByText(/不会关闭你的普通 Chrome/)).toBeInTheDocument();
    expect(
      screen.getByText("chrome://inspect/#remote-debugging"),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "结束浏览器会话 codefactory-task-123",
      }),
    );

    await waitFor(() => {
      expect(mocks.closeBrowserSession).toHaveBeenCalledWith(
        "codefactory-task-123",
      );
    });
    expect(
      await screen.findByText("当前没有活动的 CodeFactory 自动化浏览器。"),
    ).toBeInTheDocument();
  });

  it("hydrates defaults for settings saved before the fields existed", async () => {
    await openGeneralTab();

    expect(screen.getByRole("spinbutton")).toHaveValue(3);
    expect(screen.getByDisplayValue("共享目录(默认)")).toBeInTheDocument();
    // Delivery ceiling: a legacy settings.json (no delivery_ceiling) hydrates to
    // A formal release artifact is not the same thing as a live-verifier pass.
    expect(screen.getByDisplayValue("…并创建正式发布(默认)")).toBeInTheDocument();
    expect(screen.queryByText(/默认一路合并、发布上线/)).not.toBeInTheDocument();
  });

  it("does not expose the unsupported Ultra orchestration label as a global effort", async () => {
    await openGeneralTab();

    const effort = screen.getByDisplayValue("中");
    expect(effort).toHaveTextContent("最大");
    expect(effort).not.toHaveTextContent("极致");
  });

  it("saves the selected delivery ceiling", async () => {
    await openGeneralTab();

    fireEvent.change(screen.getByDisplayValue("…并创建正式发布(默认)"), {
      target: { value: "pr_only" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(mocks.save).toHaveBeenCalledTimes(1);
    });
    expect(mocks.save.mock.calls[0][0].delivery_ceiling).toBe("pr_only");
    expect(mocks.save.mock.calls[0][0].delivery_ceiling_explicit).toBe(true);
  });

  it("saves the edited parallelism cap and isolation mode", async () => {
    await openGeneralTab();

    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "5" } });
    fireEvent.change(screen.getByDisplayValue("共享目录(默认)"), {
      target: { value: "worktree" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(mocks.save).toHaveBeenCalledTimes(1);
    });
    const saved = mocks.save.mock.calls[0][0];
    expect(saved.max_parallel_tasks).toBe(5);
    expect(saved.subagent_isolation).toBe("worktree");
  });

  it("clamps an out-of-range cap into 1..=8 on save", async () => {
    await openGeneralTab();

    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "42" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(mocks.save).toHaveBeenCalledTimes(1);
    });
    expect(mocks.save.mock.calls[0][0].max_parallel_tasks).toBe(8);
  });
});
