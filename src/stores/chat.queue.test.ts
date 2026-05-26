// SPDX-License-Identifier: Apache-2.0
//
// Message-queue regression tests. These exercise the USER FLOW, not the
// hook's internal state — per AGENTS.md, "单元测试通过 ≠ 用户体验正确".
// We model real interactions: typing while streaming, removing queued
// items, drain on done.
//
// Scope limit: the actual send_message Tauri call is mocked. What we
// verify is the queue STATE MACHINE (sendOrQueue → enqueue → drain
// auto-fire) — the IPC contract is covered separately in Rust tests.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, QUEUE_MAX } from "./chat";
import { reduceChatStreamEvent } from "./chatEvents";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
  onStream: vi.fn(async () => () => {}),
  onSessionUpdated: vi.fn(async () => () => {}),
}));

function resetStore() {
  useChatStore.setState({
    sessions: [],
    activeSession: {
      id: "s1",
      title: "t",
      cwd: "/proj",
      model_id: "m",
      created_at: 0,
      updated_at: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
    },
    messages: [],
    streaming: false,
    queue: [],
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
  });
}

describe("chat message queue", () => {
  beforeEach(() => {
    resetStore();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
  });

  it("sendOrQueue fires immediately when idle", async () => {
    const result = await useChatStore.getState().sendOrQueue("hello");
    expect(result).toBe("sent");
    expect(useChatStore.getState().queue).toHaveLength(0);
    // send_message was called via the underlying sendMessage path.
    expect(invokeMock).toHaveBeenCalledWith("send_message", expect.objectContaining({ content: "hello" }));
  });

  it("sendOrQueue enqueues while streaming, returns 'queued'", async () => {
    useChatStore.setState({ streaming: true });
    const result = await useChatStore.getState().sendOrQueue("queued one");
    expect(result).toBe("queued");
    expect(useChatStore.getState().queue).toHaveLength(1);
    expect(useChatStore.getState().queue[0].content).toBe("queued one");
    // No send call yet — we deferred.
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  it("enforces QUEUE_MAX, returning 'full' on overflow", async () => {
    useChatStore.setState({ streaming: true });
    for (let i = 0; i < QUEUE_MAX; i++) {
      const r = await useChatStore.getState().sendOrQueue(`msg ${i}`);
      expect(r).toBe("queued");
    }
    const overflow = await useChatStore.getState().sendOrQueue("nope");
    expect(overflow).toBe("full");
    expect(useChatStore.getState().queue).toHaveLength(QUEUE_MAX);
  });

  it("ignores empty/whitespace input without enqueueing", async () => {
    useChatStore.setState({ streaming: true });
    await useChatStore.getState().sendOrQueue("   ");
    await useChatStore.getState().sendOrQueue("");
    expect(useChatStore.getState().queue).toHaveLength(0);
  });

  it("removeFromQueue drops the right entry; clearQueue empties everything", async () => {
    useChatStore.setState({ streaming: true });
    await useChatStore.getState().sendOrQueue("a");
    await useChatStore.getState().sendOrQueue("b");
    await useChatStore.getState().sendOrQueue("c");
    const middle = useChatStore.getState().queue[1].id;
    useChatStore.getState().removeFromQueue(middle);
    const ids = useChatStore.getState().queue.map((q) => q.content);
    expect(ids).toEqual(["a", "c"]);
    useChatStore.getState().clearQueue();
    expect(useChatStore.getState().queue).toHaveLength(0);
  });

  it("reducer's done event flips streaming to false (drain trigger contract)", () => {
    // The drain itself lives in chat.ts's handleStreamEvent — it inspects
    // state before/after the reducer. Here we just verify the reducer
    // contract that drain depends on.
    const before = { ...useChatStore.getState(), streaming: true };
    const after = reduceChatStreamEvent(
      before as any,
      { type: "done", input_tokens: 10, output_tokens: 20 },
      "msg-1",
    );
    expect(after.streaming).toBe(false);
  });

});
