// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { SettingsPage } from "./SettingsPage";
import type { UpdaterPhase } from "../../stores/updater";

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
  phase: { kind: "idle" } as UpdaterPhase,
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
  countUpdateBlockers: () => 1,
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
  // The Browser tab now also subscribes to Chromium download progress. This
  // test does not exercise the download, so a no-op unsubscriber is enough —
  // but it must exist, or the real Tauri listener runs and jsdom has no runtime.
  onChromiumProgress: () => Promise.resolve(() => {}),
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
    updaterState.phase = { kind: "idle" };
  });

  it("describes queued and observe-only updater states without claiming a download", () => {
    updaterState.phase = {
      kind: "waiting_for_safe_restart",
      update: { version: "1.81.13" },
      blockers: {
        update_install_state: "queued",
      },
      safetyCheckError: null,
      checkedAt: 1,
    } as UpdaterPhase;
    const { unmount } = render(<SettingsPage onBack={() => {}} initialTab="about" />);

    expect(screen.getByText("更新已排队，等待本地执行结束。")).toBeInTheDocument();
    expect(screen.getByText(/结束后自动下载、安装并重启/)).toBeInTheDocument();
    expect(screen.queryByText(/更新已下载/)).toBeNull();
    unmount();

    updaterState.phase = {
      kind: "waiting_for_safe_restart",
      update: { version: "1.81.13" },
      blockers: {
        update_install_state: "observe_only",
      },
      safetyCheckError: null,
      checkedAt: 1,
    } as UpdaterPhase;
    render(<SettingsPage onBack={() => {}} initialTab="about" />);

    expect(screen.getByText("正在核对上次安装结果…")).toBeInTheDocument();
    expect(screen.getByText(/不会重复安装未知结果/)).toBeInTheDocument();
    expect(screen.queryByText(/自动下载、安装并重启/)).toBeNull();
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

    // Wait on the fetched row, not on the heading. The heading renders
    // synchronously, so awaiting it settles listBrowserSessions() only as a
    // side effect of waitFor's act() flush — the assertions below would be
    // racing that flush rather than the fetch they actually depend on.
    expect(await screen.findByText("任务 task-123")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "浏览器会话" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/不会关闭你的普通 Chrome/)).toBeInTheDocument();
    expect(
      screen.getByText("chrome://inspect/#remote-debugging"),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "结束浏览器会话 codefactory-task-123",
      }),
    );

    // One settle for the whole close round trip instead of two sequential
    // polls. The empty state is gated on `!loading && sessions.length === 0`,
    // so it can only appear after closeBrowserSession() resolved AND the
    // refetch came back empty — waiting for the mock call first added a second
    // polling cycle (a real cost on a contended runner) and proved nothing the
    // empty state does not already prove.
    expect(
      await screen.findByText("当前没有活动的 CodeFactory 自动化浏览器。"),
    ).toBeInTheDocument();
    expect(mocks.closeBrowserSession).toHaveBeenCalledWith(
      "codefactory-task-123",
    );
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

  it("presents IM notifications as a one-click binding flow instead of a raw webhook form", async () => {
    await openGeneralTab();

    expect(screen.getByRole("heading", { name: "手机通知" })).toBeInTheDocument();
    expect(screen.getByText("任务完成、失败或等待你批准时，推送到企业微信或飞书。"))
      .toBeInTheDocument();
    expect(screen.getByRole("button", { name: "一键绑定 IM" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "高级：手动 Webhook" })).toBeInTheDocument();
    expect(screen.queryByLabelText("IM Webhook 地址")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "一键绑定 IM" }));

    expect(screen.getByRole("dialog", { name: "绑定 IM 通知" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "企业微信" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "飞书" })).toBeInTheDocument();
    expect(screen.getByText("CodeFactory 会先发送一条诊断消息，成功后才保存绑定。"))
      .toBeInTheDocument();
  });

  it("keeps manual IM webhooks as an advanced fallback, not the primary path", async () => {
    await openGeneralTab();

    fireEvent.click(screen.getByRole("button", { name: "高级：手动 Webhook" }));

    expect(screen.getByLabelText("IM Webhook 地址")).toBeInTheDocument();
    expect(screen.getByText("仅在无法一键绑定时使用。保存前请确认机器人已经在目标群内。"))
      .toBeInTheDocument();
  });

  it("shows provider-specific next steps after selecting an IM provider", async () => {
    await openGeneralTab();

    fireEvent.click(screen.getByRole("button", { name: "一键绑定 IM" }));
    fireEvent.click(screen.getByRole("button", { name: "企业微信" }));

    expect(screen.getByText("企业微信绑定步骤")).toBeInTheDocument();
    expect(screen.getByText("1. 在企业微信群里添加群机器人。"))
      .toBeInTheDocument();
    expect(screen.getByText("2. 复制机器人 Webhook，粘贴到下方。"))
      .toBeInTheDocument();
    expect(screen.getByText("3. CodeFactory 会发送诊断消息，成功后才保存。"))
      .toBeInTheDocument();
    expect(screen.getByLabelText("企业微信 Webhook 地址")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "飞书" }));

    expect(screen.getByText("飞书绑定步骤")).toBeInTheDocument();
    expect(screen.getByLabelText("飞书 Webhook 地址")).toBeInTheDocument();
  });

});
