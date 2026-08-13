// SPDX-License-Identifier: Apache-2.0
// A turn the system still owns must stay stoppable from the composer. When
// streaming ended but recovery kept the Objective alive, the composer fell back
// to "发送" and the only visible affordance started ANOTHER turn — so the
// 2026-08-13 sessions could not be ended at all, and resumed on every restart.
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

type TurnActivity = { objectiveStatus?: string };
type RuntimeMessage = { id: string; role: string; content: string; createdAt: number; turnActivity?: TurnActivity };

const runtime: {
  messages: RuntimeMessage[];
  streaming: boolean;
  queue: unknown[];
  pendingPermission: null;
  inputTokenTotal: number;
  outputTokenTotal: number;
  contextUsage: null;
  compressionToast: null;
} = {
  messages: [],
  streaming: false,
  queue: [],
  pendingPermission: null,
  inputTokenTotal: 0,
  outputTokenTotal: 0,
  contextUsage: null,
  compressionToast: null,
};

const activeSession = {
  id: "session-1",
  title: "会话",
  cwd: "/repo",
  kind: "project",
  modelId: "model",
  permissionMode: "trusted",
};
const chatState = {
  sessions: [activeSession],
  activeSession,
  draftSession: null,
  activeModel: "model",
  selectSession: vi.fn(),
  sendOrQueue: vi.fn(),
  cancelStream: vi.fn(),
  removeFromQueue: vi.fn(),
  respondPermission: vi.fn(),
  exitAnonymous: vi.fn(),
  renameSession: vi.fn(),
  beginDraft: vi.fn(),
  setDraftProject: vi.fn(),
  setDraftAnonymous: vi.fn(),
  deleteSession: vi.fn(),
  loadSessions: vi.fn(),
};

vi.mock("../../stores/chat", () => ({
  useChatStore: Object.assign(
    <T,>(selector?: (s: typeof chatState) => T): T | typeof chatState => (selector ? selector(chatState) : chatState),
    { getState: () => chatState, setState: vi.fn() },
  ),
  activeRuntime: () => runtime,
}));
vi.mock("../../stores/tasks", () => ({
  useTasksStore: (selector?: (s: {
    tasks: Record<string, unknown[]>;
    running: Record<string, boolean>;
    executionLog: Record<string, unknown[]>;
    loading: Record<string, boolean>;
    error: Record<string, string | null>;
    loadTasks: () => Promise<void>;
    subscribe: () => Promise<() => void>;
  }) => unknown) => {
    // Deliberately NOT running: a chat turn held by system recovery is exactly
    // the case the task-level running flag does not cover.
    const state = {
      tasks: {},
      running: {},
      executionLog: {},
      loading: {},
      error: {},
      loadTasks: vi.fn(async () => {}),
      subscribe: vi.fn(async () => () => {}),
    };
    return selector ? selector(state) : state;
  },
}));
vi.mock("../../stores/settings", () => ({
  useSettingsStore: () => ({ settings: { theme: "dark", permissions: { full_access: false } }, setTheme: vi.fn() }),
}));
vi.mock("../../stores/learning", () => ({
  useLearningStore: (selector: (s: { events: Record<string, unknown[]>; load: () => Promise<void>; subscribe: () => Promise<() => void> }) => unknown) =>
    selector({ events: {}, load: vi.fn(), subscribe: vi.fn(async () => () => {}) }),
}));
vi.mock("../../stores/skills", () => ({ useSkillsStore: () => ({ skills: [], loadSkills: vi.fn() }) }));
vi.mock("../../components/ModelPicker", () => ({ ModelPicker: () => <button aria-label="选择模型">模型：model</button> }));
vi.mock("../../components/ReasoningEffortPicker", () => ({ ReasoningEffortPicker: () => null }));
vi.mock("../../components/GitStatusBar", () => ({ GitStatusBar: () => null }));
vi.mock("../../components/CheckpointsPanel", () => ({ CheckpointsPanel: () => null }));
vi.mock("../../components/ExecutionStream", () => ({ ExecutionStream: () => null }));
vi.mock("../../components/MessageList", () => ({ MessageList: () => <div>会话</div> }));
vi.mock("../../components/MessageInput", () => ({
  // Surface the prop that decides whether the composer offers stop or send.
  MessageInput: ({ streaming }: { streaming: boolean }) => (
    <div data-testid="composer-mode">{streaming ? "停止后续生成" : "发送"}</div>
  ),
}));
vi.mock("../../components/ContextUsageBar", () => ({ ContextUsageBar: () => null }));
vi.mock("../../components/PermissionDialog", () => ({ PermissionDialog: () => null }));
vi.mock("../../components/WorkspaceDeliveryStatus", () => ({ WorkspaceDeliveryStatus: () => null }));
vi.mock("../../lib/tauri", async (orig) => ({
  ...((await orig()) as object),
  invoke: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { WorkspacePage } from "./WorkspacePage";

function renderWorkspace() {
  return render(
    <WorkspacePage sessionId="session-1" onNewConversation={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
  );
}

function assistantMessage(objectiveStatus?: string): RuntimeMessage {
  return {
    id: "assistant-1",
    role: "assistant",
    content: "分析中",
    createdAt: 1,
    ...(objectiveStatus ? { turnActivity: { objectiveStatus } } : {}),
  };
}

describe("system-owned turns stay stoppable", () => {
  it("offers stop while recovery still owns the turn, even though streaming ended", () => {
    runtime.streaming = false;
    runtime.messages = [assistantMessage("waiting_system")];

    renderWorkspace();

    expect(screen.getByTestId("composer-mode")).toHaveTextContent("停止后续生成");
  });

  it("keeps offering stop for an active objective that is between model rounds", () => {
    runtime.streaming = false;
    runtime.messages = [assistantMessage("active")];

    renderWorkspace();

    expect(screen.getByTestId("composer-mode")).toHaveTextContent("停止后续生成");
  });

  it("returns to send once the objective reaches a terminal state", () => {
    runtime.streaming = false;
    for (const terminal of ["completed", "cancelled", "legacy_orphan"]) {
      runtime.messages = [assistantMessage(terminal)];
      const view = renderWorkspace();
      expect(screen.getByTestId("composer-mode")).toHaveTextContent("发送");
      view.unmount();
    }
  });

  it("returns to send for a turn that never carried objective activity", () => {
    runtime.streaming = false;
    runtime.messages = [assistantMessage(undefined)];

    renderWorkspace();

    expect(screen.getByTestId("composer-mode")).toHaveTextContent("发送");
  });
});
