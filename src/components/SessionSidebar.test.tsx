// SPDX-License-Identifier: Apache-2.0
//
// Interaction tests for the Workspace SessionSidebar (Codex-style rail):
// unified quick+project list with tags, newest-first order, active highlight,
// in-place switching, and the "+ 新建" menu (quick / project). jsdom — no real
// Tauri backend, so the chat store + tauri helpers + dialog are mocked.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  createQuickSession: vi.fn(),
  openDialog: vi.fn(),
  createSession: vi.fn(),
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
  createSession: mocks.createSession,
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
  return { ...real, createQuickSession: mocks.createQuickSession };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openDialog }));

import { SessionSidebar } from "./SessionSidebar";

describe("SessionSidebar", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.loadSessions.mockResolvedValue(undefined);
    mocks.loadQuickSessions.mockResolvedValue(undefined);
  });

  it("renders a unified, newest-first list with quick/project tags", () => {
    render(<SessionSidebar currentSessionId="p1" onOpenSession={() => {}} />);
    expect(screen.getByText("CodeFactory")).toBeInTheDocument();
    expect(screen.getByText("改图脚本")).toBeInTheDocument();
    expect(screen.getByText("记账 app")).toBeInTheDocument();
    // one 快速 tag, two 项目 tags
    expect(screen.getAllByText("快速")).toHaveLength(1);
    expect(screen.getAllByText("项目")).toHaveLength(2);
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

  it("creates a fresh quick task from the + 新建 menu and opens it", async () => {
    const onOpen = vi.fn();
    mocks.createQuickSession.mockResolvedValue(mk({ id: "qNew", kind: "quick" }));
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);

    fireEvent.click(screen.getByRole("button", { name: /新建/ }));
    fireEvent.click(await screen.findByText("新建快速任务"));

    await waitFor(() =>
      expect(mocks.createQuickSession).toHaveBeenCalledWith("anthropic/claude-opus-4-7"),
    );
    await waitFor(() => expect(onOpen).toHaveBeenCalledWith("qNew"));
    expect(mocks.loadQuickSessions).toHaveBeenCalled(); // list refreshed after create
  });

  it("creates a project via the directory picker", async () => {
    const onOpen = vi.fn();
    mocks.openDialog.mockResolvedValue("/Users/x/newproj");
    mocks.createSession.mockResolvedValue(mk({ id: "pNew", kind: "project" }));
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);

    fireEvent.click(screen.getByRole("button", { name: /新建/ }));
    fireEvent.click(await screen.findByText("新建项目"));

    await waitFor(() => expect(mocks.openDialog).toHaveBeenCalled());
    await waitFor(() =>
      expect(mocks.createSession).toHaveBeenCalledWith("/Users/x/newproj", "anthropic/claude-opus-4-7"),
    );
    await waitFor(() => expect(onOpen).toHaveBeenCalledWith("pNew"));
  });

  it("aborts project creation when the picker is cancelled", async () => {
    const onOpen = vi.fn();
    mocks.openDialog.mockResolvedValue(null);
    render(<SessionSidebar currentSessionId="p1" onOpenSession={onOpen} />);

    fireEvent.click(screen.getByRole("button", { name: /新建/ }));
    fireEvent.click(await screen.findByText("新建项目"));

    await waitFor(() => expect(mocks.openDialog).toHaveBeenCalled());
    expect(mocks.createSession).not.toHaveBeenCalled();
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("reveals a rename/delete menu from the always-visible ⋯ button", () => {
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
