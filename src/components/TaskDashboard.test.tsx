// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { TaskDashboard } from "./TaskDashboard";
import { useTasksStore } from "../stores/tasks";
import type { TaskRun } from "../lib/tauri";

vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return { ...actual, invoke: vi.fn().mockResolvedValue([]) };
});

// The dashboard subscribes to task + evidence streams on mount.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

const SESSION = "s1";

function task(id: string, status: TaskRun["status"]): TaskRun {
  return {
    id,
    session_id: SESSION,
    title: `task ${id}`,
    description: "",
    status,
    cwd: "/tmp/x",
    parent_task_id: null,
    sub_session_id: null,
    created_at: "2026-08-03T00:00:00Z",
    started_at: null,
    completed_at: null,
    result: null,
    error: null,
    attempt_count: 0,
    verification_results: null,
    task_context_json: null,
  };
}

/// Put the store in the exact state that used to render the manual start
/// button: tasks exist and are not yet settled, and the scheduler is not
/// currently marked running in this window.
function seedActiveNotRunning() {
  useTasksStore.setState({
    tasks: { [SESSION]: [task("t1", "pending"), task("t2", "pending")] },
    loading: { [SESSION]: false },
    running: { [SESSION]: false },
    error: {},
    resumeReports: {},
  });
}

describe("TaskDashboard", () => {
  beforeEach(() => {
    seedActiveNotRunning();
  });

  // `delegate_tasks` spawns the scheduler itself and returns
  // "execution_started" (src-tauri/src/tools/delegate_tasks.rs). A manual start
  // control therefore never advances anything the system was not already
  // doing — but pressing it starts a SECOND runner that competes with the
  // conversation's own agent for the session, which is how it produced
  // mid-turn interruptions. There is nothing for the user to start.
  it("offers no manual start control when tasks are delegated but not yet running", () => {
    render(<TaskDashboard sessionId={SESSION} cwd="/tmp/x" onClose={() => {}} />);
    expect(screen.queryByRole("button", { name: /开始/ })).toBeNull();
  });

  it("explains that execution is automatic instead of leaving the area blank", () => {
    render(<TaskDashboard sessionId={SESSION} cwd="/tmp/x" onClose={() => {}} />);
    expect(screen.getByText(/自动执行/)).toBeTruthy();
  });

  // Cancelling is a real user decision and must survive.
  it("still offers cancel while the scheduler is running", () => {
    useTasksStore.setState({ running: { [SESSION]: true } });
    render(<TaskDashboard sessionId={SESSION} cwd="/tmp/x" onClose={() => {}} />);
    expect(screen.getByRole("button", { name: /取消/ })).toBeTruthy();
  });
});
