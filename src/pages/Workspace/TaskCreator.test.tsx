// SPDX-License-Identifier: Apache-2.0
//
// Smoke tests for the AI task-decomposition flow:
//   • request → decompose_request_to_tasks → review → create_task_tree
//
// jsdom can't reach the real Tauri backend, so we stub `invoke` and
// assert (1) the modal walks through its phases, (2) the right commands
// are called with the right arguments, (3) user edits to the decomposed
// list survive into the create_task_tree payload.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// vi.mock is hoisted; share state with the test body via vi.hoisted so the
// factory can capture our mocks instead of crashing on TDZ access.
const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  openDialog: vi.fn(),
  createTaskTree: vi.fn().mockResolvedValue(["id-0", "id-1"]),
  loadTasks: vi.fn(),
  subscribe: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("../../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openDialog,
}));

vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({
    activeSession: { id: "s1", cwd: "/Users/x/proj", title: "proj" },
    messages: [],
    streaming: false,
    queue: [],
    activeModel: "anthropic/claude-opus-4-7",
    selectSession: vi.fn(),
    sendMessage: vi.fn(),
    sendOrQueue: vi.fn(),
    removeFromQueue: vi.fn(),
    cancelStream: vi.fn(),
    pendingPermission: null,
    respondPermission: vi.fn(),
    updateActiveSessionModel: vi.fn(),
    inputTokenTotal: 0,
    outputTokenTotal: 0,
  }),
}));
// Stub ModelPicker — it pulls in a lot of provider state we don't care about
vi.mock("../../components/ModelPicker", () => ({
  ModelPicker: () => null,
}));
vi.mock("../../components/MessageList", () => ({
  MessageList: () => null,
}));
vi.mock("../../components/MessageInput", () => ({
  MessageInput: () => null,
}));
vi.mock("../../components/PermissionDialog", () => ({
  PermissionDialog: () => null,
}));
vi.mock("../../components/ContextUsageBar", () => ({
  ContextUsageBar: () => null,
}));
vi.mock("../../stores/settings", () => ({
  useSettingsStore: () => ({
    settings: { theme: "dark", permissions: { full_access: false } },
    setTheme: vi.fn(),
  }),
}));
// Two call shapes: the object-destructure form `const {tasks,...} = useTasksStore()`
// (TasksColumn) and the selector form `useTasksStore(s => s.foo)` (ExecutionStream).
// Detect by argument arity and respond appropriately.
const fakeState = {
  tasks: {} as Record<string, unknown[]>,
  running: {} as Record<string, boolean>,
  executionLog: {} as Record<string, unknown[]>,
  loadTasks: mocks.loadTasks,
  subscribe: mocks.subscribe,
  createTaskTree: mocks.createTaskTree,
  start: vi.fn(),
  cancel: vi.fn(),
};
vi.mock("../../stores/tasks", () => ({
  useTasksStore: Object.assign(
    <T,>(selector?: (s: typeof fakeState) => T): T | typeof fakeState =>
      selector ? selector(fakeState) : fakeState,
    { setState: vi.fn(), getState: () => fakeState },
  ),
}));
vi.mock("../../stores/skills", () => ({
  useSkillsStore: () => ({ skills: [], loadSkills: vi.fn() }),
}));

import { WorkspacePage } from "./WorkspacePage";

const sampleLibrary = {
  id: "kb-1",
  name: "历史方案库",
  root_path: "/Users/x/Knowledge",
  enabled: true,
  created_at: "2026-05-26T00:00:00Z",
  last_scan_at: "2026-05-26T00:01:00Z",
  scan_status: "ready",
};

