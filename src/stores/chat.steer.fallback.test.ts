// SPDX-License-Identifier: Apache-2.0
//
// M261 — 一条插话若永远走不到下一个回合边界（被停掉、长工具卡住、切走会话），
// 它不能就这样消失。这里锁定的是持久副本的两条路径：
//   · 循环真的读走（`steer_applied`）→ 副本记为已送达，不再作为新消息重复送出；
//   · 循环没读走 → 副本保持排队，回合结束后由既有队列送出。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, freshRuntime } from "./chat";

const streamMock = vi.hoisted(() => ({
  handler: undefined as ((event: unknown) => void) | undefined,
  onStream: vi.fn(),
}));
const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
  onStream: streamMock.onStream,
  onSessionUpdated: vi.fn(async () => () => {}),
  sendMessageAnonymous: vi.fn(async () => {}),
}));

const SID = "m261";

function resetStore() {
  useChatStore.setState({
    sessions: [],
    activeSession: {
      id: SID,
      title: "t",
      cwd: "/proj",
      model_id: "m",
      created_at: 0,
      updated_at: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
    },
    runtime: { [SID]: freshRuntime() },
    _unlisten: {},
    _unlistenSessionUpdated: {},
    _streamingMsgId: {},
  });
}

function seedStreaming() {
  useChatStore.setState((s) => ({
    runtime: {
      ...s.runtime,
      [SID]: { ...(s.runtime[SID] ?? freshRuntime()), streaming: true },
    },
  }));
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((call) => call[0] === cmd);
}

describe("M261 — 插话的持久副本", () => {
  beforeEach(() => {
    resetStore();
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "enqueue_queued_dispatch_message" ? { id: "q-1" } : undefined,
    );
    streamMock.handler = undefined;
    streamMock.onStream.mockReset();
    streamMock.onStream.mockImplementation(async (_sessionId, handler) => {
      streamMock.handler = handler;
      return () => {};
    });
  });

  it("接受插话的同时落一条持久副本", async () => {
    seedStreaming();
    await useChatStore.getState().steerRun("改用 chrome channel");

    const enqueued = calls("enqueue_queued_dispatch_message");
    expect(enqueued).toHaveLength(1);
    expect(enqueued[0][1]).toMatchObject({
      sessionId: SID,
      content: "改用 chrome channel",
    });
  });

  it("循环读走插话时把副本记为已送达，避免重复送出", async () => {
    await useChatStore.getState().sendMessage("原始任务", SID);
    await useChatStore.getState().steerRun("别提交这次改动");

    streamMock.handler?.({
      type: "steer_applied",
      message_id: "db-9",
      content: "别提交这次改动",
    });

    await vi.waitFor(() =>
      expect(calls("mark_queued_dispatch_message_delivered")).toHaveLength(1),
    );
    expect(calls("mark_queued_dispatch_message_delivered")[0][1]).toMatchObject({
      sessionId: SID,
      id: "q-1",
    });
  });

  it("这一轮没读到插话时副本保持排队，消息不会消失", async () => {
    await useChatStore.getState().sendMessage("原始任务", SID);
    await useChatStore.getState().steerRun("先别动发布");

    const rootTurnId = [...(useChatStore.getState().runtime[SID]?.messages ?? [])]
      .reverse()
      .find((message) => message.role === "assistant")?.rootTurnId;
    streamMock.handler?.({
      type: "turn_settled",
      run_instance_id: "run-1",
      root_turn_id: rootTurnId,
      objective_id: "objective-1",
      status: "stopped",
    });

    // 没有任何 steer_applied：副本必须还留在队列里，不能被记成已送达——
    // 这一轮结束时消息先走既有的插话回收（立刻重发），而持久副本继续留在
    // 队列里兜住"连 settle 都没发生"的情况（进程退出、切换会话）。
    await vi.waitFor(() => {
      const resent = invokeMock.mock.calls.some(
        (call) =>
          call[0] === "send_message" && JSON.stringify(call[1] ?? {}).includes("先别动发布"),
      );
      const reclaimed = calls("claim_next_queued_dispatch_message").length > 0;
      expect(resent || reclaimed).toBe(true);
    });
    expect(calls("mark_queued_dispatch_message_delivered")).toHaveLength(0);
    expect(calls("enqueue_queued_dispatch_message")[0][1]).toMatchObject({
      content: "先别动发布",
    });
  });
});
