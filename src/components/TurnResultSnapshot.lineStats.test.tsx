// SPDX-License-Identifier: Apache-2.0
//
// M36: "最好有个地方显示当前这次修改总和多少" — the result card must show this
// turn's total in one line, counting only the edits that actually landed.

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { summarizeTurnEvidence, TurnResultSnapshot } from "./TurnResultSnapshot";
import type { TurnPlan } from "../lib/chatPlan";
import type { ToolCallState } from "../stores/chatEvents";

const plan: TurnPlan = {
  rootTurnId: "root-1",
  revision: 1,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: [],
  createdAt: 1,
  steps: [{ id: "step-1", title: "改代码", kind: "implementation", status: "completed" }],
};

function edit(id: string, path: string, oldString: string, newString: string): ToolCallState {
  return {
    id,
    name: "edit_file",
    args: JSON.stringify({ path, old_string: oldString, new_string: newString }),
    status: "done",
    result: "ok",
  };
}

describe("summarizeTurnEvidence line totals", () => {
  it("sums multiple edits across multiple files", () => {
    const evidence = summarizeTurnEvidence([
      edit("1", "src/a.ts", "a\nb\nc", "a\nB\nc"),
      edit("2", "src/b.ts", "x", "x\ny\nz\nw"),
      edit("3", "src/a.ts", "1\n2\n3", "1\n2\n3"),
    ]);
    expect(evidence.changedFileCount).toBe(2);
    expect(evidence.addedLines).toBe(4);
    expect(evidence.removedLines).toBe(1);
  });

  it("does not count failed or denied edits", () => {
    const evidence = summarizeTurnEvidence([
      { ...edit("ok", "src/a.ts", "a", "a\nb"), status: "done" },
      { ...edit("err", "src/b.ts", "a\nb\nc", "a"), isError: true },
      { ...edit("denied", "src/c.ts", "a", "a\nb\nc"), status: "denied" },
      { ...edit("blocked", "src/d.ts", "a", "a\nb"), status: "blocked" },
    ]);
    expect(evidence.addedLines).toBe(1);
    expect(evidence.removedLines).toBe(0);
  });

  it("counts a created file's lines but not an overwrite's", () => {
    const created: ToolCallState = {
      id: "w1",
      name: "write_file",
      args: JSON.stringify({ path: "docs/new.md", content: "a\nb\nc\n" }),
      status: "done",
      result: "Written 6 bytes to docs/new.md\n\n```diff\n@@ -0,0 +1,3 @@\n+a\n```",
    };
    const overwrite: ToolCallState = {
      id: "w2",
      name: "write_file",
      args: JSON.stringify({ path: "docs/old.md", content: "a\nb\n" }),
      status: "done",
      result: "Written 4 bytes to docs/old.md\n\n```diff\n@@ -1,4 +1,2 @@\n-a\n```",
    };
    const evidence = summarizeTurnEvidence([created, overwrite]);
    expect(evidence.addedLines).toBe(3);
    expect(evidence.removedLines).toBe(0);
  });
});

describe("TurnResultSnapshot turn total", () => {
  const evidence = summarizeTurnEvidence([
    edit("1", "src/a.ts", "a\nb\nc", "a\nB\nc"),
    edit("2", "src/b.ts", "x", "x\ny\nz\nw"),
  ]);

  it("shows this turn's files and total in one line", () => {
    render(<TurnResultSnapshot plan={plan} evidence={evidence} objectiveStatus="completed" durationMs={1000} />);
    const line = screen.getByTestId("turn-line-summary");
    const text = line.textContent ?? "";
    expect(text).toContain("本次改了 2 个文件");
    expect(text).toContain("+4");
    expect(text).toContain("−1");
  });

  it("opens the existing changes view when clicked", () => {
    const onOpenEvidence = vi.fn();
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={evidence}
        objectiveStatus="completed"
        durationMs={1000}
        onOpenEvidence={onOpenEvidence}
      />,
    );
    screen.getByTestId("turn-line-summary").click();
    expect(onOpenEvidence).toHaveBeenCalledTimes(1);
  });

  it("shows nothing when the turn changed nothing", () => {
    const readOnly: ToolCallState = {
      id: "r1",
      name: "read_file",
      args: JSON.stringify({ path: "src/a.ts" }),
      status: "done",
      result: "contents",
    };
    render(
      <TurnResultSnapshot
        plan={plan}
        evidence={summarizeTurnEvidence([readOnly])}
        objectiveStatus="completed"
        durationMs={1000}
      />,
    );
    expect(screen.queryByTestId("turn-line-summary")).toBeNull();
  });
});
