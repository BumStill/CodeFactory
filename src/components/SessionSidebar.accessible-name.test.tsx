// SPDX-License-Identifier: Apache-2.0
//
// R4:同名会话必须能被无障碍工具区分。
//
// 侧边栏里一排「新会话」在无障碍树里原本是完全相同的 AXButton:后台无障碍
// 工具只能靠"第几个按钮"去点,读屏软件也读不出区别。这一层断言渲染出来的
// 无障碍名称里确实带着短 id 与相对时间 —— 也就是"后台点它"时能认准人。

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  loadSessions: vi.fn(),
  deleteSession: vi.fn(),
  renameSession: vi.fn(),
}));

const mk = (over: Record<string, unknown>) => ({
  id: "x",
  title: "",
  cwd: "/x",
  model_id: "m",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "quick",
  ...over,
});

// 两个同名会话 —— 这正是无法区分的那一对。
const NOW = Date.now();
const fakeChatState: { sessions: ReturnType<typeof mk>[] } & Record<string, unknown> = {
  sessions: [
    mk({ id: "9537257c-1111-2222-3333-444455556666", title: "新会话", updated_at: NOW - 5 * 60_000 }),
    mk({ id: "11112222-aaaa-bbbb-cccc-ddddeeeeffff", title: "新会话", updated_at: NOW - 5 * 60_000 }),
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

describe("同名会话的无障碍名称", () => {
  it("每个会话按钮的名称里都带短 id 与相对时间,且彼此可区分", () => {
    render(<SessionSidebar currentSessionId="9537257c-1111-2222-3333-444455556666" onOpenSession={noop} onNewConversation={noop} />);

    const first = screen.getByRole("button", { name: /打开会话 .*9537257c/ });
    const second = screen.getByRole("button", { name: /打开会话 .*11112222/ });
    expect(first).toBeInTheDocument();
    expect(second).toBeInTheDocument();

    // 名称形如「打开会话 新会话(9537257c,5 分钟前)」。
    expect(first.getAttribute("aria-label")).toMatch(/^打开会话 新会话\(9537257c,/);
    expect(first.getAttribute("aria-label")).toContain("分钟前");
    expect(second.getAttribute("aria-label")).toMatch(/^打开会话 新会话\(11112222,/);
    // 关键:两个同名会话的无障碍名称不同,后台工具才能点准。
    expect(first.getAttribute("aria-label")).not.toBe(second.getAttribute("aria-label"));
  });

  it("当前会话仍然被标记为当前页", () => {
    render(<SessionSidebar currentSessionId="11112222-aaaa-bbbb-cccc-ddddeeeeffff" onOpenSession={noop} onNewConversation={noop} />);
    const current = screen.getByRole("button", { name: /打开会话 .*11112222/ });
    expect(current).toHaveAttribute("aria-current", "page");
  });
});
