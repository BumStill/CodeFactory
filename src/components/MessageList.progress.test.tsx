// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { TurnPlan } from "../lib/chatPlan";
import type { UIMessage } from "../stores/chatEvents";
import { MessageList } from "./MessageList";

const plan: TurnPlan = {
  rootTurnId: "user",
  revision: 2,
  explanation: null,
  waitingReason: "等待 CI",
  changeReason: null,
  createdAt: 2,
  steps: [
    { id: "implement", title: "实现修改", kind: "implementation", status: "completed" },
    { id: "verify", title: "验证真实应用", kind: "verification", status: "in_progress" },
    { id: "deliver", title: "交付 PR", kind: "delivery", status: "pending" },
  ],
};

function messages(terminal: boolean): UIMessage[] {
  return [
    { id: "user", role: "user", content: "完成任务", createdAt: 1 },
    {
      id: "assistant",
      role: "assistant",
      content: terminal ? "已完成并验证。" : "正在验证。",
      createdAt: 2,
      durationMs: terminal ? 5_000 : undefined,
      plan: terminal
        ? {
            ...plan,
            steps: plan.steps.map((step) => ({ ...step, status: "completed" })),
          }
        : plan,
      toolCalls: [
        {
          id: "test",
          name: "bash",
          args: JSON.stringify({ command: "pnpm test" }),
          status: terminal ? "done" : "running",
        },
      ],
      segments: [
        { kind: "tool", toolCallId: "test" },
        { kind: "text", text: terminal ? "已完成并验证。" : "正在验证。" },
      ],
    },
  ];
}

describe("MessageList structured progress and result", () => {
  it("keeps the current and next step visible while the turn runs", () => {
    render(<MessageList messages={messages(false)} streaming cwd={null} />);

    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "33");
    expect(screen.getByText(/当前 · 验证真实应用/)).toBeInTheDocument();
    expect(screen.getByText(/下一步 · 交付 PR/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /展开较早的执行过程/ }),
    ).not.toBeInTheDocument();
  });

  it("forms a local result snapshot immediately after the terminal state", () => {
    render(<MessageList messages={messages(true)} streaming={false} cwd={null} />);

    expect(screen.getByTestId("turn-result-snapshot")).toHaveTextContent("任务结果");
    fireEvent.click(screen.getByRole("button", { name: "证据化重新总结" }));
    expect(screen.getByRole("status")).toHaveTextContent("完成 3/3 个计划步骤");
  });
});
