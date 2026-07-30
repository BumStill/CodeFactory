// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TurnPlan } from "../lib/chatPlan";
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

    fireEvent.click(screen.getByRole("button", { name: "执行过程" }));
    expect(onToggleProcess).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "结果摘要" }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "完成 3/3 个计划步骤；修改 1 个文件；执行 1 项验证；没有失败证据。",
    );
  });

  it("never presents an incomplete result with success semantics", () => {
    const incompletePlan: TurnPlan = {
      ...plan,
      waitingReason: "缺少 live verifier",
      steps: [
        { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
        { id: "deliver", title: "正式发布", kind: "delivery", status: "in_progress" },
        { id: "live", title: "线上验证", kind: "verification", status: "pending" },
      ],
    };

    render(
      <TurnResultSnapshot
        plan={incompletePlan}
        evidence={summarizeTurnEvidence([
          ...tools,
          {
            id: "blocked",
            name: "bash",
            args: JSON.stringify({ command: "pnpm release:verify" }),
            status: "blocked",
            result: "missing live verifier",
            isError: true,
          },
        ])}
        durationMs={80_000}
        processExpanded={false}
      />,
    );

    const result = screen.getByTestId("turn-result-snapshot");
    expect(result).toHaveAttribute("data-status-tone", "warning");
    expect(result).toHaveTextContent("需要处理");
    expect(result).toHaveTextContent("缺少 live verifier");
    expect(result.querySelector(".lucide-circle-check-big")).toBeNull();
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
