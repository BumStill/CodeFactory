// SPDX-License-Identifier: Apache-2.0
//
// Interaction tests for the Workspace SessionSidebar: one recency-ordered
// list where a folder only becomes a group after it holds a second
// conversation, plus the two rules the old sidebar broke:
//   - "+" always starts a BLANK conversation, never resumes one
//   - clicking a FOLDER expands it; only a conversation row opens history
// jsdom — no real Tauri backend, so the chat store is mocked.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  loadSessions: vi.fn(),
  deleteSession: vi.fn(),
  renameSession: vi.fn(),
}));

const mk = (over: Record<string, unknown>) => ({
  id: "x", title: "", cwd: "/x", model_id: "m", created_at: 1, updated_at: 1,
  total_input_tokens: 0, total_output_tokens: 0, kind: "project", ...over,
});

// Two conversations in one project, one in another, plus a standalone task.
const fakeChatState: { sessions: ReturnType<typeof mk>[] } & Record<string, unknown> = {
  sessions: [
    mk({ id: "p1a", title: "CodeFactory 主线", cwd: "/code/CodeFactory", updated_at: 400 }),
    mk({ id: "q1", title: "改图脚本", cwd: "/home/.codefactory/quick/q1", updated_at: 300, kind: "quick" }),
    mk({ id: "p1b", title: "CodeFactory 旧会话", cwd: "/code/CodeFactory", updated_at: 200 }),
    mk({ id: "p2", title: "记账 app", cwd: "/code/ledger", updated_at: 100 }),
  ],
  runtime: {},
  activeModel: "anthropic/claude-opus-4-7",
  draftSession: null,
  loadSessions: mocks.loadSessions,
  deleteSession: mocks.deleteSession,
  renameSession: mocks.renameSession,
};

