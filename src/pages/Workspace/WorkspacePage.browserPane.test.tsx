// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unlisten: vi.fn(),
  closeBrowserSession: vi.fn(),
}));

const taskState = vi.hoisted(() => ({
  tasks: {} as Record<string, Array<Record<string, unknown>>>,
  running: {} as Record<string, boolean>,
  loading: {} as Record<string, boolean>,
  error: {} as Record<string, string | null>,
  executionLog: {} as Record<string, unknown[]>,
  loadTasks: vi.fn(async () => {}),
  subscribe: vi.fn(async () => () => {}),
  start: vi.fn(async () => {}),
  cancel: vi.fn(async () => {}),
  retryFailedTasks: vi.fn(async () => 0),
  retryTasks: vi.fn(async () => 0),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));
vi.mock("../../lib/tauri", async (orig) => ({
  ...((await orig()) as object),
  invoke: mocks.invoke,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

vi.mock("../../stores/tasks", () => ({
  useTasksStore: (selector?: (s: Record<string, unknown>) => unknown) => {
    return selector ? selector(taskState) : taskState;
  },
}));
vi.mock("../../stores/settings", () => ({
  useSettingsStore: (selector?: (s: Record<string, unknown>) => unknown) => {
    const state = {
      settings: { theme: "dark", permissions: { full_access: false } },
      setTheme: vi.fn(),
      save: vi.fn(),
    };
    return selector ? selector(state) : state;
  },
}));
vi.mock("../../stores/git", () => ({
  useGitStore: (selector?: (s: Record<string, unknown>) => unknown) => {
    const state = { status: null, load: vi.fn() };
    return selector ? selector(state) : state;
  },
}));
vi.mock("../../components/ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("../../components/ReasoningEffortPicker", () => ({ ReasoningEffortPicker: () => null }));
vi.mock("../../components/GitStatusBar", () => ({
  GitStatusBar: ({ onOpenChanges, detailsId, detailsOpen }: { onOpenChanges: () => void; detailsId?: string; detailsOpen?: boolean }) => (
    <button type="button" aria-label="打开本地 Git" aria-controls={detailsId} aria-expanded={detailsOpen} onClick={onOpenChanges}>Git</button>
  ),
}));
vi.mock("../../components/GitChangesPanel", () => ({
  GitChangesPanel: ({ onClose, onOpenHistory, onOpenRemote }: { onClose: () => void; onOpenHistory: () => void; onOpenRemote: () => void }) => (
    <div data-testid="git-auxiliary-content">
      本地 Git 详情
      <button type="button" onClick={onOpenHistory}>历史</button>
      <button type="button" onClick={onOpenRemote}>远程</button>
      <button data-auxiliary-initial-focus type="button" aria-label="关闭本地 Git" onClick={onClose}>关闭</button>
    </div>
  ),
}));
vi.mock("../../components/GitHistoryPanel", () => ({
  GitHistoryPanel: ({ onClose }: { onClose: () => void }) => <div data-testid="history-auxiliary-content">提交历史详情<button data-auxiliary-initial-focus type="button" aria-label="返回本地 Git" onClick={onClose}>返回</button></div>,
}));
vi.mock("../../components/RemoteGitPanel", () => ({
  RemoteGitPanel: ({ onClose }: { onClose: () => void }) => <div data-testid="remote-auxiliary-content">远程仓库详情<button data-auxiliary-initial-focus type="button" aria-label="返回本地 Git" onClick={onClose}>返回</button></div>,
}));
vi.mock("../../components/WorkspaceDeliveryStatus", () => ({
  WorkspaceDeliveryStatus: ({ onOpenDetails, detailsOpen, detailsId }: { onOpenDetails?: () => void; detailsOpen?: boolean; detailsId?: string }) => (
    <button type="button" aria-label="打开交付详情" aria-expanded={detailsOpen} aria-controls={detailsId} onClick={() => onOpenDetails?.()}>交付</button>
  ),
}));
vi.mock("../../components/WelcomeUsageCard", () => ({ WelcomeUsageCard: () => null }));
vi.mock("../../components/MessageList", () => ({
  MessageList: ({
    onOpenDocument,
    onOpenEvidence,
  }: {
    onOpenDocument?: (path: string) => void;
    onOpenEvidence?: (evidenceId: string) => void;
  }) => (
    <div>
      <button type="button" onClick={() => onOpenDocument?.("/code/browser/report.md")}>打开测试文档</button>
      <button type="button" onClick={() => onOpenEvidence?.("turn-evidence-1")}>查看回合证据</button>
    </div>
  ),
}));
vi.mock("../../components/MessageInput", () => ({ MessageInput: () => <input aria-label="任务输入" /> }));
vi.mock("../../components/ContextUsageBar", () => ({ ContextUsageBar: () => null }));
vi.mock("../../components/PermissionDialog", () => ({
  PermissionDialog: () => <div role="dialog" aria-label="需要权限">权限确认</div>,
}));
vi.mock("../../components/QueueBadge", () => ({ QueueBadge: () => null }));

import { TurnEvidencePane, WorkspacePage } from "./WorkspacePage";
import { useChatStore, type SessionRuntime } from "../../stores/chat";

const projectSession = {
  id: "s-browser",
  title: "浏览器任务",
  cwd: "/code/browser",
  model_id: "m",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project" as const,
};

function runtimeFixture(): SessionRuntime {
  return {
    messages: [],
    persistedMessages: [],
    persistedPlans: [],
    historyBeforeRowid: null,
    hasOlderHistory: false,
    loadingOlderHistory: false,
    historyTruncated: false,
    revision: 0,
    historyRequestId: 0,
    localMessages: [],
    streaming: false,
    queue: [],
    pendingPermission: null,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    contextUsage: null,
    compressionToast: null,
  };
}

function seedActiveSession() {
  useChatStore.setState({
    sessions: [projectSession],
    activeSession: projectSession,
    draftSession: null,
    runtime: {
      [projectSession.id]: runtimeFixture(),
    },
    activeModel: "m",
    _unlisten: {},
    _unlistenSessionUpdated: {},
    _streamingMsgId: {},
  });
}

function renderWorkspace() {
  return render(
    <WorkspacePage
      sessionId="s-browser"
      onNewConversation={() => {}}
      onOpenSettings={() => {}}
      onOpenSession={() => {}}
    />,
  );
}

function setViewportWidth(width: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn((query: string) => {
      const min = query.match(/min-width:\s*(\d+)px/)?.[1];
      const max = query.match(/max-width:\s*(\d+)px/)?.[1];
      const matches = (!min || width >= Number(min)) && (!max || width <= Number(max));
      return {
        matches,
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      };
    }),
  });
}

function activeBrowserSession() {
  return {
    session_id: "codefactory-s-browser-1",
    owner_session_id: "s-browser",
    task_id: null,
    updated_at_unix_secs: 123,
    expired: false,
    status: "active",
    pane_url: "https://example.com/research",
    current_host: "example.com",
    page_title: "Example Research",
  };
}

describe("Workspace on-demand embedded browser pane", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
    taskState.tasks = {};
    taskState.running = {};
    taskState.loading = {};
    taskState.error = {};
    taskState.executionLog = {};
    taskState.loadTasks.mockClear();
    taskState.subscribe.mockClear();
    taskState.start.mockClear();
    taskState.cancel.mockClear();
    taskState.retryFailedTasks.mockClear();
    taskState.retryTasks.mockClear();
    setViewportWidth(1440);
    mocks.listen.mockResolvedValue(mocks.unlisten);
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([]);
      if (cmd === "close_browser_session") return Promise.resolve(undefined);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_session") return Promise.resolve(projectSession);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    seedActiveSession();
  });

  it("does not render a browser pane when the current session has no managed browser", async () => {
    renderWorkspace();

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));
    expect(screen.getByRole("main", { name: "会话窗口" })).toBeInTheDocument();
    expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /浏览器入口|打开浏览器/ })).not.toBeInTheDocument();
  });

  it("does not reserve an empty auxiliary pane for a stale task deep link", async () => {
    taskState.tasks = { "s-browser": [] };
    render(
      <WorkspacePage
        sessionId="s-browser"
        initialTaskLogId="missing-task"
        onNewConversation={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />,
    );

    await waitFor(() => expect(taskState.loadTasks).toHaveBeenCalledWith("s-browser"));
    await waitFor(() => expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument());
    expect(screen.queryByText(/不会保留空白占位/)).not.toBeInTheDocument();
  });

  it("distinguishes task loading and failure from a loaded empty task list", async () => {
    taskState.loading = { "s-browser": true };
    const view = render(
      <WorkspacePage
        sessionId="s-browser"
        initialTaskLogId="task-loading"
        onNewConversation={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />,
    );

    let pane = await screen.findByTestId("workspace-auxiliary-pane");
    expect(within(pane).getByRole("status")).toHaveTextContent("正在加载任务活动");
    expect(within(pane).queryByText("暂无任务活动")).not.toBeInTheDocument();

    taskState.loading = { "s-browser": false };
    taskState.error = { "s-browser": "任务服务暂时不可用" };
    view.rerender(
      <WorkspacePage
        sessionId="s-browser"
        initialTaskLogId="task-loading"
        onNewConversation={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />,
    );

    pane = screen.getByTestId("workspace-auxiliary-pane");
    expect(within(pane).getByRole("alert")).toHaveTextContent("任务服务暂时不可用");
    await userEvent.click(within(pane).getByRole("button", { name: "重试加载任务" }));
    expect(taskState.loadTasks).toHaveBeenLastCalledWith("s-browser");
  });

  it("only exposes aria-controls for the trigger that owns the mounted auxiliary pane", async () => {
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));

    const gitTrigger = screen.getByRole("button", { name: "打开本地 Git" });
    const deliveryTrigger = screen.getByRole("button", { name: "打开交付详情" });
    expect(gitTrigger).not.toHaveAttribute("aria-controls");
    expect(deliveryTrigger).not.toHaveAttribute("aria-controls");

    await user.click(gitTrigger);
    expect(gitTrigger).toHaveAttribute("aria-controls", "workspace-auxiliary-pane");
    expect(deliveryTrigger).not.toHaveAttribute("aria-controls");

    await user.click(deliveryTrigger);
    expect(gitTrigger).not.toHaveAttribute("aria-controls");
    expect(deliveryTrigger).toHaveAttribute("aria-controls", "workspace-auxiliary-pane");
  });

  it("does not place a browser refresh error above an active document", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.reject(new Error("browser daemon unavailable"));
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      if (cmd === "read_document") return Promise.resolve({ path: "/code/browser/report.md", relative_path: "report.md", name: "report.md", extension: "md", content: "# Report", truncated: false });
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();
    await user.click(screen.getByRole("button", { name: "打开测试文档" }));

    const pane = await screen.findByTestId("workspace-auxiliary-pane");
    expect(within(pane).getByRole("tabpanel")).toHaveTextContent("Report");
    expect(within(pane).queryByText(/浏览器状态读取失败/)).not.toBeInTheDocument();
  });

  it("opens the right embedded browser pane only for the active session, mounts a native webview, and can close it", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") {
        return Promise.resolve([
          activeBrowserSession(),
          {
            session_id: "codefactory-other-1",
            owner_session_id: "other-session",
            task_id: null,
            updated_at_unix_secs: 124,
            expired: false,
            status: "active",
            pane_url: "https://other.example/",
            current_host: "other.example",
          },
        ]);
      }
      if (cmd === "close_browser_session") return Promise.resolve(undefined);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });

    renderWorkspace();

    Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ left: 900, top: 120, width: 480, height: 640, right: 1380, bottom: 760, x: 900, y: 120, toJSON: () => ({}) }),
    });

    const pane = await screen.findByTestId("workspace-auxiliary-pane");
    expect(pane).toHaveAccessibleName("辅助工作区");
    expect(pane).toHaveAttribute("data-layout", "dock");
    expect(pane).toHaveAttribute("data-pane-kind", "browser");
    expect(within(pane).getByText("example.com")).toBeInTheDocument();
    expect(within(pane).queryByText("other.example")).not.toBeInTheDocument();
    expect(screen.getByRole("main", { name: "会话窗口" })).toHaveAttribute("data-auxiliary-pane", "open");
    expect(within(pane).getByRole("application", { name: "网页视图：Example Research" })).toBeInTheDocument();
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("embedded_browser_mount", {
        sessionId: "codefactory-s-browser-1",
        url: "https://example.com/research",
        bounds: { x: 900, y: 120, width: 480, height: 640 },
      });
    });

    const endBrowser = within(pane).getByRole("button", { name: "结束浏览器" });
    expect(endBrowser).toHaveClass("min-w-11", "lg:min-w-9");
    await userEvent.click(endBrowser);
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("close_browser_session", { sessionId: "codefactory-s-browser-1" });
    });
  });

  it("does not steal composer focus when browser polling discovers a passive session", async () => {
    setViewportWidth(1366);
    let resolveBrowserSessions!: (sessions: ReturnType<typeof activeBrowserSession>[]) => void;
    const browserSessions = new Promise<ReturnType<typeof activeBrowserSession>[]>((resolve) => {
      resolveBrowserSessions = resolve;
    });
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return browserSessions;
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });

    renderWorkspace();
    const composer = screen.getByRole("textbox", { name: "任务输入" });
    composer.focus();
    expect(composer).toHaveFocus();
    act(() => resolveBrowserSessions([activeBrowserSession()]));

    await screen.findByTestId("workspace-auxiliary-pane");
    expect(composer).toHaveFocus();
  });

  it("stops retrying a failed native mount until the user explicitly retries", async () => {
    let mountAttempts = 0;
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "embedded_browser_mount") {
        mountAttempts += 1;
        return mountAttempts === 1
          ? Promise.reject(new Error("native child unavailable"))
          : Promise.resolve(undefined);
      }
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ left: 900, top: 120, width: 480, height: 640, right: 1380, bottom: 760, x: 900, y: 120, toJSON: () => ({}) }),
    });

    const user = userEvent.setup();
    renderWorkspace();
    expect(await screen.findByRole("alert")).toHaveTextContent("内置浏览器打开失败");
    expect(mountAttempts).toBe(1);

    await user.click(screen.getByRole("button", { name: "重试内置浏览器" }));
    await waitFor(() => expect(mountAttempts).toBe(2));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it.each([
    [1440, "dock", "complementary", null],
    [1366, "drawer", "dialog", "false"],
    [800, "overlay", "dialog", "true"],
  ] as const)(
    "uses the %s px auxiliary layout without leaving a permanently reserved blank column",
    async (width, layout, role, modal) => {
      setViewportWidth(width);
      mocks.invoke.mockImplementation((cmd: string) => {
        if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
        if (cmd === "list_sessions") return Promise.resolve([projectSession]);
        if (cmd === "get_message_page") {
          return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
        }
        if (cmd === "is_chat_running") return Promise.resolve(false);
        return Promise.resolve(null);
      });

      renderWorkspace();

      const pane = await screen.findByTestId("workspace-auxiliary-pane");
      expect(pane).toHaveAttribute("data-layout", layout);
      expect(pane).toHaveRole(role);
      if (modal) expect(pane).toHaveAttribute("aria-modal", modal);
      else expect(pane).not.toHaveAttribute("aria-modal");
      expect(screen.getAllByTestId("workspace-auxiliary-pane")).toHaveLength(1);
    },
  );

  it("switches browser, Git, and task details inside one auxiliary pane instead of stacking drawers", async () => {
    taskState.tasks = {
      "s-browser": [{
        id: "task-1",
        session_id: "s-browser",
        title: "验证右侧工作区",
        status: "running",
        ordinal: 0,
        created_at: "2026-08-11T00:00:00Z",
        updated_at: "2026-08-11T00:00:00Z",
      }],
    };
    taskState.running = { "s-browser": true };
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();

    expect(await screen.findByTestId("workspace-auxiliary-pane")).toHaveAttribute("data-pane-kind", "browser");
    await user.click(screen.getByRole("button", { name: "打开本地 Git" }));
    let panes = screen.getAllByTestId("workspace-auxiliary-pane");
    expect(panes).toHaveLength(1);
    expect(panes[0]).toHaveAttribute("data-pane-kind", "git");
    expect(within(panes[0]).getByTestId("git-auxiliary-content")).toBeInTheDocument();
    expect(within(panes[0]).queryByRole("application", { name: /网页视图/ })).not.toBeInTheDocument();
    const gitTrigger = screen.getByRole("button", { name: "打开本地 Git" });
    expect(gitTrigger).toHaveAttribute("aria-controls", panes[0].id);
    expect(gitTrigger).toHaveAttribute("aria-expanded", "true");

    const taskTrigger = screen.getByRole("button", { name: /打开任务活动/ });
    await user.click(taskTrigger);
    panes = screen.getAllByTestId("workspace-auxiliary-pane");
    expect(panes).toHaveLength(1);
    expect(panes[0]).toHaveAttribute("data-pane-kind", "tasks");
    expect(taskTrigger).toHaveAttribute("aria-controls", panes[0].id);
    expect(taskTrigger).toHaveAttribute("aria-expanded", "true");
    expect(within(panes[0]).getByText("任务活动")).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "任务活动" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "打开本地 Git" }));
    expect(taskTrigger).toHaveAttribute("aria-expanded", "false");
  });

  it("keeps Git subviews inside the same pane, focuses them, and returns to local changes", async () => {
    setViewportWidth(1366);
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));

    const trigger = screen.getByRole("button", { name: "打开本地 Git" });
    await user.click(trigger);
    const pane = screen.getByTestId("workspace-auxiliary-pane");
    await user.click(within(pane).getByRole("button", { name: "历史" }));
    const historyBack = within(pane).getByRole("button", { name: "返回本地 Git" });
    await waitFor(() => expect(historyBack).toHaveFocus());
    await user.click(historyBack);
    expect(within(pane).getByTestId("git-auxiliary-content")).toBeInTheDocument();
    expect(screen.getByTestId("workspace-auxiliary-pane")).toBe(pane);

    await user.click(within(pane).getByRole("button", { name: "远程" }));
    const remoteBack = within(pane).getByRole("button", { name: "返回本地 Git" });
    await waitFor(() => expect(remoteBack).toHaveFocus());
    await user.click(remoteBack);
    expect(within(pane).getByTestId("git-auxiliary-content")).toBeInTheDocument();
  });

  it("lets a docked Git pane shrink below 640px without exposing a dead separator", async () => {
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));
    await user.click(screen.getByRole("button", { name: "打开本地 Git" }));

    const pane = screen.getByTestId("workspace-auxiliary-pane");
    const separator = within(pane).getByRole("separator", { name: "调整辅助工作区宽度" });
    const initial = Number(separator.getAttribute("aria-valuenow"));
    expect(initial).toBeLessThan(640);
    separator.focus();
    await user.keyboard("{ArrowLeft}");
    expect(Number(separator.getAttribute("aria-valuenow"))).toBeGreaterThan(initial);
  });

  it("moves focus into the new drawer content when switching auxiliary kinds", async () => {
    setViewportWidth(1366);
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();
    await screen.findByTestId("workspace-auxiliary-pane");

    await user.click(screen.getByRole("button", { name: "打开本地 Git" }));
    const close = screen.getByRole("button", { name: "关闭本地 Git" });
    await waitFor(() => expect(close).toHaveFocus());
  });

  it("closes task details without revealing a passive browser underneath", async () => {
    taskState.tasks = {
      "s-browser": [{
        id: "task-close",
        session_id: "s-browser",
        title: "关闭任务详情",
        status: "running",
        ordinal: 0,
        created_at: "2026-08-11T00:00:00Z",
        updated_at: "2026-08-11T00:00:00Z",
      }],
    };
    taskState.running = { "s-browser": true };
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();
    await screen.findByTestId("workspace-auxiliary-pane");

    const taskTrigger = screen.getByRole("button", { name: /打开任务活动/ });
    await user.click(taskTrigger);
    await user.click(screen.getByRole("button", { name: "关闭任务活动" }));
    expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument();
    await waitFor(() => expect(taskTrigger).toHaveFocus());
  });

  it("returns focus to the workspace when the task trigger disappears before closing", async () => {
    taskState.tasks = {
      "s-browser": [{
        id: "task-finishing",
        session_id: "s-browser",
        title: "即将完成",
        status: "running",
        ordinal: 0,
        created_at: "2026-08-11T00:00:00Z",
        updated_at: "2026-08-11T00:00:00Z",
      }],
    };
    taskState.running = { "s-browser": true };
    const user = userEvent.setup();
    const view = renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));
    await user.click(screen.getByRole("button", { name: /打开任务活动/ }));

    taskState.tasks["s-browser"][0].status = "completed";
    taskState.running = { "s-browser": false };
    view.rerender(
      <WorkspacePage
        sessionId="s-browser"
        onNewConversation={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />,
    );
    expect(screen.queryByRole("button", { name: /打开任务活动/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "关闭任务活动" }));
    await waitFor(() => expect(screen.getByRole("main", { name: "会话窗口" })).toHaveFocus());
  });

  it("routes a permission-blocked task back to the composer permission control", async () => {
    taskState.tasks = {
      "s-browser": [{
        id: "task-permission",
        session_id: "s-browser",
        title: "需要权限",
        status: "failed",
        ordinal: 0,
        failure_attribution: { kind: "permission", repairable: false, summary: "需要调整权限" },
        created_at: "2026-08-11T00:00:00Z",
        updated_at: "2026-08-11T00:00:00Z",
      }],
    };
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));

    await user.click(screen.getByRole("button", { name: /打开任务活动/ }));
    await user.click(screen.getByRole("button", { name: "调整会话权限" }));
    expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("combobox", { name: "会话权限" })).toHaveFocus());
  });

  it("keeps task verification controls touchable on narrow panes", async () => {
    taskState.tasks = {
      "s-browser": [{
        id: "task-verification",
        session_id: "s-browser",
        title: "验收任务",
        status: "failed",
        ordinal: 0,
        parent_task_id: null,
        verification_results: JSON.stringify([{ check: "pnpm test", passed: false, output: "1 failed", duration_ms: 1200 }]),
        created_at: "2026-08-11T00:00:00Z",
        updated_at: "2026-08-11T00:00:00Z",
      }],
    };
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));
    await user.click(screen.getByRole("button", { name: /打开任务活动/ }));

    const summary = screen.getByTitle(/验收验证/);
    expect(summary).toHaveClass("min-h-11", "lg:min-h-9");
    await user.click(summary);
    expect(screen.getByRole("button", { name: /pnpm test，查看验收输出/ })).toHaveClass(
      "min-h-11",
      "lg:min-h-9",
    );
  });

  it("keeps the session sidebar and auxiliary overlay mutually exclusive on a narrow viewport", async () => {
    setViewportWidth(375);
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));

    await user.click(screen.getByRole("button", { name: "展开会话侧栏" }));
    expect(screen.getByRole("complementary", { name: "会话列表" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "打开本地 Git" }));
    expect(screen.queryByRole("complementary", { name: "会话列表" })).not.toBeInTheDocument();
    expect(screen.getByTestId("workspace-auxiliary-pane")).toHaveAttribute("data-pane-kind", "git");

    await user.click(screen.getByRole("button", { name: "展开会话侧栏" }));
    expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "会话列表" })).toBeInTheDocument();
  });

  it("lets the foreground permission dialog own Escape instead of collapsing the auxiliary overlay", async () => {
    setViewportWidth(800);
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));
    await user.click(screen.getByRole("button", { name: "打开本地 Git" }));
    expect(screen.getByTestId("workspace-auxiliary-pane")).toHaveAttribute("data-layout", "overlay");

    act(() => {
      const current = useChatStore.getState().runtime[projectSession.id];
      useChatStore.setState({
        runtime: {
          ...useChatStore.getState().runtime,
          [projectSession.id]: {
            ...current,
            pendingPermission: { toolCallId: "tool-permission", toolName: "bash", args: { command: "pnpm test" } },
          },
        },
      });
    });

    expect(screen.getByRole("dialog", { name: "需要权限" })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.getByTestId("workspace-auxiliary-pane")).toBeInTheDocument();
  });

  it("collapses the browser overlay and restores focus when native child Escape is bridged", async () => {
    setViewportWidth(800);
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    renderWorkspace();

    const pane = await screen.findByTestId("workspace-auxiliary-pane");
    expect(pane).toHaveAttribute("data-layout", "overlay");
    await waitFor(() => {
      expect(mocks.listen).toHaveBeenCalledWith(
        "embedded-browser:escape",
        expect.any(Function),
      );
    });
    const escapeHandler = mocks.listen.mock.calls.find(
      ([eventName]) => eventName === "embedded-browser:escape",
    )?.[1] as ((event: { payload: { session_id: string } }) => void) | undefined;
    expect(escapeHandler).toBeTypeOf("function");

    act(() => escapeHandler?.({ payload: { session_id: "codefactory-other-1" } }));
    expect(screen.getByTestId("workspace-auxiliary-pane")).toBeInTheDocument();

    act(() => {
      const current = useChatStore.getState().runtime[projectSession.id];
      useChatStore.setState({
        runtime: {
          ...useChatStore.getState().runtime,
          [projectSession.id]: {
            ...current,
            pendingPermission: { toolCallId: "tool-native-escape", toolName: "bash", args: {} },
          },
        },
      });
    });
    act(() => escapeHandler?.({ payload: { session_id: "codefactory-s-browser-1" } }));
    expect(screen.getByTestId("workspace-auxiliary-pane")).toBeInTheDocument();

    act(() => {
      const current = useChatStore.getState().runtime[projectSession.id];
      useChatStore.setState({
        runtime: {
          ...useChatStore.getState().runtime,
          [projectSession.id]: { ...current, pendingPermission: null },
        },
      });
    });
    act(() => escapeHandler?.({ payload: { session_id: "codefactory-s-browser-1" } }));
    await waitFor(() => expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument());
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /恢复辅助工作区.*浏览器/ })).toHaveFocus();
    });
  });

  it("keeps the docked browser open when its native page handles Escape", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    renderWorkspace();

    const pane = await screen.findByTestId("workspace-auxiliary-pane");
    expect(pane).toHaveAttribute("data-layout", "dock");
    await waitFor(() => {
      expect(mocks.listen).toHaveBeenCalledWith(
        "embedded-browser:escape",
        expect.any(Function),
      );
    });
    const escapeHandler = mocks.listen.mock.calls.find(
      ([eventName]) => eventName === "embedded-browser:escape",
    )?.[1] as ((event: { payload: { session_id: string } }) => void) | undefined;

    act(() => escapeHandler?.({ payload: { session_id: "codefactory-s-browser-1" } }));

    expect(screen.getByTestId("workspace-auxiliary-pane")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /恢复辅助工作区.*浏览器/ })).not.toBeInTheDocument();
  });

  it("unmounts the native child fail-closed when permission-time hiding fails", async () => {
    mocks.invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "embedded_browser_set_visible" && args?.visible === false) {
        return Promise.reject(new Error("native hide failed"));
      }
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ left: 900, top: 120, width: 480, height: 640, right: 1380, bottom: 760, x: 900, y: 120, toJSON: () => ({}) }),
    });
    renderWorkspace();
    await screen.findByTestId("workspace-auxiliary-pane");
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("embedded_browser_mount", expect.any(Object)));

    act(() => {
      const current = useChatStore.getState().runtime[projectSession.id];
      useChatStore.setState({
        runtime: {
          ...useChatStore.getState().runtime,
          [projectSession.id]: {
            ...current,
            pendingPermission: { toolCallId: "tool-hide", toolName: "bash", args: { command: "pnpm test" } },
          },
        },
      });
    });

    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("embedded_browser_unmount", {
        sessionId: "codefactory-s-browser-1",
      });
    });
  });

  it("routes delivery and turn evidence into the same auxiliary arbiter", async () => {
    const runtime = useChatStore.getState().runtime[projectSession.id];
    useChatStore.setState({
      runtime: {
        ...useChatStore.getState().runtime,
        [projectSession.id]: {
          ...runtime,
          messages: [{ id: "turn-evidence-1", role: "assistant", content: "证据", createdAt: 1 }],
        },
      },
    });
    const user = userEvent.setup();
    renderWorkspace();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("list_browser_sessions"));

    const deliveryTrigger = screen.getByRole("button", { name: "打开交付详情" });
    await user.click(deliveryTrigger);
    let pane = screen.getByTestId("workspace-auxiliary-pane");
    expect(pane).toHaveAttribute("data-pane-kind", "delivery");
    expect(deliveryTrigger).toHaveAttribute("aria-controls", pane.id);
    expect(deliveryTrigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getAllByTestId("workspace-auxiliary-pane")).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "查看回合证据" }));
    pane = screen.getByTestId("workspace-auxiliary-pane");
    expect(pane).toHaveAttribute("data-pane-kind", "evidence");
    expect(screen.getAllByTestId("workspace-auxiliary-pane")).toHaveLength(1);
  });

  it("gives browser and document tabs a tabpanel relationship, roving tabindex, and arrow-key navigation", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();
    await screen.findByTestId("workspace-auxiliary-pane");

    await user.click(screen.getByRole("button", { name: "打开测试文档" }));
    const pane = screen.getByTestId("workspace-auxiliary-pane");
    const browserTab = within(pane).getByRole("tab", { name: /Example Research/ });
    const documentTab = within(pane).getByRole("tab", { name: /report\.md/ });
    const panel = within(pane).getByRole("tabpanel");
    expect(documentTab).toHaveAttribute("aria-selected", "true");
    expect(documentTab).toHaveAttribute("tabindex", "0");
    expect(browserTab).toHaveAttribute("tabindex", "-1");
    expect(documentTab).toHaveAttribute("aria-controls", panel.id);
    expect(panel).toHaveAttribute("aria-labelledby", documentTab.id);

    documentTab.focus();
    await user.keyboard("{ArrowLeft}");
    expect(browserTab).toHaveFocus();
    expect(browserTab).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowRight}");
    expect(documentTab).toHaveFocus();
    expect(documentTab).toHaveAttribute("aria-selected", "true");
  });

  it("restores a deliberately collapsed pane without keeping an empty white region", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();

    const pane = await screen.findByTestId("workspace-auxiliary-pane");
    const collapseButton = within(pane).getByRole("button", { name: /折叠辅助工作区/ });
    expect(collapseButton).toHaveClass("min-w-11", "lg:min-w-9");
    await user.click(collapseButton);
    expect(screen.queryByTestId("workspace-auxiliary-pane")).not.toBeInTheDocument();
    expect(screen.getByRole("main", { name: "会话窗口" })).toHaveAttribute("data-auxiliary-pane", "closed");

    const restore = screen.getByRole("button", { name: /恢复辅助工作区.*浏览器/ });
    await user.click(restore);
    const restoredPane = await screen.findByTestId("workspace-auxiliary-pane");
    expect(restoredPane).toHaveAttribute("data-pane-kind", "browser");
    const collapse = within(restoredPane).getByRole("button", { name: /折叠辅助工作区/ });
    await waitFor(() => expect(collapse).toHaveFocus());
    await user.click(collapse);
    await waitFor(() => expect(screen.getByRole("button", { name: /恢复辅助工作区.*浏览器/ })).toHaveFocus());
  });

  it("exposes a labelled keyboard-adjustable separator for a docked browser or document pane", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") return Promise.resolve([activeBrowserSession()]);
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_message_page") {
        return Promise.resolve({ messages: [], next_before_rowid: null, has_more: false, truncated: false });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    const user = userEvent.setup();
    renderWorkspace();

    const pane = await screen.findByTestId("workspace-auxiliary-pane");
    const separator = within(pane).getByRole("separator", { name: "调整辅助工作区宽度" });
    expect(separator).toHaveAttribute("aria-orientation", "vertical");
    expect(separator).toHaveAttribute("tabindex", "0");
    expect(separator).toHaveAttribute("aria-valuemin");
    expect(separator).toHaveAttribute("aria-valuemax");
    const initial = Number(separator.getAttribute("aria-valuenow"));

    separator.focus();
    await user.keyboard("{ArrowLeft}");
    expect(Number(separator.getAttribute("aria-valuenow"))).toBeGreaterThan(initial);
  });
});