describe("AI task decomposition flow", () => {

  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.openDialog.mockReset();
    mocks.openDialog.mockResolvedValue(null);
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      return Promise.resolve(undefined);
    });
    mocks.createTaskTree.mockClear();
  });

  it("describes → decomposes → reviews → creates task tree end-to-end", async () => {
    const user = userEvent.setup();

    mocks.invoke.mockImplementation((cmd: string, args: { request?: string }) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      if (cmd === "decompose_request_to_tasks") {
        expect(args.request).toBe("做一个本地记账 app");
        return Promise.resolve([
          { tmp_id: "t-0", title: "搭项目骨架", description: "Vite + React", dependencies: [] },
          { tmp_id: "t-1", title: "做数据模型",   description: "SQLite schema", dependencies: ["t-0"] },
        ]);
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} />,
    );

    // Open the modal via the empty-state CTA
    const cta = await screen.findByText(/点这里描述需求/);
    await user.click(cta);

    // Phase 1: input
    const textarea = await screen.findByPlaceholderText(/例如：做一个本地记账/);
    await user.type(textarea, "做一个本地记账 app");

        const decomposeBtn = screen.getByRole("button", { name: "AI 拆解" });
    await user.click(decomposeBtn);

    // Phase 2 (decomposing) is transient — wait for phase 3 (review)
    await waitFor(() => screen.getByText(/审核并确认任务/));

    // Both AI-suggested tasks appear, editable
    expect(screen.getByDisplayValue("搭项目骨架")).toBeInTheDocument();
    expect(screen.getByDisplayValue("做数据模型")).toBeInTheDocument();
    expect(screen.getByText("任务上下文")).toBeInTheDocument();

    // User edits the first task's title
    const titleInput = screen.getByDisplayValue("搭项目骨架");
    await user.clear(titleInput);
    await user.type(titleInput, "搭骨架与 CI");

    // Confirm
    const confirmBtn = screen.getByRole("button", { name: /创建 2 个任务/ });
    await user.click(confirmBtn);

    // createTaskTree called with edited title + correct deps
    await waitFor(() => expect(mocks.createTaskTree).toHaveBeenCalledTimes(1));
    const [sessionId, tasks, deps, context] = mocks.createTaskTree.mock.calls[0];
    expect(sessionId).toBe("s1");
    expect(tasks).toHaveLength(2);
    expect(tasks[0]).toMatchObject({
      tmp_id: "t-0",
      title: "搭骨架与 CI",  // ← edited value preserved
      cwd: "/Users/x/proj",
    });
    expect(deps).toEqual([
      { task_tmp_id: "t-1", depends_on_tmp_id: "t-0" },
    ]);
    expect(context).toEqual({
      knowledge_libraries: [
        {
          id: "kb-1",
          name: "历史方案库",
          root_path: "/Users/x/Knowledge",
          scan_status: "ready",
          last_scan_at: "2026-05-26T00:01:00Z",
        },
      ],
    });
  });

  it("user can remove a decomposed task before confirming", async () => {
    const user = userEvent.setup();

    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      if (cmd === "decompose_request_to_tasks") {
        return Promise.resolve([
          { tmp_id: "t-0", title: "Task A", description: "A", dependencies: [] },
          { tmp_id: "t-1", title: "Task B", description: "B", dependencies: [] },
        ]);
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} />,
    );

    await user.click(await screen.findByText(/点这里描述需求/));
    await user.type(await screen.findByPlaceholderText(/例如：做一个本地记账/), "x");
    await user.click(screen.getByRole("button", { name: "AI 拆解" }));
    await waitFor(() => screen.getByText(/审核并确认任务/));

    // Remove Task A's row (find by its title input then click sibling Trash button)
    const taskARow = screen.getByDisplayValue("Task A").closest("li");
    expect(taskARow).toBeTruthy();
    const removeBtn = within(taskARow as HTMLElement).getByTitle("移除");
    fireEvent.click(removeBtn);

    expect(screen.queryByDisplayValue("Task A")).toBeNull();
    expect(screen.getByDisplayValue("Task B")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /创建 1 个任务/ }));
    await waitFor(() => expect(mocks.createTaskTree).toHaveBeenCalledTimes(1));
    const [, tasks] = mocks.createTaskTree.mock.calls[0];
    expect(tasks).toHaveLength(1);
    expect(tasks[0].title).toBe("Task B");
  });

  it("shows knowledge libraries as task connectors and scans on demand", async () => {
    const user = userEvent.setup();

    mocks.invoke.mockImplementation((cmd: string, args: { libraryId?: string }) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      if (cmd === "scan_knowledge_library") {
        expect(args.libraryId).toBe("kb-1");
        return Promise.resolve({
          library_id: "kb-1",
          scanned_files: 3,
          indexed_documents: 2,
          failed_documents: 1,
          chunks_indexed: 16,
        });
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} />,
    );

    const library = await screen.findByText("历史方案库");
    expect(library).toBeInTheDocument();
    expect(screen.getByText("1 个知识库")).toBeInTheDocument();

    const scanButton = screen.getByTitle("扫描知识库");
    await user.click(scanButton);

    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("scan_knowledge_library", {
        libraryId: "kb-1",
      });
    });
    expect(await screen.findByText("2 文档 / 16 片段")).toBeInTheDocument();

    await user.click(await screen.findByText(/点这里描述需求/));
    expect(screen.getByText("知识库 1")).toBeInTheDocument();
  });

});
