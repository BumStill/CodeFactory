// SPDX-License-Identifier: Apache-2.0
//
// Turn-timeline rendering. The 26-minute-turn screenshot showed every tool
// card stacked above one 800-character narration wall. With segments, the
// row renders in arrival order, mid-turn narration reads as light step
// lines, and only settled long turns collapse their early steps.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MessageList } from "./MessageList";
import type { UIMessage, TurnSegment } from "../stores/chatEvents";

const msg = (over: Partial<UIMessage> = {}): UIMessage => ({
  id: "m1",
  role: "assistant",
  content: "",
  createdAt: Date.now(),
  ...over,
});

function interleaved(): UIMessage {
  return msg({
    content: "两类红灯都稳定复现了。最终总结:全部修复。",
    segments: [
      { kind: "text", text: "两类红灯都稳定复现了。" },
      { kind: "tool", toolCallId: "t1" },
      { kind: "text", text: "最终总结:全部修复。" },
    ],
    toolCalls: [
      { id: "t1", name: "bash", args: "{}", status: "done", result: "ok" },
    ],
  });
}

describe("MessageList turn timeline", () => {
  it("renders segments in arrival order instead of tools-then-text blobs", () => {
    const { container } = render(
      <MessageList messages={[interleaved()]} streaming={false} cwd={null} />,
    );
    const text = container.textContent ?? "";
    const first = text.indexOf("两类红灯都稳定复现了");
    const tool = text.indexOf("命令");
    const last = text.indexOf("最终总结");
    expect(first).toBeGreaterThanOrEqual(0);
    expect(tool).toBeGreaterThan(first);
    expect(last).toBeGreaterThan(tool);
  });

  it("styles mid-turn narration as step lines and the final segment as prose", () => {
    const { container } = render(
      <MessageList messages={[interleaved()]} streaming={false} cwd={null} />,
    );
    const steps = container.querySelectorAll("[data-segment='step']");
    expect(steps.length).toBe(1);
    expect(steps[0].textContent).toContain("两类红灯都稳定复现了");
    expect(steps[0]).toHaveClass("text-reading", "leading-6");
    expect(steps[0]).not.toHaveClass("text-note", "border-l");
    const finals = container.querySelectorAll("[data-segment='final']");
    expect(finals.length).toBe(1);
    expect(finals[0].textContent).toContain("最终总结");
    // The answer is never smaller than the narration leading up to it. This
    // branch used to carry no font size and inherited the row's 14px, so a
    // turn with tool calls rendered its step lines at 15px and its actual
    // answer at 14px — the most important text on the surface, smallest.
    expect(finals[0]).toHaveClass("text-reading");
  });

  it("preserves Markdown when a live tail becomes an intermediate step", () => {
    const formatted = [
      "**当前验证状态**",
      "",
      "- 已通过 `pnpm test`",
      "- [查看详情](https://example.com/check)",
    ].join("\n");
    const first = msg({
      content: formatted,
      segments: [{ kind: "text", text: formatted }],
    });
    const { container, rerender } = render(
      <MessageList messages={[first]} streaming cwd={null} />,
    );

    expect(container.querySelector("[data-segment='final'] strong")).toHaveTextContent(
      "当前验证状态",
    );
    expect(container.querySelector("[data-segment='final'] code")).toHaveTextContent(
      "pnpm test",
    );

    const continued = msg({
      content: `${formatted}继续执行。`,
      segments: [
        { kind: "text", text: formatted },
        { kind: "tool", toolCallId: "t1" },
        { kind: "text", text: "继续执行。" },
      ],
      toolCalls: [
        { id: "t1", name: "bash", args: "{}", status: "done", result: "ok" },
      ],
    });
    rerender(<MessageList messages={[continued]} streaming cwd={null} />);

    const step = container.querySelector("[data-segment='step']");
    expect(step?.querySelector("strong")).toHaveTextContent("当前验证状态");
    expect(step?.querySelector("code")).toHaveTextContent("pnpm test");
    expect(step?.querySelector("li")).toHaveTextContent("已通过 pnpm test");
    expect(step?.querySelector("a")).toHaveAttribute(
      "href",
      "https://example.com/check",
    );
    expect(step).not.toHaveTextContent("**当前验证状态**");
  });

  it("keeps a long active turn flat until it reaches a terminal state", () => {
    const segments: TurnSegment[] = [];
    const toolCalls = [];
    for (let i = 0; i < 12; i++) {
      segments.push({ kind: "text", text: `执行中第 ${i} 步。` });
      segments.push({ kind: "tool", toolCallId: `active-${i}` });
      toolCalls.push({
        id: `active-${i}`,
        name: "bash",
        args: "{}",
        status: i === 11 ? "running" as const : "done" as const,
        result: i === 11 ? undefined : "ok",
      });
    }
    const long = msg({ content: "仍在执行。", segments, toolCalls });

    const { rerender } = render(
      <MessageList messages={[long]} streaming cwd={null} />,
    );

    expect(screen.getByText("执行中第 0 步。")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /展开较早的执行过程/ }),
    ).not.toBeInTheDocument();

    rerender(<MessageList messages={[long]} streaming={false} cwd={null} />);

    expect(screen.queryByText("执行中第 0 步。")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /展开较早的执行过程，共 \d+ 条/ }),
    ).toBeInTheDocument();
  });

  it("still collapses an earlier settled turn while a newer turn is active", () => {
    const earlierSegments: TurnSegment[] = [];
    const earlierTools = [];
    for (let i = 0; i < 12; i++) {
      earlierSegments.push({ kind: "text", text: `历史第 ${i} 步。` });
      earlierSegments.push({ kind: "tool", toolCallId: `history-${i}` });
      earlierTools.push({
        id: `history-${i}`,
        name: "bash",
        args: "{}",
        status: "done" as const,
        result: "ok",
      });
    }

    render(
      <MessageList
        messages={[
          msg({
            id: "history",
            content: "历史完成。",
            segments: earlierSegments,
            toolCalls: earlierTools,
          }),
          msg({ id: "next-user", role: "user", content: "继续。" }),
          msg({
            id: "active",
            content: "正在继续。",
            segments: [{ kind: "text", text: "当前步骤保持可见。" }],
          }),
        ]}
        streaming
        cwd={null}
      />,
    );

    expect(screen.queryByText("历史第 0 步。")).not.toBeInTheDocument();
    expect(screen.getByText("当前步骤保持可见。")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /展开较早的执行过程，共 \d+ 条/ }),
    ).toBeInTheDocument();
  });

  it("collapses early steps of a very long turn behind a toggle", () => {
    const segments: TurnSegment[] = [];
    const toolCalls = [];
    for (let i = 0; i < 12; i++) {
      segments.push({ kind: "text", text: `第 ${i} 步叙述。` });
      segments.push({ kind: "tool", toolCallId: `t${i}` });
      toolCalls.push({
        id: `t${i}`,
        name: "bash",
        args: "{}",
        status: "done" as const,
        result: "ok",
      });
    }
    segments.push({ kind: "text", text: "收尾总结。" });
    const long = msg({ content: "…", segments, toolCalls });

    render(<MessageList messages={[long]} streaming={false} cwd={null} />);
    // Early steps are hidden by default…
    expect(screen.queryByText(/第 0 步叙述/)).toBeNull();
    // …behind a summary toggle, while the tail stays visible.
    expect(screen.getByText(/收尾总结/)).toBeTruthy();
    const toggle = screen.getByRole("button", {
      name: /展开较早的执行过程，共 \d+ 条/,
    });
    expect(toggle).toHaveTextContent("展开较早的执行过程");
    expect(toggle).not.toHaveTextContent(/前 \d+ 步|点击展开/);
    expect(toggle).not.toHaveClass("border");
    fireEvent.click(toggle);
    expect(screen.getByText(/第 0 步叙述/)).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "收起较早的执行过程" }),
    ).toBeInTheDocument();
  });
  it("groups consecutive successful commands after a turn settles but keeps failures visible", () => {
    const grouped = msg({
      content: "完成。",
      segments: [
        { kind: "tool", toolCallId: "ok-1" },
        { kind: "tool", toolCallId: "ok-2" },
        { kind: "tool", toolCallId: "ok-3" },
        { kind: "tool", toolCallId: "bad" },
        { kind: "text", text: "完成。" },
      ],
      toolCalls: [
        { id: "ok-1", name: "read_file", args: JSON.stringify({ path: "a.ts" }), status: "done", result: "ok" },
        { id: "ok-2", name: "edit_file", args: JSON.stringify({ path: "a.ts" }), status: "done", result: "ok" },
        { id: "ok-3", name: "bash", args: JSON.stringify({ command: "npm test" }), status: "done", result: "ok" },
        { id: "bad", name: "bash", args: JSON.stringify({ command: "npm run check" }), status: "error", isError: true, result: "check failed" },
      ],
    });

    render(<MessageList messages={[grouped]} streaming={false} cwd={null} />);
    expect(screen.getByRole("button", { name: "查看 3 个已完成操作" })).toBeInTheDocument();
    expect(screen.getByText(/check failed/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /读取.*a.ts/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "查看 3 个已完成操作" }));
    expect(screen.getByRole("button", { name: /读取.*a.ts/ })).toBeInTheDocument();
  });

});
