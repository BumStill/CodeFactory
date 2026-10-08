// SPDX-License-Identifier: Apache-2.0

// M31 rewrote the card's contract: the verdict comes from the backend's
// authoritative objective status, and the card never speaks internal
// control-loop vocabulary. The assertions below were updated to that spec —
// the previous "证据待复核 / 当前边界" wording is exactly what the task removed.

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
  it("forms a completed result footer with changes and summary controls", () => {
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence(tools)}
        objectiveStatus="completed"
        durationMs={80_000}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveAttribute("data-status-tone", "success");
    expect(screen.getByText("已完成")).toBeInTheDocument();
    expect(screen.getByText(/3\/3/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "查看改动" }));
    expect(screen.getByText("src/App.tsx")).toBeInTheDocument();
    expect(screen.getByText(/pnpm test/)).toBeInTheDocument();
    expect(screen.getByText("等待 · 等待 CI")).toBeInTheDocument();
    expect(screen.getByText("没有失败的操作")).toBeInTheDocument();

    expect(screen.queryByRole("button", { name: "执行过程" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "结果摘要" }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "完成 3/3 个计划步骤；改动 1 个文件；运行 1 项检查；没有失败的操作。",
    );
  });

  it("keeps a completed objective completed even when a tool call failed mid-turn", () => {
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
        objectiveStatus="completed"
        durationMs={80_000}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveAttribute("data-status-tone", "success");
    expect(result).toHaveTextContent("已完成");
    expect(result).toHaveTextContent("6/6");
    expect(result).not.toHaveTextContent("需要处理");
    expect(result.querySelector("[class*='text-status-success']")).not.toBeNull();
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

  it("opens the shared changes pane without also expanding inline detail", () => {
    const onOpenEvidence = vi.fn();
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence(tools)}
        objectiveStatus="completed"
        durationMs={80_000}
        onOpenEvidence={onOpenEvidence}
        evidenceControlsId="workspace-auxiliary-pane"
        evidenceOpen
      />,
    );

    const trigger = screen.getByRole("button", { name: "查看改动" });
    expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
    expect(trigger).toHaveAttribute("aria-controls", "workspace-auxiliary-pane");
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(trigger);
    expect(onOpenEvidence).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("改动的文件")).not.toBeInTheDocument();
  });

  it("says a turn boundary failure did not finish without internal wording", () => {
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence(tools)}
        turnBoundaryFailure
        durationMs={80_000}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveAttribute("data-verdict", "incomplete");
    expect(result).toHaveTextContent("还没做完");
    expect(result).not.toHaveTextContent("证据");
    expect(result).toHaveTextContent("这次没有全部做完");
    // The boundary failure is a turn-level signal, not a failed tool call, so
    // the tool list stays honest about there being none.
    fireEvent.click(screen.getByRole("button", { name: "查看改动" }));
    expect(screen.getByText("没有失败的操作")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "结果摘要" }));
    expect(screen.getByText(/没有失败的操作。/)).toBeInTheDocument();
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

  it("redacts credentials from bounded file and command evidence", () => {
    const evidence = summarizeTurnEvidence([
      {
        id: "secret-edit",
        name: "edit_file",
        args: JSON.stringify({ path: "fixtures/key=PLAINTEXTMARKER1" }),
        status: "done",
        result: "ok",
      },
      {
        id: "secret-test",
        name: "bash",
        args: JSON.stringify({ command: "API_KEY=PLAINTEXTMARKER2 pnpm test" }),
        status: "done",
        result: "passed",
      },
    ]);

    const serialized = JSON.stringify(evidence);
    expect(serialized).not.toContain("PLAINTEXTMARKER1");
    expect(serialized).not.toContain("PLAINTEXTMARKER2");
    expect(serialized).toContain("[redacted]");

    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={evidence}
        objectiveStatus="completed"
        durationMs={1_000}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "查看改动" }));

    const snapshot = screen.getByTestId("turn-result-snapshot");
    expect(snapshot).not.toHaveTextContent(/PLAINTEXTMARKER/);
    expect(snapshot).toHaveTextContent("[redacted]");
  });
});
