// SPDX-License-Identifier: Apache-2.0
//
// Interaction tests for the Workspace SessionSidebar (Codex-style rail):
// unified quick+project list with time groups, newest-first order, active highlight,
// in-place switching, and the "+ 新建" menu (quick / project). jsdom — no real
// Tauri backend, so the chat store + tauri helpers + dialog are mocked.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  beginQuickDraft: vi.fn(),
  beginProjectDraft: vi.fn(),
  startAnonymousSession: vi.fn(),
  openDialog: vi.fn(),
  loadSessions: vi.fn(),
  loadQuickSessions: vi.fn(),
}));

const mk = (over: Record<string, unknown>) => ({
  id: "x", title: "", cwd: "/x", model_id: "m", created_at: 1, updated_at: 1,
  total_input_tokens: 0, total_output_tokens: 0, ...over,
});

// Two projects + one quick, interleaved update times to prove the merge sort.
const fakeChatState = {
  sessions: [
    mk({ id: "p1", title: "CodeFactory", updated_at: 300, kind: "project" }),
    mk({ id: "p2", title: "记账 app", updated_at: 100, kind: "project" }),
  ],
  quickSessions: [mk({ id: "q1", title: "改图脚本", updated_at: 200, kind: "quick" })],
  activeModel: "anthropic/claude-opus-4-7",
  draftSession: null,
  beginQuickDraft: mocks.beginQuickDraft,
  beginProjectDraft: mocks.beginProjectDraft,
  startAnonymousSession: mocks.startAnonymousSession,
  loadSessions: mocks.loadSessions,
  loadQuickSessions: mocks.loadQuickSessions,
};

vi.mock("../stores/chat", () => ({
  useChatStore: Object.assign(
    <T,>(selector?: (s: typeof fakeChatState) => T): T | typeof fakeChatState =>
      selector ? selector(fakeChatState) : fakeChatState,
    { setState: vi.fn(), getState: () => fakeChatState },
  ),
}));
vi.mock("../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openDialog }));

import { SessionSidebar } from "./SessionSidebar";

describe("SessionSidebar", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.loadSessions.mockResolvedValue(undefined);
    mocks.loadQuickSessions.mockResolvedValue(undefined);
  });

  it("renders a flat, grouped, newest-first list without redundant type badges", () => {
    render(<SessionSidebar currentSessionId="p1" onOpenSession={() => {}} />);
    expect(screen.getByText("CodeFactory")).toBeInTheDocument();
    expect(screen.getByText("改图脚本")).toBeInTheDocument();
    expect(screen.getByText("记账 app")).toBeInTheDocument();
    expect(screen.getByText("会话")).toBeInTheDocument();
    expect(screen.getByText("更早")).toBeInTheDocument();
    expect(screen.queryByText("快速")).not.toBeInTheDocument();
    expect(screen.queryByText("项目")).not.toBeInTheDocument();
    // order by updated_at desc: p1(300) > q1(200) > p2(100)
    const order = screen
      .getAllByRole("button")
      .map((b) => b.textContent || "")
      .filter((t) => /CodeFactory|改图脚本|记账/.test(t));
    expect(order[0]).toContain("CodeFactory");
    expect(order[1]).toContain("改图脚本");
    expect(order[2]).toContain("记账");
  });

  it("marks the current session active via aria-current", () => {
    render(<SessionSidebar currentSessionId="q1" onOpenSession={() => {}} />);
    expect(screen.getByText("改图脚本").closest('[role="button"]')).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("CodeFactory").closest('[role="button"]')).not.toHaveAttribute("aria-current");
  });

  it("switches session in place when a row is clicked", () => {
    const onOpen = vi.fn();
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);
    fireEvent.click(screen.getByText("记账 app"));
    expect(onOpen).toHaveBeenCalledWith("p2");
  });

  it("opens a fresh in-memory quick draft from + 新建 without persistence", async () => {
    const onOpen = vi.fn();
    mocks.beginQuickDraft.mockReturnValue({ id: "draft-q", mode: "quick", cwd: null, modelId: "m", text: "" });
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);

    fireEvent.click(screen.getByRole("button", { name: /新建/ }));
    fireEvent.click(await screen.findByText("新建快速任务"));

    await waitFor(() => expect(mocks.beginQuickDraft).toHaveBeenCalledTimes(1));
    expect(onOpen).toHaveBeenCalledWith("draft-q");
    expect(mocks.loadQuickSessions).not.toHaveBeenCalledTimes(2);
  });

  it("opens a project draft after directory selection without persistence", async () => {
    const onOpen = vi.fn();
    mocks.openDialog.mockResolvedValue("/Users/x/newproj");
    mocks.beginProjectDraft.mockReturnValue({ id: "draft-p", mode: "project", cwd: "/Users/x/newproj", modelId: "m", text: "" });
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);

    fireEvent.click(screen.getByRole("button", { name: /新建/ }));
    fireEvent.click(await screen.findByText("新建项目"));

    await waitFor(() => expect(mocks.openDialog).toHaveBeenCalled());
    expect(mocks.beginProjectDraft).toHaveBeenCalledWith("/Users/x/newproj");
    expect(onOpen).toHaveBeenCalledWith("draft-p");
  });

  it("aborts project creation when the picker is cancelled", async () => {
    const onOpen = vi.fn();
    mocks.openDialog.mockResolvedValue(null);
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);

    fireEvent.click(screen.getByRole("button", { name: /新建/ }));
    fireEvent.click(await screen.findByText("新建项目"));

    await waitFor(() => expect(mocks.openDialog).toHaveBeenCalled());
    expect(mocks.beginProjectDraft).not.toHaveBeenCalled();
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("reveals rename/delete from a low-emphasis row action", () => {
    render(<SessionSidebar currentSessionId="p1" onOpenSession={() => {}} />);
    // The menu is closed until the kebab is clicked.
    expect(screen.queryByText("重命名")).not.toBeInTheDocument();
    fireEvent.click(screen.getAllByLabelText("更多操作")[0]);
    expect(screen.getByText("重命名")).toBeInTheDocument();
    expect(screen.getByText("删除")).toBeInTheDocument();
  });

  it("double-clicking a session title opens an inline rename input", () => {
    render(<SessionSidebar currentSessionId="p1" onOpenSession={() => {}} />);
    fireEvent.doubleClick(screen.getByText("CodeFactory"));
    expect(screen.getByDisplayValue("CodeFactory")).toBeInTheDocument();
  });
});
