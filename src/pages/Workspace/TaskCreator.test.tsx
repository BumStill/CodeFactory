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
}));

vi.mock("../../lib/tauri", async (original) => {
  const real = (await original()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const fakeChatState = {
  sessions: [] as unknown[],
  quickSessions: [] as unknown[],
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
  loadQuickSessions: vi.fn(),
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
vi.mock("../../components/CheckpointsPanel", () => ({
  CheckpointsPanel: () => <button aria-label="检查点 0">检查点 0</button>,
}));
vi.mock("../../components/MessageList", () => ({ MessageList: () => null }));
vi.mock("../../components/MessageInput", () => ({ MessageInput: () => null }));
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

const renderWorkspace = () =>
  render(
    <WorkspacePage
      sessionId="s1"
      onBackHome={() => {}}
      onOpenSettings={() => {}}
      onOpenSession={() => {}}
    />,
  );

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
    expect(within(header).getByRole("button", { name: "收起会话侧栏" })).toBeInTheDocument();
    expect(within(header).queryByRole("button", { name: "新建空白会话" })).not.toBeInTheDocument();
    expect(within(header).getByRole("button", { name: "Git 状态" })).toBeInTheDocument();
    expect(within(header).getByRole("button", { name: "检查点 0" })).toBeInTheDocument();
    expect(within(header).getByRole("button", { name: "设置" })).toBeInTheDocument();
    for (const label of ["我的画像", "进化审查", "能力评测", "资源中心", "AI Coding OS"]) {
      expect(within(header).queryByRole("button", { name: label })).not.toBeInTheDocument();
    }
    expect(within(header).queryByRole("group", { name: "主题" })).not.toBeInTheDocument();
  });

  it("keeps delegated execution out of the conversation and opens it from a compact task activity control", async () => {
    fakeTasksState.tasks = { s1: [task()] };
    renderWorkspace();

    const conversation = screen.getByRole("main", { name: "会话窗口" });
    expect(within(conversation).queryByText("会话执行详情")).not.toBeInTheDocument();
    expect(within(conversation).queryByText("实现登录页")).not.toBeInTheDocument();

    const activity = screen.getByRole("button", { name: "打开任务活动" });
    expect(activity).toHaveTextContent("待处理 1");
    await userEvent.click(activity);
    const drawer = screen.getByRole("dialog", { name: "任务活动" });
    expect(within(drawer).getByText("实现登录页")).toBeInTheDocument();
    expect(screen.queryByText("会话执行详情")).not.toBeInTheDocument();
  });

  it("keeps legacy spec provenance visible without reopening a spec product surface", async () => {
    fakeTasksState.tasks = {
      s1: [task({ spec_req_id: "CF-010", spec_title: "Token 成本仪表盘" })],
    };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: "打开任务活动" }));
    expect(screen.getByText("规范《Token 成本仪表盘》")).toBeInTheDocument();
    expect(screen.queryByTitle(/规范工作台/)).not.toBeInTheDocument();
  });

  it("continues pending delegated work from the session detail", async () => {
    fakeTasksState.tasks = { s1: [task()] };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: "打开任务活动" }));
    await userEvent.click(screen.getByRole("button", { name: "继续" }));
    expect(mocks.start).toHaveBeenCalledWith("s1");
  });

  it("stops a running delegated execution", async () => {
    fakeTasksState.tasks = { s1: [task({ status: "running" })] };
    fakeTasksState.running = { s1: true };
    renderWorkspace();

    await userEvent.click(screen.getByRole("button", { name: "打开任务活动" }));
    await userEvent.click(screen.getByRole("button", { name: "停止" }));
    expect(mocks.cancel).toHaveBeenCalledWith("s1");
  });

  it("retries repairable failures and resumes the same session", async () => {
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

    await userEvent.click(screen.getByRole("button", { name: "打开任务活动" }));
    await userEvent.click(screen.getByRole("button", { name: "重试失败步骤" }));
    await waitFor(() => expect(mocks.retryFailedTasks).toHaveBeenCalledWith("s1"));
    expect(mocks.start).toHaveBeenCalledWith("s1");
  });

  it("does not blindly retry a provider failure", async () => {
    fakeTasksState.tasks = {
      s1: [
        task({
          status: "failed",
          error: "HTTP 402 Insufficient Balance",
          failure_attribution: {
            kind: "model-provider",
            label: "模型/Provider",
            summary: "HTTP 402 Insufficient Balance",
            next_action: "修复 endpoint、API key 或余额后再重试。",
            repairable: false,
            source: "error",
          },
        }),
      ],
    };
    renderWorkspace();

    expect(screen.getByRole("button", { name: "打开任务活动" })).toHaveTextContent("1 项需处理");
    expect(screen.queryByText("模型/Provider")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "打开任务活动" }));
    expect(screen.getByText("模型/Provider")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "需在对话处理" })).toBeDisabled();
    expect(mocks.retryFailedTasks).not.toHaveBeenCalled();
    expect(mocks.start).not.toHaveBeenCalled();
  });
});
