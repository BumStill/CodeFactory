// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

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
            waitingReason: null,
            waitingHistory: ["等待 CI"],
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

  it("shows a compact activity snapshot before a structured plan exists", () => {
    render(
      <MessageList
        messages={[
          { id: "user", role: "user", content: "检查并修复", createdAt: 1 },
          {
            id: "assistant",
            role: "assistant",
            content: "",
            createdAt: Date.now() - 5_000,
            turnActivity: {
              rootTurnId: "user",
              revision: 4,
              phase: "recovering",
              status: "active",
              kind: "verification",
              label: "正在补充缺失验证",
              waitingReason: "验证证据不足",
              updatedAt: Date.now(),
              terminalReason: null,
            },
          },
        ]}
        streaming
        cwd={null}
      />,
    );

    expect(screen.getByTestId("turn-activity-progress")).toHaveTextContent(
      "正在补充缺失验证",
    );
    expect(screen.getByTestId("turn-activity-progress")).toHaveTextContent(
      "验证证据不足",
    );
    expect(screen.getByTestId("turn-activity-progress")).toHaveAttribute(
      "data-status-tone",
      "warning",
    );
  });

  it("keeps a long-tool waiting reason visible even when a structured plan exists", () => {
    const running = messages(false);
    running[1] = {
      ...running[1],
      plan: { ...plan, waitingReason: null },
      turnActivity: {
        rootTurnId: "user",
        revision: 8,
        phase: "working",
        status: "active",
        kind: "tool",
        label: "命令仍在运行（约 1 分钟）",
        waitingReason: "命令已连续运行约 1 分钟",
        updatedAt: Date.now(),
        terminalReason: null,
      },
    };

    render(<MessageList messages={running} streaming cwd={null} />);

    expect(screen.getByTestId("turn-progress")).toHaveTextContent(
      "命令已连续运行约 1 分钟",
    );
    expect(screen.getByTestId("turn-progress")).not.toHaveTextContent(
      "SECRET_COMMAND",
    );
  });

  it("forms a local result snapshot immediately after the terminal state", () => {
    const onOpenEvidence = vi.fn();
    render(
      <MessageList
        messages={messages(true)}
        streaming={false}
        cwd={null}
        onOpenEvidence={onOpenEvidence}
      />,
    );

    expect(screen.getByTestId("turn-result-snapshot")).toHaveTextContent("已完成");
    fireEvent.click(screen.getByRole("button", { name: "查看证据" }));
    expect(onOpenEvidence).toHaveBeenCalledWith("assistant");
    expect(screen.queryByText("等待与失败边界")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "结果摘要" }));
    expect(screen.getByRole("status")).toHaveTextContent("完成 3/3 个计划步骤");
  });

  it("does not mark a completed plan green when the same turn has failure evidence", () => {
    const failed = messages(true);
    failed[1] = {
      ...failed[1],
      failureEvidence: "provider credential unavailable",
    };

    render(<MessageList messages={failed} streaming={false} cwd={null} />);

    expect(screen.getByTestId("turn-result-snapshot")).toHaveAttribute(
      "data-status-tone",
      "warning",
    );
    expect(screen.getByTestId("turn-result-snapshot")).toHaveTextContent(
      "已执行，证据待复核",
    );
    expect(screen.getByTestId("turn-result-snapshot")).not.toHaveTextContent(
      "需要你处理",
    );
    expect(screen.getByTestId("failure-resolution-card")).toHaveAccessibleName("失败证据");
    expect(screen.queryByText("需要处理")).not.toBeInTheDocument();
  });

  it.each([
    ["system", "系统继续处理", false],
    ["external", "外部等待", false],
    ["user", "需要你处理", true],
  ] as const)("keeps failure evidence neutral when next action owner is %s", (owner, label, userOwned) => {
    const failed = messages(true);
    failed[1] = {
      ...failed[1],
      failureEvidence: "provider credential unavailable",
      plan: {
        ...failed[1].plan!,
        waitingReason: "等待下一步",
        nextActionOwner: owner,
      },
    };

    render(<MessageList messages={failed} streaming={false} cwd={null} />);

    expect(screen.getByTestId("failure-resolution-card")).toHaveAccessibleName("失败证据");
    expect(screen.getByTestId("turn-result-snapshot")).toHaveTextContent(label);
    expect(screen.queryByText("需要处理")).not.toBeInTheDocument();
    expect(screen.queryAllByText("需要你处理")).toHaveLength(userOwned ? 1 : 0);
  });
});
