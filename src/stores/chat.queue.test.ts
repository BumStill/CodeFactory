// SPDX-License-Identifier: Apache-2.0
//
// Message-queue regression tests. These exercise the USER FLOW, not the
// hook's internal state — per AGENTS.md, "单元测试通过 ≠ 用户体验正确".
// We model real interactions: typing while streaming, removing queued
// items, drain on done. The queue now lives in the ACTIVE session's
// per-session runtime bucket (multiple sessions can stream concurrently).
//
// Scope limit: the actual send_message Tauri call is mocked. What we
// verify is the queue STATE MACHINE (sendOrQueue → enqueue → drain
// auto-fire) — the IPC contract is covered separately in Rust tests.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, QUEUE_MAX, freshRuntime, type SessionRuntime } from "./chat";
import { reduceChatStreamEvent } from "./chatEvents";

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

const SID = "s1";

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

/** Merge a patch into the active session's runtime bucket. */
function seed(patch: Partial<SessionRuntime>) {
  useChatStore.setState((s) => ({
    runtime: { ...s.runtime, [SID]: { ...(s.runtime[SID] ?? freshRuntime()), ...patch } },
  }));
}
/** The active session's queue. */
function q() {
  return useChatStore.getState().runtime[SID]?.queue ?? [];
}

function settle(
  status: "completed" | "cancelled" | "waiting_system" | "waiting_user" | "failed_setup" = "completed",
) {
  streamMock.handler?.({
    type: "turn_settled",
    run_instance_id: "run-1",
    root_turn_id: "root-1",
    objective_id: "objective-1",
    status,
  });
}

