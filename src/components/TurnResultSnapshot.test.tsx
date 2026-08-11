// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { PlanStep, TurnPlan } from "../lib/chatPlan";
import type { ToolCallState } from "../stores/chatEvents";
import { summarizeTurnEvidence, TurnResultSnapshot } from "./TurnResultSnapshot";

const plan: TurnPlan = {
  rootTurnId: "root",
  revision: 5,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: ["等待 CI"],
  createdAt: 100,
  steps: [
    { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
    { id: "implement", title: "实现修改", kind: "implementation", status: "completed" },
    { id: "verify", title: "验证", kind: "verification", status: "completed" },
  ],
};

const tools: ToolCallState[] = [
  {
    id: "edit",
    name: "edit_file",
    args: JSON.stringify({ path: "src/App.tsx" }),
    status: "done",
    result: "updated",
  },
  {
    id: "test",
    name: "bash",
    args: JSON.stringify({ command: "pnpm test -- --run src/App.test.tsx" }),
    status: "done",
    result: "3 passed",
  },
];

type NextActionOwner = "system" | "external" | "user";

function withNextActionOwner(
  value: TurnPlan,
  nextActionOwner: NextActionOwner,
): TurnPlan {
  return { ...value, nextActionOwner } as TurnPlan;
}

describe("TurnResultSnapshot", () => {
  it("forms a completed result footer with evidence/process/summary controls", () => {
    const onToggleProcess = vi.fn();
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence(tools)}
        durationMs={80_000}
        processExpanded={false}
        onToggleProcess={onToggleProcess}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveAttribute("data-status-tone", "success");
    expect(screen.getByText("已完成")).toBeInTheDocument();
    expect(screen.getByText(/3\/3/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "查看证据" }));
    expect(screen.getByText("src/App.tsx")).toBeInTheDocument();
    expect(screen.getByText(/pnpm test/)).toBeInTheDocument();
    expect(screen.getByText("等待 · 等待 CI")).toBeInTheDocument();
    expect(screen.getByText("没有失败操作证据")).toBeInTheDocument();

    const processButton = screen.getByRole("button", { name: "执行过程" });
    expect(processButton).toHaveAttribute("aria-expanded", "false");
    expect(processButton).not.toHaveAttribute("aria-pressed");
    fireEvent.click(processButton);
    expect(onToggleProcess).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "结果摘要" }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "完成 3/3 个计划步骤；修改 1 个文件；记录 1 项验证操作；没有失败证据。",
    );
  });

  it("reports completed execution with failed evidence as evidence review, not user action", () => {
    const completedPlan: TurnPlan = {
      ...plan,
      steps: Array.from({ length: 6 }, (_, index): PlanStep => ({
        id: `step-${index + 1}`,
        title: `步骤 ${index + 1}`,
        kind: index === 5 ? "verification" : "implementation",
        status: "completed",
      })),
    };

    render(
      <TurnResultSnapshot
        plan={completedPlan}
        evidence={summarizeTurnEvidence([
          ...tools,
          {
            id: "failed-verification",
            name: "bash",
            args: JSON.stringify({ command: "pnpm release:verify" }),
            status: "error",
            result: "verification failed",
            isError: true,
          },
        ])}
        durationMs={80_000}
        processExpanded={false}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveAttribute("data-status-tone", "warning");
    expect(result).toHaveTextContent("已执行，证据待复核");
    expect(result).toHaveTextContent("6/6");
    expect(result).not.toHaveTextContent("需要处理");
    expect(result.querySelector(".lucide-circle-check-big")).toBeNull();
  });

  it("does not claim failed writes as changed files or failed commands as verification", () => {
    const evidence = summarizeTurnEvidence([
      {
        id: "failed-write",
        name: "write_file",
        args: JSON.stringify({ path: "src/not-written.ts" }),
        status: "error",
        isError: true,
        result: "permission denied",
      },
      {
        id: "failed-test",
        name: "bash",
        args: JSON.stringify({ command: "pnpm test" }),
        status: "error",
        isError: true,
        result: "failed to start",
      },
    ]);

    expect(evidence.changedFileCount).toBe(0);
    expect(evidence.changedFiles).toEqual([]);
    expect(evidence.verificationCount).toBe(0);
    expect(evidence.verificationCommands).toEqual([]);
    expect(evidence.failureCount).toBe(2);
  });

  it("opens the shared evidence pane without also expanding inline evidence", () => {
    const onOpenEvidence = vi.fn();
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence(tools)}
        durationMs={80_000}
        processExpanded={false}
        onOpenEvidence={onOpenEvidence}
        evidenceControlsId="workspace-auxiliary-pane"
        evidenceOpen
      />,
    );

    const trigger = screen.getByRole("button", { name: "查看证据" });
    expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
    expect(trigger).toHaveAttribute("aria-controls", "workspace-auxiliary-pane");
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(trigger);
    expect(onOpenEvidence).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("等待与失败边界")).not.toBeInTheDocument();
  });

  it("counts a turn-boundary failure consistently in evidence and summary", () => {
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence(tools)}
        turnBoundaryFailure
        durationMs={80_000}
        processExpanded={false}
      />,
    );

    expect(screen.getByTestId("turn-result-snapshot")).toHaveTextContent(
      "已执行，证据待复核",
    );
    fireEvent.click(screen.getByRole("button", { name: "查看证据" }));
    expect(screen.getByText("1 项失败或中断证据")).toBeInTheDocument();
    expect(screen.queryByText("没有失败操作证据")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "结果摘要" }));
    expect(screen.getByText(/1 项失败证据/)).toBeInTheDocument();
  });

  it("fails safe for legacy waiting data without assigning work to the user", () => {
    const legacyPlan: TurnPlan = {
      ...plan,
      waitingReason: "等待 CI 检查完成",
      steps: [
        { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
        { id: "deliver", title: "等待 CI", kind: "external_job", status: "in_progress" },
      ],
    };

    render(
      <TurnResultSnapshot
        plan={legacyPlan}
        evidence={summarizeTurnEvidence(tools)}
        durationMs={80_000}
        processExpanded={false}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveTextContent("系统继续处理");
    expect(result).not.toHaveTextContent("需要处理");
    expect(result).toHaveTextContent("等待 CI 检查完成");
  });

  it("shows external waiting without assigning it to the user", () => {
    const externalPlan = withNextActionOwner(
      {
        ...plan,
        waitingReason: "等待 GitHub required checks",
        steps: [
          { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
          { id: "deliver", title: "等待 CI", kind: "external_job", status: "in_progress" },
        ],
      },
      "external",
    );

    render(
      <TurnResultSnapshot
        plan={externalPlan}
        evidence={summarizeTurnEvidence(tools)}
        durationMs={80_000}
        processExpanded={false}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveTextContent("外部等待");
    expect(result).not.toHaveTextContent("需要处理");
  });

  it("shows user action only for a structured user owner", () => {
    const userPlan = withNextActionOwner(
      {
        ...plan,
        waitingReason: "请选择发布窗口",
        steps: [
          { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
          { id: "decide", title: "等待业务裁决", kind: "other", status: "pending" },
        ],
      },
      "user",
    );

    render(
      <TurnResultSnapshot
        plan={userPlan}
        evidence={summarizeTurnEvidence(tools)}
        durationMs={80_000}
        processExpanded={false}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveTextContent("需要你处理");
    expect(result).not.toHaveTextContent("需要处理");
  });

  it("keeps a thousand-event evidence summary bounded", () => {
    const many = Array.from({ length: 1_000 }, (_, index): ToolCallState => ({
      id: `edit-${index}`,
      name: "edit_file",
      args: JSON.stringify({ path: `src/generated-${index}.ts` }),
      status: "done",
      result: "ok",
    }));

    const evidence = summarizeTurnEvidence(many);
    expect(evidence.operationCount).toBe(1_000);
    expect(evidence.changedFileCount).toBe(1_000);
    expect(evidence.changedFiles).toHaveLength(20);
    expect(JSON.stringify(evidence).length).toBeLessThan(4_000);
  });
});
