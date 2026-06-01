// SPDX-License-Identifier: Apache-2.0
//
// The collapsed-sidebar quick switcher: it must still surface the full unified
// session list (so collapsing never buries navigation), switch on click, and
// offer a "pin open" (expand) action.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

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

const fakeChatState = {
  sessions: [mk({ id: "p1", title: "CodeFactory", updated_at: 300, kind: "project" })],
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

import { SessionSwitcherPopover } from "./SessionSwitcherPopover";

describe("SessionSwitcherPopover", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.loadSessions.mockResolvedValue(undefined);
    mocks.loadQuickSessions.mockResolvedValue(undefined);
  });

  it("surfaces the full unified switcher list while collapsed", () => {
    render(
      <SessionSwitcherPopover currentSessionId="p1" onOpenSession={() => {}} onExpand={() => {}} />,
    );
    expect(screen.getByText("快速切换会话")).toBeInTheDocument();
    expect(screen.getByText("CodeFactory")).toBeInTheDocument();
    expect(screen.getByText("改图脚本")).toBeInTheDocument();
  });

  it("pins the sidebar back open via the expand action", () => {
    const onExpand = vi.fn();
    render(
      <SessionSwitcherPopover currentSessionId="p1" onOpenSession={() => {}} onExpand={onExpand} />,
    );
    fireEvent.click(screen.getByTitle("固定展开侧栏"));
    expect(onExpand).toHaveBeenCalledTimes(1);
  });

  it("switches session when a row is clicked", () => {
    const onOpen = vi.fn();
    render(
      <SessionSwitcherPopover currentSessionId="p1" onOpenSession={onOpen} onExpand={() => {}} />,
    );
    fireEvent.click(screen.getByText("改图脚本"));
    expect(onOpen).toHaveBeenCalledWith("q1");
  });
});
