// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

import { useTasksStore } from "./tasks";
import type { TaskConnectorContext, TaskInput } from "../lib/tauri";

describe("tasks store", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
    useTasksStore.setState({
      tasks: {},
      running: {},
      loading: {},
      error: {},
      executionLog: {},
    });
  });

  it("sends enabled knowledge connector context with task creation payload", async () => {
    const tasks: TaskInput[] = [
      {
        tmp_id: "t-0",
        title: "生成产品路线图 PPT",
        description: "复用历史方案库",
        cwd: "/Users/x/proj",
      },
    ];
    const context: TaskConnectorContext = {
      knowledge_libraries: [
        {
          id: "kb-1",
          name: "历史方案库",
          root_path: "/Users/x/Knowledge",
          scan_status: "completed",
          last_scan_at: "2026-05-26T00:01:00Z",
        },
      ],
    };
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "create_task_tree") return Promise.resolve(["task-1"]);
      if (cmd === "list_tasks") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    await useTasksStore.getState().createTaskTree("s1", tasks, [], context);

    expect(mocks.invoke).toHaveBeenCalledWith("create_task_tree", {
      sessionId: "s1",
      tasksIn: tasks,
      dependencies: [],
      context,
      specReqId: null,
      specTitle: null,
    });
  });

  it("tags created tasks with their source spec when provided", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "create_task_tree") return Promise.resolve(["task-1"]);
      if (cmd === "list_tasks") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    await useTasksStore
      .getState()
      .createTaskTree("s1", [], [], undefined, "REQ-7", "深色模式");

    expect(mocks.invoke).toHaveBeenCalledWith("create_task_tree", {
      sessionId: "s1",
      tasksIn: [],
      dependencies: [],
      context: null,
      specReqId: "REQ-7",
      specTitle: "深色模式",
    });
  });
});
