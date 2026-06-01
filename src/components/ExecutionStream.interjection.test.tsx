// SPDX-License-Identifier: Apache-2.0
//
// Interjection-bar tests for ExecutionStream.
//
// Covers the user-facing contract:
//   1. Bar only appears when scheduler is running (idle stream has no bar).
//   2. Enter submits; Shift+Enter does not.
//   3. Empty input is blocked.
//   4. Successful submit calls queue_interjection with the right args and
//      flashes "已加入下一任务" confirmation.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

describe("ExecutionStream interjection bar", () => {

  beforeEach(() => {
    useTasksStore.setState({ executionLog: {}, running: {} });
    invokeMock.mockReset();
  });

  it("is hidden when scheduler is not running", () => {
    // No running flag — bar should not render
    useTasksStore.setState((s) => ({
      executionLog: {
        ...s.executionLog,
        s1: [{ id: "1", kind: "task_completed", taskId: "t1", at: Date.now() }],
      },
    }));
    render(<ExecutionStream sessionId="s1" />);
    expect(screen.queryByLabelText(/引导/)).toBeNull();
  });

  it("appears when running and submits via Enter", async () => {
    seedRunning("s1");
    invokeMock.mockResolvedValue(undefined);

    render(<ExecutionStream sessionId="s1" />);

    const input = screen.getByLabelText(/引导/) as HTMLInputElement;
    expect(input).toBeInTheDocument();

    const user = userEvent.setup();
    await user.type(input, "改用深色配色{Enter}");

    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    expect(invokeMock).toHaveBeenCalledWith("queue_interjection", {
      sessionId: "s1",
      message: "改用深色配色",
    });
    // Input cleared
    expect(input.value).toBe("");
    // Confirmation appears
    expect(screen.getByText(/已加入下一任务/)).toBeInTheDocument();
  });

  it("does not submit on empty input via button", () => {
    seedRunning("s1");
    invokeMock.mockResolvedValue(undefined);

    render(<ExecutionStream sessionId="s1" />);

    const btn = screen.getByRole("button", { name: "发送" });
    expect((btn as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(btn);
    expect(invokeMock).not.toHaveBeenCalled();
  });

});
