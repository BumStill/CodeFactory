// SPDX-License-Identifier: Apache-2.0
//
// ExecutionStream rendering tests. We poke the zustand store directly to
// add ExecutionEvents and assert the component renders the right cards.
// This catches: missing event kinds, broken grouping, broken icons/colors,
// and accidental empty-state regressions when there ARE events.

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { ExecutionStream } from "./ExecutionStream";
import { useTasksStore, type ExecutionEvent } from "../stores/tasks";

function ev(over: Partial<ExecutionEvent> = {}): ExecutionEvent {
  return {
    id: Math.random().toString(36),
    kind: "task_progress",
    taskId: "t1",
    at: Date.now(),
    ...over,
  };
}

function pushEvents(sessionId: string, events: ExecutionEvent[]) {
  useTasksStore.setState((s) => ({
    executionLog: { ...s.executionLog, [sessionId]: events },
  }));
}

describe("ExecutionStream", () => {
  beforeEach(() => {
    useTasksStore.setState({ executionLog: {}, running: {} });
  });

  it("renders nothing when the log is empty (default)", () => {
    const { container } = render(<ExecutionStream sessionId="s1" />);
    expect(container.firstChild).toBeNull();
  });

  it("groups events by taskId and renders a card per task", () => {
    pushEvents("s1", [
      ev({ kind: "task_started", taskId: "t1", title: "Build UI" }),
      ev({ kind: "task_progress", taskId: "t1", message: "Editing App.tsx" }),
      ev({ kind: "task_started", taskId: "t2", title: "Update tests" }),
      ev({ kind: "task_progress", taskId: "t1", message: "Wired button" }),
      ev({ kind: "task_completed", taskId: "t2", result: "Tests green" }),
    ]);

    render(<ExecutionStream sessionId="s1" />);

    expect(screen.getByText("Build UI")).toBeInTheDocument();
    expect(screen.getByText("Update tests")).toBeInTheDocument();
    expect(screen.getByText(/Editing App.tsx/)).toBeInTheDocument();
    expect(screen.getByText(/Wired button/)).toBeInTheDocument();
    expect(screen.getByText(/Tests green/)).toBeInTheDocument();

    // Total event count badge: 5 events
    expect(screen.getByText("5 条事件")).toBeInTheDocument();
  });

  it("shows failure state when the latest terminal event is a failure", () => {
    pushEvents("s1", [
      ev({ kind: "task_started", taskId: "t1", title: "Doomed task" }),
      ev({ kind: "task_failed", taskId: "t1", error: "tests blew up" }),
    ]);
    render(<ExecutionStream sessionId="s1" />);
    expect(screen.getByText(/tests blew up/)).toBeInTheDocument();
  });

  it("marks header as 运行中 when scheduler is running", () => {
    pushEvents("s1", [ev({ kind: "task_started", taskId: "t1", title: "Live" })]);
    useTasksStore.setState((s) => ({ running: { ...s.running, s1: true } }));
    render(<ExecutionStream sessionId="s1" />);
    expect(screen.getByText("运行中")).toBeInTheDocument();
  });

  it("renders a live per-criterion verification summary (passed/total)", () => {
    pushEvents("s1", [
      ev({ kind: "task_started", taskId: "t1", title: "Verify me" }),
      ev({
        kind: "task_verification",
        taskId: "t1",
        verification: [
          { check: "cargo test", passed: true, output: "", duration_ms: 10 },
          { check: "tsc", passed: true, output: "", duration_ms: 5 },
          { check: "lint", passed: false, output: "oops", duration_ms: 3 },
        ],
      }),
    ]);
    render(<ExecutionStream sessionId="s1" />);
    expect(screen.getByText("验证 · 2/3 通过")).toBeInTheDocument();
  });
});
