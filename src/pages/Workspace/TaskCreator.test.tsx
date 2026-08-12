// SPDX-License-Identifier: Apache-2.0
// Regression tests for session-native task delegation. Decomposition has no
// standalone product surface: the chat agent delegates internally and the
// conversation reveals execution detail only after tasks exist.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  loadTasks: vi.fn().mockResolvedValue(undefined),
  subscribe: vi.fn().mockResolvedValue(() => {}),
  start: vi.fn().mockResolvedValue(undefined),
  cancel: vi.fn().mockResolvedValue(undefined),
  retryFailedTasks: vi.fn().mockResolvedValue(1),
  retryTasks: vi.fn().mockResolvedValue(true),
}));

vi.mock("../../lib/tauri", async (original) => {
  const real = (await original()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const fakeChatState = {
  sessions: [] as unknown[],
  activeSession: { id: "s1", kind: "project", cwd: "/Users/x/proj", title: "proj" },
  activeDraft: null,
  messages: [] as unknown[],
  streaming: false,
  queue: [] as unknown[],
  activeModel: "anthropic/claude-opus-4-7",
  selectSession: vi.fn(),
  sendMessage: vi.fn(),
  sendOrQueue: vi.fn(),
  removeFromQueue: vi.fn(),
  cancelStream: vi.fn(),
  pendingPermission: null,
  respondPermission: vi.fn(),
  updateActiveSessionModel: vi.fn(),
  createSession: vi.fn(),
  loadSessions: vi.fn(),
  inputTokenTotal: 0,
  outputTokenTotal: 0,
};
const fakeChatRuntime = {
  messages: fakeChatState.messages,
  streaming: fakeChatState.streaming,
  queue: fakeChatState.queue,
  inputTokenTotal: 0,
  outputTokenTotal: 0,
  pendingPermission: null,
  contextUsage: null,
  compressionToast: null,
};
vi.mock("../../stores/chat", () => ({
  useChatStore: Object.assign(
    <T,>(selector?: (state: typeof fakeChatState) => T): T | typeof fakeChatState =>
      selector ? selector(fakeChatState) : fakeChatState,
    { setState: vi.fn(), getState: () => fakeChatState },
  ),
  activeRuntime: () => fakeChatRuntime,
}));

vi.mock("../../components/ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("../../components/GitStatusBar", () => ({
  GitStatusBar: () => <button aria-label="Git 状态">Git</button>,
}));
vi.mock("../../components/CheckpointsPanel", () => ({ CheckpointsPanel: () => null }));
vi.mock("../../components/WorkspaceDeliveryStatus", () => ({ WorkspaceDeliveryStatus: () => null }));
vi.mock("../../components/MessageList", () => ({ MessageList: () => null }));
vi.mock("../../components/MessageInput", () => ({
  MessageInput: ({ pendingInsert }: { pendingInsert?: string }) => pendingInsert ? <div data-testid="pending-repair-prompt">{pendingInsert}</div> : null,
}));
vi.mock("../../components/PermissionDialog", () => ({ PermissionDialog: () => null }));
vi.mock("../../components/ContextUsageBar", () => ({ ContextUsageBar: () => null }));
vi.mock("../../stores/settings", () => ({
  useSettingsStore: Object.assign(
    () => ({
      settings: { theme: "dark", permissions: { full_access: false } },
      setTheme: vi.fn(),
    }),
    { getState: () => ({ settings: { remote_postmortem_enabled: false } }) },
  ),
}));

const fakeTasksState = {
  tasks: {} as Record<string, unknown[]>,
  running: {} as Record<string, boolean>,
  executionLog: {} as Record<string, unknown[]>,
  loadTasks: mocks.loadTasks,
  subscribe: mocks.subscribe,
  subscribeEvidence: vi.fn().mockResolvedValue(() => {}),
  retryFailedTasks: mocks.retryFailedTasks,
  retryTasks: mocks.retryTasks,
  start: mocks.start,
  cancel: mocks.cancel,
};
vi.mock("../../stores/tasks", () => ({
  useTasksStore: Object.assign(
    <T,>(selector?: (state: typeof fakeTasksState) => T): T | typeof fakeTasksState =>
      selector ? selector(fakeTasksState) : fakeTasksState,
    { setState: vi.fn(), getState: () => fakeTasksState },
  ),
}));

const fakeLearningState = {
  events: {} as Record<string, unknown[]>,
  loading: {} as Record<string, boolean>,
  load: vi.fn(async () => []),
  subscribe: vi.fn(async () => () => {}),
  accept: vi.fn(async () => {}),
  reject: vi.fn(async () => {}),
};
vi.mock("../../stores/learning", () => ({
  useLearningStore: Object.assign(
    <T,>(selector?: (state: typeof fakeLearningState) => T): T | typeof fakeLearningState =>
      selector ? selector(fakeLearningState) : fakeLearningState,
    { setState: vi.fn(), getState: () => fakeLearningState },
  ),
}));
vi.mock("../../stores/skills", () => ({
  useSkillsStore: () => ({ skills: [], loadSkills: vi.fn() }),
}));

import { WorkspacePage } from "./WorkspacePage";

const renderWorkspace = (onOpenSettings = vi.fn()) =>
  ({
    onOpenSettings,
    ...render(
      <WorkspacePage
        sessionId="s1"
        onNewConversation={() => {}}
        onOpenSettings={onOpenSettings}
        onOpenSession={() => {}}
      />,
    ),
  });

function task(overrides: Record<string, unknown> = {}) {
  return {
    id: "task-1",
    session_id: "s1",
    title: "实现登录页",
    description: "Implement the login page",
    status: "pending",
    cwd: "/Users/x/proj",
    parent_task_id: null,
    sub_session_id: null,
    created_at: "2026-07-08T00:00:00Z",
    started_at: null,
    completed_at: null,
    result: null,
    error: null,
    attempt_count: 0,
    verification_results: null,
    failure_attribution: null,
    task_context_json: null,
    spec_req_id: null,
    spec_title: null,
    ...overrides,
  };
}

describe("session-native task delegation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.loadTasks.mockResolvedValue(undefined);
    mocks.subscribe.mockResolvedValue(() => {});
    mocks.start.mockResolvedValue(undefined);
    mocks.cancel.mockResolvedValue(undefined);
    mocks.retryFailedTasks.mockResolvedValue(1);
    mocks.retryTasks.mockResolvedValue(true);
    fakeTasksState.tasks = {};
    fakeTasksState.running = {};
    fakeTasksState.executionLog = {};
    fakeLearningState.events = {};
  });

  it("has no standalone spec, plan, decomposition, or empty task surface", async () => {
    fakeTasksState.tasks = { s1: [] };
    renderWorkspace();

    expect(screen.queryByTitle(/规范工作台/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /规范|计划/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "AI 拆解项目任务" })).not.toBeInTheDocument();
    expect(screen.queryByText("拆任务")).not.toBeInTheDocument();
    expect(screen.queryByText("会话执行详情")).not.toBeInTheDocument();
    expect(screen.queryByText(/审核并确认任务/)).not.toBeInTheDocument();
    await waitFor(() => expect(mocks.loadTasks).toHaveBeenCalledWith("s1"));
    expect(mocks.subscribe).toHaveBeenCalledWith("s1");
  });

  it("keeps only session-critical actions in the workspace header", () => {
    fakeTasksState.tasks = { s1: [] };
    renderWorkspace();

    const header = screen.getByRole("banner", { name: "会话工具栏" });
    const sessionRail = screen.getByRole("complementary", { name: "会话列表" });
    expect(within(header).queryByRole("button", { name: "收起会话侧栏" })).not.toBeInTheDocument();
    expect(within(sessionRail).getByRole("button", { name: "收起会话侧栏" })).toBeInTheDocument();
    expect(within(header).queryByRole("button", { name: "新建空白会话" })).not.toBeInTheDocument();
    expect(within(header).getByRole("button", { name: "Git 状态" })).toBeInTheDocument();
    expect(within(header).queryByRole("button", { name: /检查点|恢复/ })).not.toBeInTheDocument();
    expect(within(header).getByRole("button", { name: "设置" })).toBeInTheDocument();
    for (const label of ["我的画像", "进化审查", "能力评测", "资源中心", "AI Coding OS"]) {
      expect(within(header).queryByRole("button", { name: label })).not.toBeInTheDocument();
    }
    expect(within(header).queryByRole("group", { name: "主题" })).not.toBeInTheDocument();
  });

  it("surfaces pending delegated work without placing the task tree in the conversation", async () => {
    fakeTasksState.tasks = { s1: [task()] };
    renderWorkspace();

    const conversation = screen.getByRole("main", { name: "会话窗口" });
    expect(within(conversation).queryByText("会话执行详情")).not.toBeInTheDocument();
    expect(within(conversation).queryByText("实现登录页")).not.toBeInTheDocument();
    const activity = screen.getByRole("button", { name: "打开任务活动：1 个后台任务等待调度" });
    expect(activity).toHaveTextContent("1");
    expect(activity).not.toHaveTextContent("待执行");
    expect(activity).toHaveAttribute("title", "1 个后台任务等待调度");
  });

  it("opens task activity for running delegated execution and keeps it out of the conversation", async () => {
    fakeTasksState.tasks = { s1: [task({ status: "running" })] };
    fakeTasksState.running = { s1: true };
    renderWorkspace();

    const conversation = screen.getByRole("main", { name: "会话窗口" });
    expect(within(conversation).queryByText("会话执行详情")).not.toBeInTheDocument();
    expect(within(conversation).queryByText("实现登录页")).not.toBeInTheDocument();

    const activity = screen.getByRole("button", { name: "打开任务活动：1 个后台任务正在运行" });
    expect(activity).toHaveTextContent("1");
    expect(activity).not.toHaveTextContent("正在执行");
    expect(activity.querySelector(".animate-spin")).toBeInTheDocument();
    expect(activity).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(activity);
    const drawer = screen.getByRole("dialog", { name: "任务活动" });
    await waitFor(() => expect(screen.getByRole("button", { name: "关闭任务活动" })).toHaveFocus());
    expect(activity).toHaveAttribute("aria-expanded", "true");
    expect(within(drawer).getByText("实现登录页")).toBeInTheDocument();
    expect(screen.queryByText("会话执行详情")).not.toBeInTheDocument();

    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "任务活动" })).not.toBeInTheDocument();
    await waitFor(() => expect(activity).toHaveFocus());
    expect(activity).toHaveAttribute("aria-expanded", "false");
  });

  it("turns the session rail into a dismissible overlay at narrow or 200% zoom widths", async () => {
    const originalMatchMedia = window.matchMedia;
    const media = {
      matches: true,
      media: "(max-width: 720px)",
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    };
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => media),
    });

    const rendered = renderWorkspace();
    expect(screen.queryByRole("complementary", { name: "会话列表" })).not.toBeInTheDocument();
    const toggle = screen.getByRole("button", { name: "展开会话侧栏" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(toggle);
    const sessionRail = screen.getByRole("complementary", { name: "会话列表" });
    expect(sessionRail).toBeInTheDocument();
    expect(within(sessionRail).getByRole("button", { name: "关闭会话侧栏" })).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "会话窗口" })).toBeInTheDocument();

    await userEvent.click(within(sessionRail).getByRole("button", { name: "关闭会话侧栏" }));
    expect(screen.queryByRole("complementary", { name: "会话列表" })).not.toBeInTheDocument();
    rendered.unmount();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: originalMatchMedia,
    });
  });

  it("keeps legacy spec provenance visible without reopening a spec product surface", async () => {
    fakeTasksState.tasks = {
      s1: [task({ status: "running", spec_req_id: "CF-010", spec_title: "Token 成本仪表盘" })],
    };
    fakeTasksState.running = { s1: true };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    expect(screen.getByText("规范《Token 成本仪表盘》")).toBeInTheDocument();
    expect(screen.queryByTitle(/规范工作台/)).not.toBeInTheDocument();
  });

  it("shows pending-only tasks as informational activity without inventing a manual start action", async () => {
    fakeTasksState.tasks = { s1: [task()] };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    expect(screen.getByText("已委派，还有 1 项等待后台调度。" )).toBeInTheDocument();
    expect(screen.getByText("任务已委派，系统会持续调度并自动诊断恢复，无需手动重试。")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /开始|继续执行/ })).not.toBeInTheDocument();
  });

  it("does not offer a generic continue action while a failure blocks pending work", async () => {
    fakeTasksState.tasks = {
      s1: [
        task({ id: "pending-after-failure" }),
        task({
          id: "provider-blocker",
          status: "failed",
          error: "API key not found",
          failure_attribution: {
            kind: "model-provider",
            label: "模型/Provider",
            summary: "API key not found",
            next_action: "打开模型设置后重试。",
            repairable: false,
            source: "error",
          },
        }),
      ],
    };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    expect(screen.queryByRole("button", { name: /继续执行/ })).not.toBeInTheDocument();
    expect(
      screen.getByText("系统正在处理失败项，并会自动续接剩余 1 项。"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开模型设置" })).toBeEnabled();
  });

  it("stops a running delegated execution", async () => {
    fakeTasksState.tasks = { s1: [task({ status: "running" })] };
    fakeTasksState.running = { s1: true };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    await userEvent.click(screen.getByRole("button", { name: "停止" }));
    expect(mocks.cancel).toHaveBeenCalledWith("s1");
  });

  it("keeps repairable technical failures system-owned without a manual retry action", async () => {
    fakeTasksState.tasks = {
      s1: [
        task({
          status: "failed",
          error: "npm test failed",
          failure_attribution: {
            kind: "verification",
            label: "验收失败",
            summary: "npm test 验收未通过",
            next_action: "修实现并重跑同一检查。",
            repairable: true,
            source: "verification_results",
          },
        }),
      ],
    };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    expect(screen.queryByRole("button", { name: "重试失败步骤" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /已修复，重试/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /回到对话处理/ })).not.toBeInTheDocument();
    expect(screen.queryByTestId("pending-repair-prompt")).not.toBeInTheDocument();
    expect(mocks.retryFailedTasks).not.toHaveBeenCalled();
    expect(mocks.retryTasks).not.toHaveBeenCalled();
    expect(mocks.start).not.toHaveBeenCalled();
  });

  it("uses model settings only for required provider input and resumes without a retry CTA", async () => {
    fakeTasksState.tasks = {
      s1: [
        task({
          status: "failed",
          error: "API key not found for key_ref 'codefactory.endpoint.chatgpt'",
          failure_attribution: {
            kind: "model-provider",
            label: "模型/Provider",
            summary: "API key not found for key_ref 'codefactory.endpoint.chatgpt'",
            next_action: "修复 endpoint、API key 或余额后再重试。",
            repairable: false,
            source: "error",
          },
        }),
      ],
    };
    const onOpenSettings = vi.fn();
    renderWorkspace(onOpenSettings);

    const activity = screen.getByRole("button", { name: "打开任务活动：1 个后台任务需要修复模型配置" });
    expect(activity).toHaveTextContent("1");
    expect(activity).not.toHaveTextContent("模型配置待修复");
    expect(activity).toHaveAttribute("title", "模型配置需要处理");
    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    expect(screen.getByText("模型/Provider").closest("[data-status-tone]")).toHaveAttribute(
      "data-status-tone",
      "danger",
    );

    await userEvent.click(screen.getByRole("button", { name: "打开模型设置" }));
    expect(onOpenSettings).toHaveBeenCalledWith("endpoints");

    expect(screen.queryByRole("button", { name: /已修复，重试/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /已授权，重试/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /回到对话处理/ })).not.toBeInTheDocument();
    expect(mocks.retryTasks).not.toHaveBeenCalled();
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.retryFailedTasks).not.toHaveBeenCalled();
  });
  it("does not turn cancelled or completed task history into a persistent alert", () => {
    fakeTasksState.tasks = {
      s1: [task({ id: "cancelled", status: "cancelled" }), task({ id: "done", status: "completed" })],
    };
    renderWorkspace();
    expect(screen.queryByRole("button", { name: /打开任务活动/ })).not.toBeInTheDocument();
  });

  it("does not label a system-owned technical failure as user-retryable", () => {
    fakeTasksState.tasks = { s1: [task({ status: "failed", failure_attribution: { repairable: true } })] };
    renderWorkspace();
    const activity = screen.getByRole("button", { name: /打开任务活动/ });
    expect(activity).toHaveTextContent("1");
    expect(activity).not.toHaveAccessibleName(/可重试/);
    expect(activity).toHaveClass("text-status-warning");
  });

  it("keeps an unknown technical failure in system recovery without injecting a fake user message", async () => {
    fakeTasksState.tasks = {
      s1: [task({
        status: "failed",
        error: "opaque worker crash 73",
        failure_attribution: {
          kind: "unknown",
          label: "未分类",
          summary: "opaque worker crash 73",
          next_action: "回到对话继续诊断。",
          repairable: false,
          source: "error",
        },
      })],
    };
    renderWorkspace();
    await userEvent.click(screen.getByRole("button", { name: /打开任务活动/ }));
    expect(screen.queryByRole("button", { name: "回到对话处理" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重试失败步骤" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /已修复，重试/ })).not.toBeInTheDocument();
    expect(screen.queryByTestId("pending-repair-prompt")).not.toBeInTheDocument();
    expect(fakeChatState.sendMessage).not.toHaveBeenCalled();
    expect(fakeChatState.sendOrQueue).not.toHaveBeenCalled();
  });

});
