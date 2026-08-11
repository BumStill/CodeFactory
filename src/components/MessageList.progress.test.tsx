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

  it("keeps a non-terminal system-owned objective visible after the stream closes", () => {
    const nextObservationAt = Date.now() + 30_000;
    const activity = {
      rootTurnId: "user",
      revision: 5,
      phase: "recovering",
      status: "active",
      kind: "remediation",
      label: "已切换到备用模型 route",
      waitingReason: "等待退避窗口结束",
      updatedAt: Date.now(),
      terminalReason: null,
      objectiveId: "objective-1",
      objectiveStatus: "waiting_system",
      recoveryOwner: "系统恢复监督器",
      nextObservationAt,
      lastProgressAt: Date.now() - 5_000,
    } as UIMessage["turnActivity"] & {
      objectiveId: string;
      objectiveStatus: "waiting_system";
      recoveryOwner: string;
      nextObservationAt: number;
      lastProgressAt: number;
    };

    render(
      <MessageList
        messages={[
          { id: "user", role: "user", content: "完成并发布", createdAt: 1 },
          {
            id: "assistant",
            role: "assistant",
            content: "模型连接暂时中断。",
            createdAt: Date.now() - 10_000,
            turnActivity: activity,
          },
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    const progress = screen.getByTestId("turn-activity-progress");
    expect(progress).toHaveTextContent(/系统仍在处理|恢复中/);
    expect(progress).toHaveTextContent("系统恢复监督器");
    expect(progress).toHaveTextContent("已切换到备用模型 route");
    expect(progress).toHaveTextContent(/下次观察/);
    for (const forbiddenAction of [
      /继续执行/,
      /重试/,
      /重新发送/,
      /回到对话/,
    ]) {
      expect(screen.queryByRole("button", { name: forbiddenAction })).not.toBeInTheDocument();
    }
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
    render(<MessageList messages={messages(true)} streaming={false} cwd={null} />);

    expect(screen.getByTestId("turn-result-snapshot")).toHaveTextContent("已完成");
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
      "需要处理",
    );
  });
});
