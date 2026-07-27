// SPDX-License-Identifier: Apache-2.0
//
// Regression cover for the bug this workspace layout exists to kill: picking a
// project used to drop the user into that project's PREVIOUS conversation.
//
// These run against the real chat store (only the Tauri bridge is mocked), so
// they cover the seam where it actually broke — the empty-state welcome screen
// called selectSession directly, loading history and leaving the shell's idea
// of "the open session" pointing somewhere else entirely.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  openDialog: vi.fn(),
  onStream: vi.fn(),
  onSessionUpdated: vi.fn(),
}));

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
vi.mock("../../lib/tauri", async (orig) => ({
  ...((await orig()) as object),
  invoke: mocks.invoke,
  onStream: mocks.onStream,
  onSessionUpdated: mocks.onSessionUpdated,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openDialog }));

import { WorkspacePage } from "./WorkspacePage";
import { useChatStore, openSessionId } from "../../stores/chat";

const projectSession = {
  id: "s-old",
  title: "上一次的对话",
  cwd: "/code/ledger",
  model_id: "m",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project" as const,
};

const HISTORY_LINE = "这是历史消息";

/** Mirrors the app shell: the open-session id is derived from the store. */
function Shell() {
  const sessionId = useChatStore(openSessionId);
  const beginDraft = useChatStore((s) => s.beginDraft);
  const selectSession = useChatStore((s) => s.selectSession);
  if (!sessionId) return null;
  return (
    <WorkspacePage
      sessionId={sessionId}
      onNewConversation={(cwd) => beginDraft({ cwd: cwd ?? null })}
      onOpenSettings={() => {}}
      onOpenSession={(id) => void selectSession(id)}
    />
  );
}

describe("picking a project never opens its history", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.onStream.mockResolvedValue(() => {});
    mocks.onSessionUpdated.mockResolvedValue(() => {});
    mocks.invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_sessions") return Promise.resolve([projectSession]);
      if (cmd === "get_session") {
        return args?.sessionId === projectSession.id
          ? Promise.resolve(projectSession)
          : Promise.reject(new Error(`no such session: ${String(args?.sessionId)}`));
      }
      if (cmd === "get_message_page") {
        return Promise.resolve({
          messages: [
            { id: "m1", session_id: "s-old", role: "user", content: HISTORY_LINE, created_at: 1 },
          ],
          next_before_rowid: null,
          has_more: false,
          truncated: false,
        });
      }
      if (cmd === "is_chat_running") return Promise.resolve(false);
      return Promise.resolve(null);
    });
    useChatStore.setState({
      sessions: [projectSession],
      activeSession: null,
      draftSession: null,
      runtime: {},
      activeModel: "m",
      _unlisten: {},
      _unlistenSessionUpdated: {},
      _streamingMsgId: {},
      _draftMaterialization: null,
      _selectionRequestId: 0,
    });
    useChatStore.getState().beginDraft();
  });

  it("keeps the blank draft when a project is picked from the welcome screen", async () => {
    const draftId = useChatStore.getState().draftSession?.id;
    render(<Shell />);
    const scopeSection = await screen.findByRole("region", { name: "这次在哪里干活" });

    fireEvent.click(within(scopeSection).getByText("ledger"));

    await waitFor(() => {
      expect(useChatStore.getState().draftSession?.cwd).toBe("/code/ledger");
    });
    const state = useChatStore.getState();
    // Same blank conversation, re-scoped — not the project's old session.
    expect(state.draftSession?.id).toBe(draftId);
    expect(state.activeSession).toBeNull();
    expect(screen.queryByText(HISTORY_LINE)).not.toBeInTheDocument();
    expect(mocks.invoke).not.toHaveBeenCalledWith("get_message_page", expect.anything());
  });

  it("keeps the blank draft when a project is picked from the composer scope bar", async () => {
    render(<Shell />);

    fireEvent.click(await screen.findByRole("button", { name: "选择项目" }));
    const menu = await screen.findByRole("menu", { name: "项目选择" });
    fireEvent.click(within(menu).getByTitle("/code/ledger"));

    await waitFor(() => {
      expect(useChatStore.getState().draftSession?.cwd).toBe("/code/ledger");
    });
    expect(useChatStore.getState().activeSession).toBeNull();
    expect(screen.queryByText(HISTORY_LINE)).not.toBeInTheDocument();
  });

  it("starts a NEW conversation from a project's sidebar + action", async () => {
    render(<Shell />);

    fireEvent.click(await screen.findByLabelText("在 ledger 里新建会话"));

    await waitFor(() => {
      expect(useChatStore.getState().draftSession?.cwd).toBe("/code/ledger");
    });
    expect(useChatStore.getState().activeSession).toBeNull();
    expect(screen.queryByText(HISTORY_LINE)).not.toBeInTheDocument();
  });

  it("resumes history only from an explicit conversation row, and the shell follows", async () => {
    render(<Shell />);

    // Expand the project, then click the conversation itself.
    const rail = await screen.findByRole("complementary", { name: "会话列表" });
    fireEvent.click(within(rail).getByText("ledger"));
    fireEvent.click(within(rail).getByText("上一次的对话"));

    await screen.findByText(HISTORY_LINE);
    const state = useChatStore.getState();
    expect(state.activeSession?.id).toBe(projectSession.id);
    expect(state.draftSession).toBeNull();
    // The shell's session id is derived, so it cannot drift from the chat pane.
    expect(openSessionId(state)).toBe(projectSession.id);
  });
});
