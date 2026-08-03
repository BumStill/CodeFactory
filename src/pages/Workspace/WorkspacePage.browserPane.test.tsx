// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unlisten: vi.fn(),
  closeBrowserSession: vi.fn(),
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
    const state = {
      tasks: {},
      running: {},
      executionLog: {},
      loadTasks: vi.fn(async () => {}),
      subscribe: vi.fn(async () => () => {}),
    };
    return selector ? selector(state) : state;
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
vi.mock("../../components/GitStatusBar", () => ({ GitStatusBar: () => null }));
vi.mock("../../components/GitChangesPanel", () => ({ GitChangesPanel: () => null }));
vi.mock("../../components/GitHistoryPanel", () => ({ GitHistoryPanel: () => null }));
vi.mock("../../components/RemoteGitPanel", () => ({ RemoteGitPanel: () => null }));
vi.mock("../../components/WorkspaceDeliveryStatus", () => ({ WorkspaceDeliveryStatus: () => null }));
vi.mock("../../components/WelcomeUsageCard", () => ({ WelcomeUsageCard: () => null }));
vi.mock("../../components/MessageInput", () => ({ MessageInput: () => <div>输入框</div> }));
vi.mock("../../components/ContextUsageBar", () => ({ ContextUsageBar: () => null }));
vi.mock("../../components/PermissionDialog", () => ({ PermissionDialog: () => null }));
vi.mock("../../components/QueueBadge", () => ({ QueueBadge: () => null }));

import { WorkspacePage } from "./WorkspacePage";
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

describe("Workspace on-demand embedded browser pane", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
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
    expect(screen.queryByRole("complementary", { name: "内置浏览器" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /浏览器入口|打开浏览器/ })).not.toBeInTheDocument();
  });

  it("opens the right embedded browser pane only for the active session, mounts a native webview, and can close it", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_browser_sessions") {
        return Promise.resolve([
          {
            session_id: "codefactory-s-browser-1",
            owner_session_id: "s-browser",
            task_id: null,
            updated_at_unix_secs: 123,
            expired: false,
            status: "active",
            pane_url: "https://example.com/research",
            current_host: "example.com",
            page_title: "Example Research",
          },
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

    const pane = await screen.findByRole("complementary", { name: "内置浏览器" });
    expect(within(pane).getByText("example.com")).toBeInTheDocument();
    expect(within(pane).queryByText("other.example")).not.toBeInTheDocument();
    expect(pane).toHaveAttribute("data-browser-width", "38");
    expect(screen.getByRole("main", { name: "会话窗口" })).toHaveAttribute("data-browser-pane", "open");
    expect(within(pane).getByRole("application", { name: "网页视图：Example Research" })).toBeInTheDocument();
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("embedded_browser_mount", {
        sessionId: "codefactory-s-browser-1",
        url: "https://example.com/research",
        bounds: { x: 900, y: 120, width: 480, height: 640 },
      });
    });

    await userEvent.click(within(pane).getByRole("button", { name: "结束浏览器" }));
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("close_browser_session", { sessionId: "codefactory-s-browser-1" });
    });
  });
});
