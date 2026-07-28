// SPDX-License-Identifier: Apache-2.0
//
// Mid-run steering, at the store level: the USER FLOW of typing while the
// agent is working. The queue path is covered in chat.queue.test.ts; what
// matters here is that a steer is never silently lost — it either lands in
// the running turn, or it becomes the next one.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, freshRuntime, type SessionRuntime } from "./chat";

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

function seed(patch: Partial<SessionRuntime>) {
  useChatStore.setState((s) => ({
    runtime: { ...s.runtime, [SID]: { ...(s.runtime[SID] ?? freshRuntime()), ...patch } },
  }));
}
function messages() {
  return useChatStore.getState().runtime[SID]?.messages ?? [];
}
function queue() {
  return useChatStore.getState().runtime[SID]?.queue ?? [];
}

describe("steering a run in flight", () => {
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

  it("shows the steer immediately as undelivered and queues it for the loop", async () => {
    seed({ streaming: true });
    await useChatStore.getState().steerRun("改用 chrome channel");

    expect(invokeMock).toHaveBeenCalledWith("queue_interjection", {
      sessionId: SID,
      message: "改用 chrome channel",
    });
    expect(messages()).toHaveLength(1);
    expect(messages()[0]).toMatchObject({
      role: "user",
      content: "改用 chrome channel",
      steerPending: true,
    });
    // It is NOT a new turn — no send_message.
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  it("adds no pending bubble when the interjection is bound for the task scheduler", async () => {
    // An autonomous run with no chat stream: the scheduler drains the queue at
    // its own boundary and never emits steer_applied, so a pending bubble here
    // would hang forever with nothing able to resolve it.
    seed({ streaming: false });
    await useChatStore.getState().steerRun("下一个任务改用 chrome channel");

    expect(invokeMock).toHaveBeenCalledWith("queue_interjection", {
      sessionId: SID,
      message: "下一个任务改用 chrome channel",
    });
    expect(messages()).toHaveLength(0);
  });

  it("removes the bubble when the backend refuses it, rather than lying", async () => {
    seed({ streaming: true });
    invokeMock.mockRejectedValueOnce(new Error("no such session"));

    await expect(useChatStore.getState().steerRun("试试")).rejects.toThrow("no such session");
    expect(messages()).toHaveLength(0);
  });

  it("re-sends a steer the loop never reached as the next turn", async () => {
    await useChatStore.getState().sendMessage("原始任务", SID);
    await useChatStore.getState().steerRun("等一下，别提交");
    expect(messages().some((m) => m.steerPending)).toBe(true);

    invokeMock.mockClear();
    // The turn ends before any round boundary drained it.
    streamMock.handler?.({ type: "done", input_tokens: 0, output_tokens: 0 });
    await new Promise((resolve) => setTimeout(resolve, 0));

    // The undelivered bubble is gone, replaced by a real turn carrying it.
    expect(messages().some((m) => m.steerPending)).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ sessionId: SID, content: "等一下，别提交" }),
    );
  });

  it("keeps several undelivered steers in order, the rest queued behind the first", async () => {
    await useChatStore.getState().sendMessage("原始任务", SID);
    await useChatStore.getState().steerRun("第一条");
    await useChatStore.getState().steerRun("第二条");
    await useChatStore.getState().steerRun("第三条");

    invokeMock.mockClear();
    streamMock.handler?.({ type: "done", input_tokens: 0, output_tokens: 0 });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ content: "第一条" }),
    );
    expect(queue().map((item) => item.content)).toEqual(["第二条", "第三条"]);
  });

  it("leaves a confirmed steer alone when the turn ends", async () => {
    await useChatStore.getState().sendMessage("原始任务", SID);
    await useChatStore.getState().steerRun("已经送到了");
    streamMock.handler?.({
      type: "steer_applied",
      message_id: "db-1",
      content: "已经送到了",
    });
    expect(messages().some((m) => m.steerPending)).toBe(false);

    invokeMock.mockClear();
    streamMock.handler?.({ type: "done", input_tokens: 0, output_tokens: 0 });
    await new Promise((resolve) => setTimeout(resolve, 0));

    // Already delivered — re-sending it would duplicate the instruction.
    expect(invokeMock).not.toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ content: "已经送到了" }),
    );
  });
});
