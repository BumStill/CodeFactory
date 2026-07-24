// SPDX-License-Identifier: Apache-2.0

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { UIMessage } from "../stores/chat";

const markdownRender = vi.hoisted(() => vi.fn());
vi.mock("react-markdown", () => ({
  default: ({ children }: { children: unknown }) => {
    markdownRender(String(children));
    return <div>{String(children)}</div>;
  },
  defaultUrlTransform: (value: string) => value,
}));

import { MessageList } from "./MessageList";

describe("MessageList render isolation", () => {
  it("does not re-render historical markdown when only the streaming tail changes", () => {
    const history: UIMessage[] = Array.from({ length: 120 }, (_, index) => ({
      id: `assistant-${index}`,
      role: "assistant",
      content: `history-${index}`,
      createdAt: index,
    }));
    const { rerender } = render(
      <MessageList
        messages={history}
        streaming
        conversationKey="long-session"
      />,
    );
    markdownRender.mockClear();

    const tailIndex = history.length - 1;
    const next = history.slice();
    next[tailIndex] = {
      ...history[tailIndex],
      content: `${history[tailIndex].content} delta`,
    };
    rerender(
      <MessageList
        messages={next}
        streaming
        conversationKey="long-session"
      />,
    );

    expect(markdownRender).toHaveBeenCalledTimes(1);
    expect(markdownRender).toHaveBeenCalledWith("history-119 delta");
  });

  it("keeps the initial eight-turn production shape bounded with many persisted tool calls", () => {
    const messages: UIMessage[] = Array.from(
      { length: 8 },
      (_, turn): UIMessage[] => {
        const toolCalls = Array.from({ length: 36 }, (_, tool) => ({
          id: `turn-${turn}-tool-${tool}`,
          name: tool % 2 === 0 ? "read_file" : "bash",
          args:
            tool % 2 === 0
              ? JSON.stringify({ path: `/project/src/file-${tool}.ts` })
              : JSON.stringify({
                  command: `pnpm test --filter turn-${turn}-${tool}`,
                }),
          result: `completed tool ${tool}`,
          status: "done" as const,
        }));
        return [
          {
            id: `turn-${turn}-user`,
            role: "user",
            content: `production user turn ${turn}`,
            createdAt: turn * 10,
          },
          {
            id: `turn-${turn}-assistant`,
            role: "assistant",
            content: `production assistant final ${turn}`,
            toolCalls,
            createdAt: turn * 10 + 1,
          },
        ];
      },
    ).flat();

    const { container, getAllByRole } = render(
      <MessageList
        messages={messages}
        streaming={false}
        conversationKey="production-eight-turn-window"
        hasOlderHistory
        onLoadOlder={async () => {}}
      />,
    );

    expect(container.querySelectorAll("[data-message-row]")).toHaveLength(16);
    expect(container.querySelectorAll("[data-tool-status]")).toHaveLength(0);
    expect(
      getAllByRole("button", { name: /查看 36 个已完成操作/ }),
    ).toHaveLength(8);
    expect(container.querySelectorAll("*").length).toBeLessThanOrEqual(250);
  });

  it("shows the truthful safety-budget notice only for a truncated history page", () => {
    const messages: UIMessage[] = [
      {
        id: "assistant",
        role: "assistant",
        content: "latest final",
        createdAt: 1,
      },
    ];
    const { rerender } = render(
      <MessageList
        messages={messages}
        streaming={false}
        conversationKey="truncated"
        historyTruncated
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "部分超大历史内容仅显示预览或分段加载",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "完整原始记录仍保存在本机",
    );

    rerender(
      <MessageList
        messages={messages}
        streaming={false}
        conversationKey="truncated"
        historyTruncated={false}
      />,
    );
    expect(screen.queryByRole("status")).toBeNull();
  });
});
