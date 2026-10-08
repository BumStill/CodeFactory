// SPDX-License-Identifier: Apache-2.0

// M31: the result card at the end of a task must say in plain words what
// happened, where the work is, and what comes next. These tests pin the
// user-visible contract; the vocabulary guard mirrors the backend's
// `assert_no_internal_vocabulary` plus the card-specific words the task names.

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PlanStep, TurnPlan } from "../lib/chatPlan";
import type { ToolCallState } from "../stores/chatEvents";
import { summarizeTurnEvidence, TurnResultSnapshot } from "./TurnResultSnapshot";

/** Words that must never reach the user through the card. */
const BANNED_WORDS = [
  // Card-specific internals named by the task.
  "证据",
  "复核",
  "当前边界",
  "失败证据",
  "中断证据",
  // Kept as close to the backend `assert_no_internal_vocabulary` list as
  // practical: these must not leak into the card either.
  "恢复耗尽",
  "安全上限",
  "系统故障",
  "已登记",
  "能力更新",
  "incident",
  "objective",
  "remediation",
  "generation",
  "recovery",
];

function assertNoBannedWords(text: string) {
  const lowered = text.toLowerCase();
  for (const word of BANNED_WORDS) {
    expect(lowered).not.toContain(word.toLowerCase());
  }
}

const untrackedPlan: TurnPlan = {
  rootTurnId: "root",
  revision: 1,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: [],
  createdAt: 100,
  steps: Array.from({ length: 5 }, (_, index): PlanStep => ({
    id: `step-${index + 1}`,
    title: `步骤 ${index + 1}`,
    kind: "implementation",
    status: "pending",
  })),
};

function editedFiles(count: number): ToolCallState[] {
  return Array.from({ length: count }, (_, index): ToolCallState => ({
    id: `edit-${index}`,
    name: "edit_file",
    args: JSON.stringify({ path: `src/changed-${index + 1}.ts` }),
    status: "done",
    result: "ok",
  }));
}

describe("TurnResultSnapshot plain-language verdict (M31)", () => {
  it("(a) objective failed + no PR + 6 changed files: says it did not get done, where the changes are, and what is next", () => {
    const evidence = summarizeTurnEvidence(editedFiles(6));
    expect(evidence.changedFileCount).toBe(6);

    render(
      <TurnResultSnapshot
        plan={untrackedPlan}
        evidence={evidence}
        objectiveStatus="failed"
        turnBoundaryFailure
        durationMs={880_000}
      />,
    );

    const card = screen.getByTestId("turn-result-snapshot");
    // Consistent with the plain-language summary message: it did not get done.
    expect(card).toHaveTextContent("没做成");
    expect(card).not.toHaveTextContent("已完成");
    // Where the work is.
    expect(card).toHaveTextContent(/6 个文件/);
    expect(card).toHaveTextContent(/保存在本次会话/);
    // What comes next.
    expect(card).toHaveTextContent(/继续/);
    // No misleading progress number when the plan was never tracked.
    expect(card).not.toHaveTextContent("0/5");
    assertNoBannedWords(card.textContent ?? "");
  });

  it("(b) objective completed with a PR: says it is done and links the PR", () => {
    const evidence = summarizeTurnEvidence([
      ...editedFiles(2),
      {
        id: "deliver",
        name: "deliver_changes",
        args: JSON.stringify({ title: "fix: thing" }),
        status: "done",
        result:
          "Delivered. PR opened: https://github.com/BumStill/CodeFactory/pull/568",
      },
    ]);

    render(
      <TurnResultSnapshot
        plan={{ ...untrackedPlan, steps: [], revision: 2 }}
        evidence={evidence}
        objectiveStatus="completed"
        durationMs={120_000}
      />,
    );

    const card = screen.getByTestId("turn-result-snapshot");
    expect(card).toHaveAttribute("data-status-tone", "success");
    expect(card).toHaveTextContent("已完成");
    expect(card).toHaveTextContent("PR #568");
    const link = screen.getByRole("link", { name: /PR #568/ });
    expect(link).toHaveAttribute(
      "href",
      "https://github.com/BumStill/CodeFactory/pull/568",
    );
    assertNoBannedWords(card.textContent ?? "");
  });

  it("(c) a blocked tool call the agent later recovered from does not turn the card into a warning", () => {
    const evidence = summarizeTurnEvidence([
      {
        id: "blocked-commit",
        name: "bash",
        args: JSON.stringify({ command: "git commit -m wip" }),
        status: "blocked",
        result: "use deliver_changes instead",
      },
      {
        id: "deliver",
        name: "deliver_changes",
        args: JSON.stringify({}),
        status: "done",
        result: "https://github.com/BumStill/CodeFactory/pull/569",
      },
    ]);
    expect(evidence.failureCount).toBeGreaterThan(0);

    render(
      <TurnResultSnapshot
        plan={untrackedPlan}
        evidence={evidence}
        objectiveStatus="completed"
        durationMs={90_000}
      />,
    );

    const card = screen.getByTestId("turn-result-snapshot");
    expect(card).toHaveAttribute("data-status-tone", "success");
    expect(card).toHaveTextContent("已完成");
    assertNoBannedWords(card.textContent ?? "");
  });

  it("(d) never renders internal vocabulary in failed, completed or waiting states", () => {
    const waitingPlan: TurnPlan = {
      ...untrackedPlan,
      waitingReason: "等待 CI 检查完成",
      nextActionOwner: "external",
    };
    const states: Array<{ objectiveStatus: string | null; plan: TurnPlan }> = [
      { objectiveStatus: "failed", plan: untrackedPlan },
      { objectiveStatus: "completed", plan: untrackedPlan },
      { objectiveStatus: null, plan: waitingPlan },
    ];

    for (const state of states) {
      const { unmount } = render(
        <TurnResultSnapshot
          plan={state.plan}
          evidence={summarizeTurnEvidence(editedFiles(3))}
          objectiveStatus={state.objectiveStatus}
          durationMs={60_000}
        />,
      );
      const card = screen.getByTestId("turn-result-snapshot");
      assertNoBannedWords(card.textContent ?? "");
      // The expandable result sections must stay clean too.
      const toggle = screen.queryByRole("button", { name: /查看/ });
      if (toggle) {
        fireEvent.click(toggle);
        assertNoBannedWords(card.textContent ?? "");
      }
      unmount();
    }
  });
});