describe("chat message queue (per-session)", () => {
  beforeEach(() => {
    resetStore();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    streamMock.handler = undefined;
    streamMock.onStream.mockReset();
    streamMock.onStream.mockImplementation(async (_sessionId, handler) => {
      streamMock.handler = handler;
      return () => {};
    });
  });

  it("sendOrQueue fires immediately when idle", async () => {
    const result = await useChatStore.getState().sendOrQueue("hello");
    expect(result).toBe("sent");
    expect(q()).toHaveLength(0);
    // send_message was called via the underlying sendMessage path.
    expect(invokeMock).toHaveBeenCalledWith("send_message", expect.objectContaining({ content: "hello" }));
  });

  it("sendOrQueue enqueues while streaming, returns 'queued'", async () => {
    seed({ streaming: true });
    const result = await useChatStore.getState().sendOrQueue("queued one");
    expect(result).toBe("queued");
    expect(q()).toHaveLength(1);
    expect(q()[0].content).toBe("queued one");
    // No send call yet — we deferred.
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  it("enforces QUEUE_MAX, returning 'full' on overflow", async () => {
    seed({ streaming: true });
    for (let i = 0; i < QUEUE_MAX; i++) {
      const r = await useChatStore.getState().sendOrQueue(`msg ${i}`);
      expect(r).toBe("queued");
    }
    const overflow = await useChatStore.getState().sendOrQueue("nope");
    expect(overflow).toBe("full");
    expect(q()).toHaveLength(QUEUE_MAX);
  });

  it("ignores empty/whitespace input without enqueueing", async () => {
    seed({ streaming: true });
    await useChatStore.getState().sendOrQueue("   ");
    await useChatStore.getState().sendOrQueue("");
    expect(q()).toHaveLength(0);
  });

  it("removeFromQueue drops the right entry; clearQueue empties everything", async () => {
    seed({ streaming: true });
    await useChatStore.getState().sendOrQueue("a");
    await useChatStore.getState().sendOrQueue("b");
    await useChatStore.getState().sendOrQueue("c");
    const middle = q()[1].id;
    useChatStore.getState().removeFromQueue(middle);
    const contents = q().map((x) => x.content);
    expect(contents).toEqual(["a", "c"]);
    useChatStore.getState().clearQueue();
    expect(q()).toHaveLength(0);
  });


  it("drains the next queued message only after an interrupted turn is durably settled", async () => {
    await useChatStore.getState().sendMessage("first turn", SID);
    await useChatStore.getState().sendOrQueue("queued after stop");
    expect(q().map((x) => x.content)).toEqual(["queued after stop"]);

    invokeMock.mockClear();
    await useChatStore.getState().cancelStream();

    expect(invokeMock).toHaveBeenCalledWith("cancel_chat", { sessionId: SID });
    expect(q()).toHaveLength(1);
    expect(useChatStore.getState().runtime[SID]?.streaming).toBe(true);

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());

    streamMock.handler?.({ type: "done", input_tokens: 0, output_tokens: 0 });
    expect(q()).toHaveLength(1);
    expect(useChatStore.getState().runtime[SID]?.streaming).toBe(true);
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());

    settle("cancelled");
    expect(q()).toHaveLength(0);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ sessionId: SID, content: "queued after stop" }),
    );
  });

  it("only turn_settled flips streaming and clears a pending permission", () => {
    const before: SessionRuntime = {
      ...freshRuntime(),
      streaming: true,
      pendingPermission: {
        intentId: "intent-tool-1",
        toolCallId: "tool-1",
        toolName: "bash",
        args: { command: "git status" },
      },
    };
    const afterTransportDone = reduceChatStreamEvent(
      before,
      { type: "done", input_tokens: 10, output_tokens: 20 },
      "msg-1",
    );
    expect(afterTransportDone.streaming).toBe(true);
    expect(afterTransportDone.pendingPermission?.toolCallId).toBe("tool-1");
    expect(afterTransportDone.inputTokenTotal).toBe(10);
    expect(afterTransportDone.outputTokenTotal).toBe(20);

    const afterSettlement = reduceChatStreamEvent(
      afterTransportDone,
      {
        type: "turn_settled",
        run_instance_id: "run-1",
        root_turn_id: "root-1",
        objective_id: "objective-1",
        status: "completed",
      },
      "msg-1",
    );
    expect(afterSettlement.streaming).toBe(false);
    expect(afterSettlement.pendingPermission).toBeNull();
  });

  it("rolls back CHAT_RUN_BUSY bubbles and restores the rejected content to the queue", async () => {
    invokeMock.mockRejectedValueOnce(new Error("CHAT_RUN_BUSY: run already owns this session"));

    await useChatStore.getState().sendMessage("do not lose me", SID);

    const runtime = useChatStore.getState().runtime[SID];
    // CHAT_RUN_BUSY proves another backend run still owns the session. Keep
    // waiting for that run's settlement instead of declaring the chat idle.
    expect(runtime?.streaming).toBe(true);
    expect(runtime?.messages).toEqual([]);
    expect(runtime?.queue.map((item) => item.content)).toEqual(["do not lose me"]);
    const firstRootTurnId = runtime?.queue[0]?.rootTurnId;
    expect(firstRootTurnId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
    );
    expect(invokeMock).toHaveBeenCalledWith("send_message", {
      sessionId: SID,
      content: "do not lose me",
      rootTurnId: firstRootTurnId,
    });
    expect(useChatStore.getState()._streamingMsgId[SID]).toBeUndefined();
  });

  it("binds an idle and queued persisted message to one stable root identity", async () => {
    await useChatStore.getState().sendMessage("stable idle root", SID);
    const idleArgs = invokeMock.mock.calls.find(([cmd]) => cmd === "send_message")?.[1] as
      | Record<string, unknown>
      | undefined;
    expect(idleArgs?.rootTurnId).toBe(
      useChatStore.getState().runtime[SID]?.messages.find((message) => message.role === "user")?.id,
    );

    seed({ streaming: true, queue: [] });
    await useChatStore.getState().sendOrQueue("stable queued root");
    const queued = useChatStore.getState().runtime[SID]?.queue[0];
    expect(queued?.rootTurnId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
    );
  });

  it("keeps permission state and reports a typed recovery when stop IPC fails", async () => {
    await useChatStore.getState().sendMessage("long turn", SID);
    seed({
      pendingPermission: {
        intentId: "intent-tool-stop",
        toolCallId: "tool-stop",
        toolName: "bash",
        args: { command: "sleep 10" },
      },
    });
    invokeMock.mockRejectedValueOnce(new Error("database unavailable"));

    await useChatStore.getState().cancelStream();

    const runtime = useChatStore.getState().runtime[SID];
    const assistant = runtime?.messages[runtime.messages.length - 1];
    expect(runtime?.streaming).toBe(true);
    expect(runtime?.pendingPermission?.toolCallId).toBe("tool-stop");
    expect(assistant?.content).toContain("停止请求未送达");
    expect(assistant?.runtimeError).toEqual(
      expect.objectContaining({ code: "CHAT_STOP_FAILED", recoverable: true }),
    );
  });

  it("runs postmortem only for completed settlement backed by transport done", async () => {
    seed({
      messages: [{ id: "old", role: "user", content: "earlier", createdAt: 0 }],
    });
    await useChatStore.getState().sendMessage("first attempt", SID);
    invokeMock.mockClear();

    streamMock.handler?.({ type: "error", message: "recoverable transport failure" });
    settle("completed");

    expect(invokeMock).not.toHaveBeenCalledWith("mine_cross_session_patterns", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("run_postmortem", expect.anything());

    await useChatStore.getState().sendMessage("second attempt", SID);
    invokeMock.mockClear();
    streamMock.handler?.({ type: "done", input_tokens: 1, output_tokens: 2 });
    settle("completed");

    expect(invokeMock).toHaveBeenCalledWith("mine_cross_session_patterns", { cwd: "/proj" });
  });
});
