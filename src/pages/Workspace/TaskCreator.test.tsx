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
  start: vi.fn(),
  retryFailedTasks: vi.fn(),
}));

vi.mock("../../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openDialog,
}));

// Chat store mock supports BOTH call shapes: the no-arg destructure form
// (WorkspacePage: `const {activeSession,...} = useChatStore()`) and the
// selector form (SessionSidebar: `useChatStore(s => s.sessions)`).
const fakeChatState = {
  sessions: [] as unknown[],
  quickSessions: [] as unknown[],
  activeSession: { id: "s1", cwd: "/Users/x/proj", title: "proj" },
  messages: [] as unknown[],
  streaming: false,
  queue: [] as unknown[],
  activeModel: "anthropic/claude-opus-4-7",
  selectSession: vi.fn(),
  sendMessage: vi.fn(),
  sendOrQueue: vi.fn(),
  removeFromQueue: vi.fn(),
  cancelStream: vi.fn(),
  pendingPermission: null,
  respondPermission: vi.fn(),
  updateActiveSessionModel: vi.fn(),
  createSession: vi.fn(),
  loadSessions: vi.fn(),
  loadQuickSessions: vi.fn(),
  inputTokenTotal: 0,
  outputTokenTotal: 0,
};
// The active session's per-session runtime slice (what `activeRuntime` returns).
const fakeChatRuntime = {
  messages: fakeChatState.messages,
  streaming: fakeChatState.streaming,
  queue: fakeChatState.queue,
  inputTokenTotal: fakeChatState.inputTokenTotal,
  outputTokenTotal: fakeChatState.outputTokenTotal,
  pendingPermission: fakeChatState.pendingPermission,
  contextUsage: null,
  compressionToast: null,
};
vi.mock("../../stores/chat", () => ({
  useChatStore: Object.assign(
    <T,>(selector?: (s: typeof fakeChatState) => T): T | typeof fakeChatState =>
      selector ? selector(fakeChatState) : fakeChatState,
    { setState: vi.fn(), getState: () => fakeChatState },
  ),
  // WorkspacePage reads the active session's slice via `useChatStore(activeRuntime)`.
  activeRuntime: () => fakeChatRuntime,
}));
// Stub ModelPicker — it pulls in a lot of provider state we don't care about
vi.mock("../../components/ModelPicker", () => ({
  ModelPicker: () => null,
}));
// Stub CheckpointsPanel — it calls Tauri listen() + list_checkpoints, neither
// of which exists in jsdom; this flow doesn't exercise checkpoints.
vi.mock("../../components/CheckpointsPanel", () => ({
  CheckpointsPanel: () => null,
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
  retryFailedTasks: mocks.retryFailedTasks,
  start: mocks.start,
  cancel: vi.fn(),
};
vi.mock("../../stores/tasks", () => ({
  useTasksStore: Object.assign(
    <T,>(selector?: (s: typeof fakeState) => T): T | typeof fakeState =>
      selector ? selector(fakeState) : fakeState,
    { setState: vi.fn(), getState: () => fakeState },
  ),
}));
// Learning store calls Tauri listen() inside subscribe() — that fails in
// jsdom because window.__TAURI_INTERNALS__ doesn't exist. Stub the whole
// store so ConnectorsColumn renders cleanly without trying to set up a
// real Tauri event listener.
const fakeLearningState = {
  events: {} as Record<string, unknown[]>,
  loading: {} as Record<string, boolean>,
  load: vi.fn(async () => []),
  subscribe: vi.fn(async () => () => {}),
  accept: vi.fn(async () => {}),
  reject: vi.fn(async () => {}),
};
vi.mock("../../stores/learning", () => ({
  useLearningStore: Object.assign(
    <T,>(selector?: (s: typeof fakeLearningState) => T): T | typeof fakeLearningState =>
      selector ? selector(fakeLearningState) : fakeLearningState,
    { setState: vi.fn(), getState: () => fakeLearningState },
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
    mocks.start.mockClear();
    mocks.retryFailedTasks.mockReset();
    mocks.retryFailedTasks.mockResolvedValue(1);
    fakeState.tasks = {};
    fakeState.running = {};
    fakeState.executionLog = {};
    // Isolate the 自主 toggle between tests. localStorage persists in CI (Node's
    // experimental localStorage) but throws in the local runner — guard it so the
    // autonomous test can't leak its on-state into the reviewed-flow tests.
    try {
      localStorage.clear();
    } catch {
      /* localStorage unavailable in this env — nothing to isolate */
    }
  });

  it("keeps knowledge and skill management out of the session workspace", async () => {
    render(
      <WorkspacePage
        sessionId="s1"
        onBackHome={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />,
    );

    expect(screen.queryByRole("button", { name: "技能库" })).not.toBeInTheDocument();
    expect(screen.queryByText("个人知识库")).not.toBeInTheDocument();
    expect(screen.queryByText("添加知识库")).not.toBeInTheDocument();
    expect(screen.queryByText("没有激活的技能")).not.toBeInTheDocument();
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
          { tmp_id: "t-0", title: "搭项目骨架", description: "Vite + React", dependencies: [], acceptance_criteria: ["pnpm dev 启动成功"] },
          { tmp_id: "t-1", title: "做数据模型",   description: "SQLite schema", dependencies: ["t-0"], acceptance_criteria: ["cargo test schema 通过"] },
        ]);
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
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
    expect(screen.queryByText("任务上下文")).not.toBeInTheDocument();

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
    expect(context).toBeUndefined();
  });

  it("autonomous mode: intent → decompose → create → start, no review modal", async () => {
    const user = userEvent.setup();

    mocks.invoke.mockImplementation((cmd: string, args: { request?: string }) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      if (cmd === "decompose_request_to_tasks") {
        expect(args.request).toBe("加个深色模式");
        return Promise.resolve([
          { tmp_id: "t-0", title: "加深色模式开关", description: "theme toggle", dependencies: [], acceptance_criteria: ["切换即时生效"] },
        ]);
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
    );

    // Flip the 自主 toggle on — the inline bar replaces the modal flow.
    const toggle = await screen.findByRole("button", { name: /自主/ });
    await user.click(toggle);

    // Describe intent inline, then run.
    const bar = await screen.findByLabelText("自主任务描述");
    await user.type(bar, "加个深色模式");
    await user.click(screen.getByRole("button", { name: /自主执行/ }));

    // Full chain fires: decompose → createTaskTree → start — all automatic.
    await waitFor(() => expect(mocks.createTaskTree).toHaveBeenCalledTimes(1));
    // Direct path passes the spec args as undefined (uniform start signature).
    await waitFor(() => expect(mocks.start).toHaveBeenCalledWith("s1", undefined, undefined));

    const [sessionId, tasks] = mocks.createTaskTree.mock.calls[0];
    expect(sessionId).toBe("s1");
    expect(tasks).toHaveLength(1);
    expect(tasks[0]).toMatchObject({
      tmp_id: "t-0",
      title: "加深色模式开关",
      cwd: "/Users/x/proj",
    });

    // The reviewed-flow modal never appeared.
    expect(screen.queryByText(/审核并确认任务/)).not.toBeInTheDocument();
  });

  it("surfaces a repair loop for failed tasks", async () => {
    const user = userEvent.setup();
    fakeState.tasks = {
      s1: [
        {
          id: "task-failed",
          session_id: "s1",
          title: "跑测试并修复失败",
          description: "npm test",
          status: "failed",
          cwd: "/Users/x/proj",
          parent_task_id: null,
          sub_session_id: null,
          created_at: "2026-07-08T00:00:00Z",
          started_at: "2026-07-08T00:00:01Z",
          completed_at: "2026-07-08T00:00:02Z",
          result: null,
          error: "npm test failed",
          attempt_count: 3,
          verification_results: JSON.stringify([
            { check: "npm test", passed: false, output: "expected failure", duration_ms: 12 },
          ]),
          failure_attribution: {
            kind: "verification",
            label: "验收失败",
            summary: "npm test 验收未通过",
            next_action: "读取失败验收项，修实现并重跑同一检查。",
            repairable: true,
            source: "verification_results",
          },
          task_context_json: null,
          spec_req_id: null,
          spec_title: null,
        },
      ],
    };

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
    );

    const repair = await screen.findByRole("button", { name: /修复可修复项/ });
    expect(screen.getByText("验收失败")).toBeInTheDocument();
    expect(screen.getByText(/读取失败验收项/)).toBeInTheDocument();
    await user.click(repair);

    await waitFor(() => expect(mocks.retryFailedTasks).toHaveBeenCalledWith("s1"));
    expect(mocks.start).toHaveBeenCalledWith("s1", undefined, undefined);
  });

  it("does not blindly retry non-repairable provider failures", async () => {
    const user = userEvent.setup();
    fakeState.tasks = {
      s1: [
        {
          id: "task-provider-failed",
          session_id: "s1",
          title: "模型调用失败",
          description: "provider failure",
          status: "failed",
          cwd: "/Users/x/proj",
          parent_task_id: null,
          sub_session_id: null,
          created_at: "2026-07-08T00:00:00Z",
          started_at: "2026-07-08T00:00:01Z",
          completed_at: "2026-07-08T00:00:02Z",
          result: null,
          error: "HTTP 402 Insufficient Balance from provider",
          attempt_count: 1,
          verification_results: null,
          failure_attribution: {
            kind: "model-provider",
            label: "模型/Provider",
            summary: "HTTP 402 Insufficient Balance from provider",
            next_action: "修复 endpoint、API key、余额或模型 route 后再重试。",
            repairable: false,
            source: "error",
          },
          task_context_json: null,
          spec_req_id: null,
          spec_title: null,
        },
      ],
    };

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
    );

    expect(await screen.findByText("模型/Provider")).toBeInTheDocument();
    const repair = screen.getByRole("button", { name: /先处理失败原因/ });
    expect(repair).toBeDisabled();
    await user.click(repair);

    expect(mocks.retryFailedTasks).not.toHaveBeenCalled();
    expect(mocks.start).not.toHaveBeenCalled();
  });

  it("autonomous + 先写规范: writes a spec, decomposes it, links tasks to the spec", async () => {
    const user = userEvent.setup();

    mocks.invoke.mockImplementation((cmd: string, args: Record<string, string>) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      if (cmd === "create_spec") {
        expect(args.cwd).toBe("/Users/x/proj");
        return Promise.resolve({
          meta: {
            req_id: "CF-007",
            title: args.title,
            file_path: "/Users/x/proj/.codefactory/specs/x.md",
            rel_path: ".codefactory/specs/x.md",
            status: "draft",
          },
          content: "",
          body: "",
        });
      }
      if (cmd === "spec_ai_assist") {
        // The real allocated id is injected into the generate prompt.
        expect(args.instruction).toContain("CF-007");
        // Model emits the placeholder id; the chain pins it back to CF-007.
        return Promise.resolve("---\nreq_id: CF-001\ntitle: 大需求\n---\n# Overview\n…");
      }
      if (cmd === "save_spec") {
        expect(args.content).toContain("req_id: CF-007");
        return Promise.resolve({ req_id: "CF-007", title: "做一个大功能", file_path: args.path });
      }
      if (cmd === "decompose_spec_to_tasks") {
        expect(args.specContent).toContain("req_id: CF-007");
        return Promise.resolve([
          { tmp_id: "t-0", title: "实现 X", description: "…", dependencies: [], acceptance_criteria: ["X 通过"] },
        ]);
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
    );

    await user.click(await screen.findByRole("button", { name: "自主" }));
    await user.click(screen.getByRole("button", { name: /先写规范/ }));
    await user.type(await screen.findByLabelText("自主任务描述"), "做一个大功能");
    await user.click(screen.getByRole("button", { name: /写规范并执行/ }));

    // Full spec-first chain, then linked start.
    await waitFor(() => expect(mocks.createTaskTree).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(mocks.start).toHaveBeenCalledWith("s1", "CF-007", "做一个大功能"),
    );

    const cmds = mocks.invoke.mock.calls.map((c: unknown[]) => c[0]);
    expect(cmds).toContain("create_spec");
    expect(cmds).toContain("spec_ai_assist");
    expect(cmds).toContain("save_spec");
    expect(cmds).toContain("decompose_spec_to_tasks");
    // The direct path is bypassed when 先写规范 is on.
    expect(cmds).not.toContain("decompose_request_to_tasks");
  });

  it("user can remove a decomposed task before confirming", async () => {
    const user = userEvent.setup();

    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_knowledge_libraries") {
        return Promise.resolve([sampleLibrary]);
      }
      if (cmd === "decompose_request_to_tasks") {
        return Promise.resolve([
          { tmp_id: "t-0", title: "Task A", description: "A", dependencies: [], acceptance_criteria: [] },
          { tmp_id: "t-1", title: "Task B", description: "B", dependencies: [], acceptance_criteria: [] },
        ]);
      }
      return Promise.resolve(undefined);
    });

    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
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

  it("does not load or manage knowledge libraries from the session", async () => {
    render(
      <WorkspacePage sessionId="s1" onBackHome={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} />,
    );

    await screen.findByText(/点这里描述需求/);
    expect(mocks.invoke).not.toHaveBeenCalledWith("list_knowledge_libraries");
    expect(screen.queryByTitle("扫描知识库")).not.toBeInTheDocument();
    expect(screen.queryByText(/个知识库/)).not.toBeInTheDocument();
  });

});
