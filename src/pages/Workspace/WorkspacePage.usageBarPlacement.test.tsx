// SPDX-License-Identifier: Apache-2.0
//
// Placement contract for the token/context readout: the usage bar sits
// directly ABOVE the message input (as a composer header row), not below it
// at the bottom of the conversation. Regression guard for the UX feedback
// "token 消耗和 context 放到了最下边，占用了大量空间".

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, within } from "@testing-library/react";

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

vi.mock("../../components/ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("../../components/GitStatusBar", () => ({
  GitStatusBar: () => <button aria-label="Git 状态">Git</button>,
}));
vi.mock("../../components/CheckpointsPanel", () => ({ CheckpointsPanel: () => null }));
vi.mock("../../components/WorkspaceDeliveryStatus", () => ({ WorkspaceDeliveryStatus: () => null }));
vi.mock("../../components/MessageList", () => ({ MessageList: () => null }));
vi.mock("../../components/MessageInput", () => ({
  MessageInput: () => <div data-testid="message-input" />,
}));
vi.mock("../../components/PermissionDialog", () => ({ PermissionDialog: () => null }));
vi.mock("../../components/ContextUsageBar", () => ({
  ContextUsageBar: () => <div data-testid="context-usage-bar" />,
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

describe("usage bar placement above the composer", () => {
  beforeEach(() => {
    mocks.invoke.mockReset().mockResolvedValue(undefined);
    mocks.loadTasks.mockReset().mockResolvedValue(undefined);
    mocks.subscribe.mockReset().mockResolvedValue(() => {});
  });

  it("renders the usage bar above the message input inside the composer shell", async () => {
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
    const usageBar = within(shell as HTMLElement).queryByTestId("context-usage-bar");
    const input = within(shell as HTMLElement).queryByTestId("message-input");
    expect(usageBar).not.toBeNull();
    expect(input).not.toBeNull();
    // DOM order: usage bar must come BEFORE the input box.
    expect(
      (usageBar as HTMLElement).compareDocumentPosition(input as HTMLElement) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });
});
