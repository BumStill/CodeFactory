// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  selectSession: vi.fn(),
  sendOrQueue: vi.fn(),
  loadSessions: vi.fn(),
}));

const draft = { id: "draft-1", cwd: null, anonymous: false, modelId: "model", text: "" };
const chatState = {
  sessions: [],
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
  beginDraft: vi.fn(),
  setDraftProject: vi.fn(),
  setDraftAnonymous: vi.fn(),
  deleteSession: vi.fn(),
  loadSessions: mocks.loadSessions,
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
      localStorage.removeItem("cf.workspace.sidebarCollapsed");
    } catch {
      // Interaction assertions still cover runners without localStorage.
    }
  });

  it("removes the duplicate header new action and lets the user collapse and restore the session rail", async () => {
    render(<WorkspacePage sessionId="draft-1" onNewConversation={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />);

    expect(screen.queryByRole("button", { name: "新建空白会话" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "新建会话" })).toHaveLength(1);
    const sessionRail = screen.getByRole("complementary", { name: "会话列表" });
    const workspaceHeader = screen.getByRole("banner", { name: "会话工具栏" });
    expect(sessionRail).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "会话窗口" })).toBeInTheDocument();
    const collapse = within(sessionRail).getByRole("button", { name: "收起会话侧栏" });
    expect(collapse).toBeInTheDocument();
    expect(collapse.querySelector(".lucide-chevron-left")).toBeInTheDocument();
    expect(collapse.querySelector(".lucide-panel-left-close")).not.toBeInTheDocument();
    expect(within(workspaceHeader).queryByRole("button", { name: "收起会话侧栏" })).not.toBeInTheDocument();

    fireEvent.click(within(sessionRail).getByRole("button", { name: "收起会话侧栏" }));
    expect(screen.queryByRole("complementary", { name: "会话列表" })).not.toBeInTheDocument();
    const restore = within(workspaceHeader).getByRole("button", { name: "展开会话侧栏" });
    expect(restore).toHaveTextContent("会话");
    expect(restore.querySelector(".lucide-message-square")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "展开会话侧栏" }));
    expect(screen.getByRole("complementary", { name: "会话列表" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "发送第一条消息" })).toBeEnabled();
    expect(screen.getAllByText("草稿").length).toBeGreaterThan(0);
    expect(screen.queryByText("不应出现检查点")).not.toBeInTheDocument();
    expect(screen.queryByText("不应出现执行流")).not.toBeInTheDocument();
    await waitFor(() => expect(mocks.selectSession).not.toHaveBeenCalled());
  });
});
