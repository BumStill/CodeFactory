// SPDX-License-Identifier: Apache-2.0
//
// ExecutionStream must remain display-only. Autonomous guidance belongs in
// the primary MessageInput so users do not see two competing input fields.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { useTasksStore, type ExecutionEvent } from "../stores/tasks";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({ invoke: invokeMock }));

import { ExecutionStream } from "./ExecutionStream";

function seedRunning(sessionId: string) {
  const ev: ExecutionEvent = {
    id: "1",
    kind: "task_started",
    taskId: "t1",
    title: "T",
    at: Date.now(),
  };
  useTasksStore.setState((s) => ({
    executionLog: { ...s.executionLog, [sessionId]: [ev] },
    running: { ...s.running, [sessionId]: true },
  }));
}

describe("ExecutionStream guidance surface", () => {

  beforeEach(() => {
    useTasksStore.setState({ executionLog: {}, running: {} });
    invokeMock.mockReset();
  });

  it("does not render a duplicate guidance input while running", () => {
    seedRunning("s1");
    render(<ExecutionStream sessionId="s1" />);
    expect(screen.getByText("运行中")).toBeInTheDocument();
    expect(screen.queryByLabelText("引导下一步")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
