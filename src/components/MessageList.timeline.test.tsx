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

  it("keeps the process expanded when the mounted active turn settles", () => {
    const segments: TurnSegment[] = [
      { kind: "text", text: "先确认信息为什么突然消失。" },
      ...Array.from({ length: 12 }, (_, index): TurnSegment => ({
        kind: "tool",
        toolCallId: `active-${index}`,
      })),
      { kind: "text", text: "已经定位到固定位置截断。" },
    ];
    const toolCalls = Array.from({ length: 12 }, (_, index) => ({
      id: `active-${index}`,
      name: "read_file",
      args: JSON.stringify({ path: `src/file-${index}.ts` }),
      status: index === 11 ? "running" as const : "done" as const,
      result: index === 11 ? undefined : "ok",
    }));
    const active = msg({ content: "已经定位到固定位置截断。", segments, toolCalls });

    const { rerender } = render(
      <MessageList messages={[active]} streaming turnActive cwd={null} />,
    );

    expect(screen.getByText("先确认信息为什么突然消失。")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /例行操作/ }),
    ).not.toBeInTheDocument();

    const settled = {
      ...active,
      toolCalls: toolCalls.map((tool) => ({ ...tool, status: "done" as const, result: "ok" })),
    };
    rerender(
      <MessageList messages={[settled]} streaming={false} turnActive={false} cwd={null} />,
    );

    expect(screen.getByText("先确认信息为什么突然消失。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "读取 · src/file-0.ts" })).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /例行操作/ }),
    ).not.toBeInTheDocument();

    rerender(
      <MessageList
        messages={[
          settled,
          msg({ id: "next-user", role: "user", content: "继续处理下一件事。" }),
        ]}
        streaming={false}
        turnActive={false}
        cwd={null}
      />,
    );

    expect(screen.getByText("先确认信息为什么突然消失。")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "读取 · src/file-0.ts" })).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /展开 12 项例行操作.*读取 12/ }),
    ).toBeInTheDocument();
  });

  it("compacts only routine successes in an earlier turn while keeping causal milestones", () => {
    const earlierSegments: TurnSegment[] = [
      { kind: "text", text: "根因：固定阈值把叙述和工具一并截断。" },
      ...Array.from({ length: 6 }, (_, index): TurnSegment => ({
        kind: "tool",
        toolCallId: `history-read-${index}`,
      })),
      { kind: "text", text: "决策：只折叠例行读取，保留因果链。" },
      { kind: "tool", toolCallId: "history-edit" },
      { kind: "tool", toolCallId: "history-test" },
      { kind: "tool", toolCallId: "history-failure" },
      { kind: "tool", toolCallId: "history-permission" },
      { kind: "tool", toolCallId: "history-unknown" },
      { kind: "text", text: "最终结论仍然可见。" },
    ];
    const earlierTools = [
      ...Array.from({ length: 6 }, (_, index) => ({
        id: `history-read-${index}`,
        name: "read_file",
        args: JSON.stringify({ path: `src/history-${index}.ts` }),
        status: "done" as const,
        result: "ok",
      })),
      { id: "history-edit", name: "edit_file", args: JSON.stringify({ path: "src/MessageList.tsx" }), status: "done" as const, result: "updated" },
      { id: "history-test", name: "bash", args: JSON.stringify({ command: "pnpm test" }), status: "done" as const, result: "12 passed" },
      { id: "history-failure", name: "bash", args: JSON.stringify({ command: "pnpm check" }), status: "error" as const, isError: true, result: "check failed" },
      { id: "history-permission", name: "read_file", args: JSON.stringify({ path: "private.ts" }), status: "waiting_permission" as const },
      { id: "history-unknown", name: "custom_probe", args: "{}", status: "done" as const, result: "material finding" },
    ];

    render(
      <MessageList
        messages={[
          msg({
            id: "history",
            content: "最终结论仍然可见。",
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

    expect(screen.getByText("根因：固定阈值把叙述和工具一并截断。")).toBeInTheDocument();
    expect(screen.getByText("决策：只折叠例行读取，保留因果链。")).toBeInTheDocument();
    expect(screen.getByText("最终结论仍然可见。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /编辑.*MessageList\.tsx/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /命令.*pnpm test/ })).toBeInTheDocument();
    expect(screen.getByText("check failed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /读取.*private\.ts/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "custom_probe" })).toBeInTheDocument();
    expect(screen.getByText("当前步骤保持可见。")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /展开 6 项例行操作.*读取 6/ }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "读取 · src/history-0.ts" })).not.toBeInTheDocument();
  });

  it("expands historical routine operations in place without hiding narration", () => {
    const segments: TurnSegment[] = [
      { kind: "text", text: "先保留这条关键判断。" },
      ...Array.from({ length: 5 }, (_, index): TurnSegment => ({ kind: "tool", toolCallId: `t${index}` })),
      { kind: "text", text: "收尾总结。" },
    ];
    const toolCalls = Array.from({ length: 5 }, (_, index) => ({
      id: `t${index}`,
      name: index < 3 ? "read_file" : "grep",
      args: index < 3
        ? JSON.stringify({ path: index === 0 ? "token=SECRET-private.ts" : `src/file-${index}.ts` })
        : JSON.stringify({ pattern: "collapse" }),
      status: "done" as const,
      result: index === 0 ? "SECRET-output" : "ok",
    }));
    const long = msg({
      content: "收尾总结。",
      segments,
      toolCalls,
      plan: {
        rootTurnId: "root",
        revision: 1,
        steps: [{ id: "done", title: "完成", kind: "analysis", status: "completed" }],
        explanation: null,
        waitingReason: null,
        changeReason: null,
        createdAt: 1,
      },
    });

    render(<MessageList messages={[long]} streaming={false} cwd={null} />);
    expect(screen.getByText("先保留这条关键判断。")).toBeInTheDocument();
    expect(screen.getByText(/收尾总结/)).toBeTruthy();
    const toggle = screen.getByRole("button", {
      name: /展开 5 项例行操作.*读取 3.*搜索 2/,
    });
    expect(toggle).toHaveTextContent("已收起 5 项例行操作");
    expect(toggle).not.toHaveClass("border");
    expect(toggle).not.toHaveTextContent(/SECRET|private\.ts/);
    expect(toggle.getAttribute("aria-label")).not.toMatch(/SECRET|private\.ts/);
    expect(screen.queryByRole("button", { name: "执行过程" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /例行操作/ })).toHaveLength(1);
    toggle.focus();
    fireEvent.click(toggle);
    expect(screen.getByRole("button", { name: /读取.*SECRET-private\.ts/ })).toBeInTheDocument();
    const expandedToggle = screen.getByRole("button", { name: /收起 5 项例行操作/ });
    expect(expandedToggle).toHaveAttribute("aria-expanded", "true");
    expect(expandedToggle).toHaveAttribute("aria-controls");
    expect(document.activeElement).toBe(expandedToggle);
    const controlsId = expandedToggle.getAttribute("aria-controls");
    expect(document.getElementById(controlsId!)).toHaveAttribute("role", "group");
  });

  it("keeps routine evidence flat while the objective is waiting on the system", () => {
    const toolCalls = Array.from({ length: 4 }, (_, index) => ({
      id: `wait-${index}`,
      name: "read_file",
      args: JSON.stringify({ path: `src/wait-${index}.ts` }),
      status: "done" as const,
      result: "ok",
    }));
    const waiting = msg({
      content: "系统恢复仍在继续。",
      segments: [
        { kind: "text", text: "系统恢复仍在继续。" },
        ...toolCalls.map((tool): TurnSegment => ({ kind: "tool", toolCallId: tool.id })),
      ],
      toolCalls,
      turnActivity: {
        rootTurnId: "root",
        revision: 2,
        phase: "recovery",
        status: "waiting",
        kind: "tool_wait",
        label: "系统正在恢复",
        waitingReason: "等待恢复服务",
        updatedAt: 2,
        terminalReason: null,
        objectiveStatus: "waiting_system",
      },
    });

    render(
      <MessageList messages={[waiting]} streaming={false} turnActive={false} cwd={null} />,
    );

    expect(screen.queryByRole("button", { name: /例行操作/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "读取 · src/wait-0.ts" })).toBeInTheDocument();
    expect(screen.getByText("系统恢复仍在继续。")).toBeInTheDocument();
  });

  it("resets an expanded historical disclosure when the conversation changes", () => {
    const toolCalls = Array.from({ length: 3 }, (_, index) => ({
      id: `history-${index}`,
      name: "read_file",
      args: JSON.stringify({ path: `src/history-${index}.ts` }),
      status: "done" as const,
      result: "ok",
    }));
    const historical = msg({
      content: "历史结论。",
      segments: [
        ...toolCalls.map((tool): TurnSegment => ({ kind: "tool", toolCallId: tool.id })),
        { kind: "text", text: "历史结论。" },
      ],
      toolCalls,
    });
    const { rerender } = render(
      <MessageList messages={[historical]} streaming={false} conversationKey="session-a" cwd={null} />,
    );

    fireEvent.click(screen.getByRole("button", { name: /展开 3 项例行操作/ }));
    expect(screen.getByRole("button", { name: "读取 · src/history-0.ts" })).toBeInTheDocument();

    rerender(
      <MessageList messages={[historical]} streaming={false} conversationKey="session-b" cwd={null} />,
    );

    expect(screen.getByRole("button", { name: /展开 3 项例行操作/ })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "读取 · src/history-0.ts" })).not.toBeInTheDocument();
  });
  it("keeps mutation, verification and failure evidence visible after a turn settles", () => {
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
    expect(screen.queryByRole("button", { name: /例行操作/ })).not.toBeInTheDocument();
    expect(screen.getByText(/check failed/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /读取.*a.ts/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /编辑.*a.ts/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /命令.*npm test/ })).toBeInTheDocument();
  });

});
