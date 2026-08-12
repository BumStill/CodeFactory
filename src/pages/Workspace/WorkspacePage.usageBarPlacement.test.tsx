// SPDX-License-Identifier: Apache-2.0
//
// Ownership contract for the compact composer controls. Model, reasoning,
// permission and context all describe the next turn, so they belong to the
// composer and must never be duplicated in the workspace header.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  loadTasks: vi.fn().mockResolvedValue(undefined),
  subscribe: vi.fn().mockResolvedValue(() => {}),
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

vi.mock("../../components/ModelPicker", () => ({
  ModelPicker: () => (
    <div>
      <button aria-label="选择下一回合模型">模型</button>
      <span data-testid="model-panel-reasoning-owner" />
    </div>
  ),
}));
vi.mock("../../components/ReasoningEffortPicker", () => ({
  ReasoningEffortPicker: () => (
    <select aria-label="下一回合思考强度" defaultValue="medium">
      <option value="medium">思考·中</option>
    </select>
  ),
}));
vi.mock("../../components/PermissionModePicker", () => ({
  PermissionModePicker: () => (
    <select aria-label="会话权限" defaultValue="standard">
      <option value="standard">标准</option>
    </select>
  ),
}));
vi.mock("../../components/GitStatusBar", () => ({
  GitStatusBar: () => <button aria-label="Git 状态">Git</button>,
}));
vi.mock("../../components/CheckpointsPanel", () => ({ CheckpointsPanel: () => null }));
vi.mock("../../components/WorkspaceDeliveryStatus", () => ({ WorkspaceDeliveryStatus: () => null }));
vi.mock("../../components/MessageList", () => ({ MessageList: () => null }));
vi.mock("../../components/MessageInput", () => ({
  MessageInput: ({ toolbar }: { toolbar?: React.ReactNode }) => (
    <div data-testid="message-input">
      <div role="toolbar" aria-label="输入工具" className="flex min-w-0 max-w-full flex-wrap overflow-x-clip">
        {toolbar}
      </div>
    </div>
  ),
}));
vi.mock("../../components/PermissionDialog", () => ({ PermissionDialog: () => null }));
vi.mock("../../components/ContextUsageBar", () => ({
  ContextUsageBar: () => <button aria-label="打开上下文与用量详情" data-testid="context-usage-ring" />,
}));
vi.mock("../../stores/settings", () => ({
  useSettingsStore: Object.assign(
    () => ({
      settings: { theme: "dark", permissions: { full_access: false } },
      setTheme: vi.fn(),
    }),
    { getState: () => ({ settings: { remote_postmortem_enabled: false } }) },
  ),
}));

vi.mock("../../stores/tasks", () => ({
  useTasksStore: Object.assign(
    <T,>(selector?: (state: Record<string, unknown>) => T): T | Record<string, unknown> =>
      selector ? selector({
        running: {},
        tasks: {},
        executionLog: {},
        loadTasks: vi.fn().mockResolvedValue(undefined),
        subscribe: vi.fn().mockResolvedValue(() => {}),
        start: vi.fn(),
        cancel: vi.fn(),
        retryFailedTasks: vi.fn(),
      }) : {
        running: {},
        tasks: {},
        executionLog: {},
        loadTasks: vi.fn().mockResolvedValue(undefined),
        subscribe: vi.fn().mockResolvedValue(() => {}),
        start: vi.fn(),
        cancel: vi.fn(),
        retryFailedTasks: vi.fn(),
      },
    { setState: vi.fn(), getState: () => ({
      running: {},
      tasks: {},
      executionLog: {},
      loadTasks: vi.fn().mockResolvedValue(undefined),
      subscribe: vi.fn().mockResolvedValue(() => {}),
      start: vi.fn(),
      cancel: vi.fn(),
      retryFailedTasks: vi.fn(),
    }) },
  ),
}));

vi.mock("../../stores/learning", () => ({
  useLearningStore: Object.assign(
    <T,>(selector?: (state: Record<string, unknown>) => T): T | Record<string, unknown> =>
      selector ? selector({ events: {}, loading: {} }) : { events: {}, loading: {} },
    { setState: vi.fn(), getState: () => ({ events: {}, loading: {} }) },
  ),
}));

describe("composer runtime control ownership", () => {
  beforeEach(() => {
    mocks.invoke.mockReset().mockImplementation((command: string) => {
      if (command === "list_browser_sessions" || command === "get_turn_timing_profile") {
        return new Promise(() => {});
      }
      return Promise.resolve(undefined);
    });
    mocks.loadTasks.mockReset().mockResolvedValue(undefined);
    mocks.subscribe.mockReset().mockResolvedValue(() => {});
  });

  it("keeps one compact utility toolbar and does not mount a standalone reasoning trigger", async () => {
    const { WorkspacePage } = await import("./WorkspacePage");
    const { container } = render(
      <WorkspacePage
        sessionId="s1"
        onNewConversation={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />,
    );
    const shell = container.querySelector('[data-testid="workspace-composer-shell"]');
    expect(shell).not.toBeNull();
    const composer = within(shell as HTMLElement);
    const header = within(container.querySelector('header[aria-label="会话工具栏"]') as HTMLElement);

    const toolbar = composer.getByRole("toolbar", { name: "输入工具" });
    expect(composer.getAllByRole("toolbar", { name: "输入工具" })).toHaveLength(1);
    expect(toolbar).toHaveClass("min-w-0", "max-w-full", "flex-wrap");
    expect(toolbar.className).toMatch(/overflow-(?:x-)?(?:hidden|clip)/);
    expect(within(toolbar).getByRole("button", { name: "选择下一回合模型" })).toBeInTheDocument();
    expect(composer.queryByRole("combobox", { name: "下一回合思考强度" })).not.toBeInTheDocument();
    expect(composer.getByTestId("model-panel-reasoning-owner")).toBeInTheDocument();
    expect(composer.getByRole("combobox", { name: "会话权限" })).toBeInTheDocument();
    expect(composer.getByRole("button", { name: "打开上下文与用量详情" })).toBeInTheDocument();

    expect(header.queryByRole("button", { name: "选择下一回合模型" })).not.toBeInTheDocument();
    expect(header.queryByRole("combobox", { name: "下一回合思考强度" })).not.toBeInTheDocument();
    expect(header.queryByRole("combobox", { name: "会话权限" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "选择下一回合模型" })).toHaveLength(1);
    expect(screen.queryByRole("combobox", { name: "下一回合思考强度" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("combobox", { name: "会话权限" })).toHaveLength(1);
  });
});
