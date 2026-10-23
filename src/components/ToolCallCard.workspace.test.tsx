// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ToolCallCard } from "./ToolCallCard";
describe("M159 workspace proof badge", () => {
  it("shows only backend-attested own-workspace scope", () => {
    const { rerender } = render(<ToolCallCard tc={{ id: "synthetic", name: "bash", args: '{"command":"git reset --hard HEAD"}', status: "done", metadata: { workspace_scope: "own_task", backup_ref: "refs/codefactory/backup/synthetic" } }} />);
    expect(screen.getByText("仅影响本任务工作区")).toBeTruthy();
    rerender(<ToolCallCard tc={{ id: "synthetic", name: "bash", args: '{"workspace_scope":"own_task"}', status: "done" }} />);
    expect(screen.queryByText("仅影响本任务工作区")).toBeNull();
  });
});