vi.mock("../stores/chat", () => ({
  useChatStore: Object.assign(
    <T,>(selector?: (s: typeof fakeChatState) => T): T | typeof fakeChatState =>
      selector ? selector(fakeChatState) : fakeChatState,
    { setState: vi.fn(), getState: () => fakeChatState },
  ),
}));
vi.mock("../lib/tauri", async (orig) => ({ ...((await orig()) as Record<string, unknown>) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { SessionSidebar } from "./SessionSidebar";

const noop = () => {};

describe("SessionSidebar", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.loadSessions.mockResolvedValue(undefined);
  });

  it("keeps one recency-ordered list with no imposed sections", () => {
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
    );

    // A folder used twice became a group; everything else stays a plain row.
    expect(screen.getByText("CodeFactory")).toBeInTheDocument();
    expect(screen.getByText("改图脚本")).toBeInTheDocument();
    expect(screen.getByText("记账 app")).toBeInTheDocument();
    // No "项目 / 独立任务" headers telling the user to classify anything.
    expect(screen.queryByText("项目")).not.toBeInTheDocument();
    expect(screen.queryByText("独立任务")).not.toBeInTheDocument();
  });

  it("marks a background session that is waiting for permission", () => {
    fakeChatState.runtime = {
      q1: {
        streaming: true,
        pendingPermission: { toolCallId: "tc-1", toolName: "bash", args: {} },
      },
    };
    try {
      render(
        <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
      );
      expect(screen.getByLabelText("等待批准")).toBeInTheDocument();
    } finally {
      fakeChatState.runtime = {};
    }
  });

  it("leaves a folder used once as an ordinary row, not a group", () => {
    const onOpen = vi.fn();
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={onOpen} onNewConversation={noop} />,
    );

    // /code/ledger holds a single conversation → the conversation itself is the
    // row (the folder is only a subtitle), with nothing to expand and no
    // per-folder action yet. Clicking it opens that conversation directly.
    expect(screen.queryByLabelText("在 ledger 里新建会话")).not.toBeInTheDocument();
    const row = screen.getByText("记账 app").closest('[role="button"]');
    expect(row).not.toHaveAttribute("aria-expanded");

    fireEvent.click(screen.getByText("记账 app"));
    expect(onOpen).toHaveBeenCalledWith("p2");
  });

  it("grows a group as soon as a folder holds a second conversation", () => {
    fakeChatState.sessions = [
      ...fakeChatState.sessions,
      mk({ id: "p2b", title: "记账 app 续", cwd: "/code/ledger", updated_at: 90 }),
    ];
    try {
      render(
        <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
      );

      expect(screen.getByText("ledger")).toBeInTheDocument();
      expect(screen.getByLabelText("在 ledger 里新建会话")).toBeInTheDocument();
    } finally {
      fakeChatState.sessions = fakeChatState.sessions.filter((s) => s.id !== "p2b");
    }
  });

  it("expands the folder holding the open conversation", () => {
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
    );

    expect(screen.getByText("CodeFactory 主线")).toBeInTheDocument();
    expect(screen.getByText("CodeFactory 旧会话")).toBeInTheDocument();
  });

  it("keeps an expanded project open when switching to another conversation", () => {
    const { rerender } = render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
    );

    expect(screen.getByText("CodeFactory 主线")).toBeInTheDocument();
    expect(screen.getByText("CodeFactory 旧会话")).toBeInTheDocument();

    rerender(
      <SessionSidebar currentSessionId="q1" onOpenSession={noop} onNewConversation={noop} />,
    );

    expect(screen.getByText("CodeFactory 主线")).toBeInTheDocument();
    expect(screen.getByText("CodeFactory 旧会话")).toBeInTheDocument();
  });

  it("clicking a folder expands it instead of opening a conversation", () => {
    const onOpen = vi.fn();
    render(
      <SessionSidebar currentSessionId="q1" onOpenSession={onOpen} onNewConversation={noop} />,
    );

    // CodeFactory holds two conversations, so it is a collapsed group here.
    expect(screen.queryByText("CodeFactory 主线")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("CodeFactory"));

    expect(onOpen).not.toHaveBeenCalled();
    expect(screen.getByText("CodeFactory 主线")).toBeInTheDocument();
  });

  it("opens history only when a conversation row is clicked", () => {
    const onOpen = vi.fn();
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={onOpen} onNewConversation={noop} />,
    );

    fireEvent.click(screen.getByText("CodeFactory 旧会话"));

    expect(onOpen).toHaveBeenCalledWith("p1b");
  });

  it("marks the current conversation active via aria-current", () => {
    render(
      <SessionSidebar currentSessionId="q1" onOpenSession={noop} onNewConversation={noop} />,
    );

    expect(screen.getByText("改图脚本").closest('[role="button"]')).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("starts a blank conversation from + 新建 with no project attached", () => {
    const onNew = vi.fn();
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={onNew} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "新建会话" }));

    expect(onNew).toHaveBeenCalledWith(null);
  });

  it("starts a blank conversation scoped to a folder from its group action", () => {
    const onNew = vi.fn();
    const onOpen = vi.fn();
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={onOpen} onNewConversation={onNew} />,
    );

    fireEvent.click(screen.getByLabelText("在 CodeFactory 里新建会话"));

    // New conversation in that folder — emphatically NOT its latest session.
    expect(onNew).toHaveBeenCalledWith("/code/CodeFactory");
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("reveals rename/delete from a low-emphasis row action", () => {
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
    );

    expect(screen.queryByText("重命名")).not.toBeInTheDocument();
    fireEvent.click(screen.getAllByLabelText("更多操作")[0]);
    expect(screen.getByText("重命名")).toBeInTheDocument();
    expect(screen.getByText("删除")).toBeInTheDocument();
  });

  it("double-clicking a conversation title opens an inline rename input", () => {
    render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
    );

    fireEvent.doubleClick(screen.getByText("CodeFactory 主线"));

    expect(screen.getByDisplayValue("CodeFactory 主线")).toBeInTheDocument();
  });

  it("applies scrollbar-auto-hide to the list for hover-only scrollbar visibility", () => {
    const { container } = render(
      <SessionSidebar currentSessionId="p1a" onOpenSession={noop} onNewConversation={noop} />,
    );

    const list = container.querySelector(".scrollbar-auto-hide");
    expect(list).not.toBeNull();
    expect(list!).toHaveClass("overflow-y-auto");
  });
});