describe("TurnEvidencePane", () => {
  it("shows actionable input summaries and preserves complete tool output", async () => {
    const user = userEvent.setup();
    render(
      <TurnEvidencePane
        evidenceId="assistant-evidence"
        onClose={() => {}}
        messages={[{
          id: "assistant-evidence",
          role: "assistant",
          content: "done",
          createdAt: 1,
          turnToolCallCount: 205,
          turnToolCalls: [{
            id: "tool-write",
            name: "write_file",
            args: JSON.stringify({ path: "/repo/src/App.tsx", content: "export default App" }),
            status: "done",
            result: "wrote /repo/src/App.tsx\nfull output line that must remain reviewable",
          }, {
            id: "tool-test",
            name: "bash",
            args: JSON.stringify({ command: "pnpm test -- App" }),
            status: "done",
            result: "PASS App.test.tsx\nTests: 12 passed",
          }],
        }]}
      />,
    );

    expect(screen.getByText("/repo/src/App.tsx")).toBeInTheDocument();
    expect(screen.getByText("pnpm test -- App")).toBeInTheDocument();
    expect(screen.getByText("仅显示最近 2/205 项操作")).toBeInTheDocument();
    const fullOutput = screen.getAllByText("完整输出")[0];
    await user.click(fullOutput);
    const output = screen.getByText(/full output line that must remain reviewable/);
    expect(output).not.toHaveClass("line-clamp-3");
  });
});
