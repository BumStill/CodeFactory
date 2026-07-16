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
    quickSessions: [],
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


  it("drains the next queued message only after an interrupted turn reaches terminal state", async () => {
    await useChatStore.getState().sendMessage("first turn", SID);
    await useChatStore.getState().sendOrQueue("queued after stop");
    expect(q().map((x) => x.content)).toEqual(["queued after stop"]);

    invokeMock.mockClear();
    useChatStore.getState().cancelStream();

    expect(invokeMock).toHaveBeenCalledWith("cancel_chat", { sessionId: SID });
    expect(q()).toHaveLength(1);
    expect(useChatStore.getState().runtime[SID]?.streaming).toBe(true);

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());

    streamMock.handler?.({ type: "done", input_tokens: 0, output_tokens: 0 });
    expect(q()).toHaveLength(0);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ sessionId: SID, content: "queued after stop" }),
    );
  });

  it("reducer's done event flips streaming to false (drain trigger contract)", () => {
    // The drain itself lives in chat.ts's handleStreamEvent — it inspects the
    // session's runtime before/after the reducer. Here we just verify the
    // reducer contract that drain depends on.
    const before: SessionRuntime = { ...freshRuntime(), streaming: true };
    const after = reduceChatStreamEvent(
      before,
      { type: "done", input_tokens: 10, output_tokens: 20 },
      "msg-1",
    );
    expect(after.streaming).toBe(false);
  });
});
