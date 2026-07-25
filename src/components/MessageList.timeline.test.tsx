// SPDX-License-Identifier: Apache-2.0
//
// Turn-timeline rendering. The 26-minute-turn screenshot showed every tool
// card stacked above one 800-character narration wall. With segments, the
// row renders in arrival order, mid-turn narration reads as light step
// lines, and long turns collapse their early steps.

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
    expect(steps[0]).toHaveClass("text-[15px]", "leading-6");
    expect(steps[0]).not.toHaveClass("text-[13px]", "border-l");
    const finals = container.querySelectorAll("[data-segment='final']");
    expect(finals.length).toBe(1);
    expect(finals[0].textContent).toContain("最终总结");
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
