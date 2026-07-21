// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  selectSession: vi.fn(),
  sendOrQueue: vi.fn(),
  loadSessions: vi.fn(),
  loadQuickSessions: vi.fn(),
}));

const draft = { id: "draft-1", mode: "quick" as const, cwd: null, modelId: "model", text: "" };
const chatState = {
  sessions: [],
  quickSessions: [],
  activeSession: null,
  draftSession: draft,
  activeModel: "model",
  selectSession: mocks.selectSession,
  sendOrQueue: mocks.sendOrQueue,
  cancelStream: vi.fn(),
  removeFromQueue: vi.fn(),
  respondPermission: vi.fn(),
  exitAnonymous: vi.fn(),
  renameSession: vi.fn(),
  beginQuickDraft: vi.fn(),
  beginProjectDraft: vi.fn(),
  startAnonymousSession: vi.fn(),
  deleteSession: vi.fn(),
  loadSessions: mocks.loadSessions,
  loadQuickSessions: mocks.loadQuickSessions,
};
const runtime = {
  messages: [], streaming: false, queue: [], pendingPermission: null,
  inputTokenTotal: 0, outputTokenTotal: 0, contextUsage: null, compressionToast: null,
};

vi.mock("../../stores/chat", () => ({
  useChatStore: Object.assign(
    <T,>(selector?: (s: typeof chatState) => T): T | typeof chatState => selector ? selector(chatState) : chatState,
    { getState: () => chatState, setState: vi.fn() },
  ),
  activeRuntime: () => runtime,
}));
vi.mock("../../stores/tasks", () => ({
  useTasksStore: (selector?: (s: {
    tasks: Record<string, unknown[]>;
    running: Record<string, boolean>;
    executionLog: Record<string, unknown[]>;
    loadTasks: () => Promise<void>;
  }) => unknown) => {
    const state = { tasks: {}, running: {}, executionLog: {}, loadTasks: vi.fn(async () => {}) };
    return selector ? selector(state) : state;
  },
}));
vi.mock("../../stores/settings", () => ({ useSettingsStore: () => ({ settings: { theme: "dark", permissions: { full_access: false } }, setTheme: vi.fn() }) }));
vi.mock("../../stores/learning", () => ({
  useLearningStore: (selector: (s: { events: Record<string, unknown[]>; load: () => Promise<void>; subscribe: () => Promise<() => void> }) => unknown) =>
    selector({ events: {}, load: vi.fn(), subscribe: vi.fn(async () => () => {}) }),
}));
vi.mock("../../stores/skills", () => ({ useSkillsStore: () => ({ skills: [], loadSkills: vi.fn() }) }));
vi.mock("../../components/ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("../../components/ReasoningEffortPicker", () => ({ ReasoningEffortPicker: () => null }));
vi.mock("../../components/GitStatusBar", () => ({ GitStatusBar: () => null }));
vi.mock("../../components/CheckpointsPanel", () => ({ CheckpointsPanel: () => <span>不应出现检查点</span> }));
vi.mock("../../components/ExecutionStream", () => ({ ExecutionStream: () => <span>不应出现执行流</span> }));
vi.mock("../../components/MessageList", () => ({ MessageList: () => <div>空白会话</div> }));
vi.mock("../../components/MessageInput", () => ({
  MessageInput: ({ disabled, onSend }: { disabled: boolean; onSend: (text: string) => void }) => (
    <button disabled={disabled} onClick={() => onSend("第一条消息")}>发送第一条消息</button>
  ),
}));
vi.mock("../../components/ContextUsageBar", () => ({ ContextUsageBar: () => null }));
vi.mock("../../components/PermissionDialog", () => ({ PermissionDialog: () => null }));
vi.mock("../../lib/tauri", async (orig) => ({ ...(await orig() as object), invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { WorkspacePage } from "./WorkspacePage";

describe("Workspace virtual draft", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockClear());
    try {
      localStorage.setItem("cf.workspace.sidebarCollapsed", "1");
    } catch {
      // Some local runners do not expose localStorage; the structural
      // assertions below still prove there is no collapse control.
    }
  });

  it("keeps the session rail permanently visible and the chat input enabled", async () => {
    render(<WorkspacePage sessionId="draft-1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />);

    expect(screen.getByRole("complementary", { name: "会话列表" })).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "会话窗口" })).toBeInTheDocument();
    expect(screen.queryByTitle(/收起会话侧栏|展开侧栏/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "发送第一条消息" })).toBeEnabled();
    expect(screen.getAllByText("草稿").length).toBeGreaterThan(0);
    expect(screen.queryByText("不应出现检查点")).not.toBeInTheDocument();
    expect(screen.queryByText("不应出现执行流")).not.toBeInTheDocument();
    await waitFor(() => expect(mocks.selectSession).not.toHaveBeenCalled());
  });
});
