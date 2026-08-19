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
function settle() {
  streamMock.handler?.({
    type: "turn_settled",
    run_instance_id: "run-1",
    root_turn_id: "root-1",
    objective_id: "objective-1",
    status: "completed",
  });
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
    // Transport ends before any round boundary drained it, but the steer must
    // remain attached until durable settlement says the run is truly over.
    streamMock.handler?.({ type: "done", input_tokens: 0, output_tokens: 0 });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(messages().some((m) => m.steerPending)).toBe(true);
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());

    settle();
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
    expect(messages().filter((m) => m.steerPending)).toHaveLength(3);
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());

    settle();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ content: "第一条" }),
    );
    expect(queue().map((item) => item.content)).toEqual(["第二条", "第三条"]);
  });

  it("lands the bubble in position while it is still WAITING, not once delivered", async () => {
    // The reported state: the turn was still running, the steer had not been
    // drained yet, and output kept piling above the bubble — 一直在最下边.
    // The bubble must take its place the moment it is said.
    await useChatStore.getState().sendMessage("原始任务", SID);
    streamMock.handler?.({ type: "text_delta", content: "先做第一步" });
    await useChatStore.getState().steerRun("改用 chrome channel");

    // Still pending — nothing has confirmed it.
    expect(messages().find((m) => m.steerPending)?.content).toBe("改用 chrome channel");

    // Work produced during the wait belongs BELOW it.
    streamMock.handler?.({ type: "text_delta", content: "还在跑上一件事" });
    expect(messages().map((m) => [m.role, m.content])).toEqual([
      ["user", "原始任务"],
      ["assistant", "先做第一步"],
      ["user", "改用 chrome channel"],
      ["assistant", "还在跑上一件事"],
    ]);
  });

  it("puts work done after a steer BELOW it, not above", async () => {
    // Field report: the live view read backwards. One growing assistant bubble
    // plus a steer appended after it meant everything the agent did in
    // response to the steer rendered above the steer that caused it.
    await useChatStore.getState().sendMessage("原始任务", SID);
    streamMock.handler?.({ type: "text_delta", content: "先做第一步" });
    await useChatStore.getState().steerRun("改用 chrome channel");
    streamMock.handler?.({
      type: "steer_applied",
      message_id: "db-1",
      content: "改用 chrome channel",
    });
    streamMock.handler?.({ type: "text_delta", content: "好的，改用 chrome channel" });

    const shape = messages().map((m) => [m.role, m.content]);
    expect(shape).toEqual([
      ["user", "原始任务"],
      ["assistant", "先做第一步"],
      ["user", "改用 chrome channel"],
      ["assistant", "好的，改用 chrome channel"],
    ]);
    const rootTurnId = messages().find((message) => message.role === "user")?.id;
    expect(
      messages()
        .filter((message) => message.role === "assistant")
        .map((message) => message.rootTurnId),
    ).toEqual([rootTurnId, rootTurnId]);
  });

  it("keeps feeding the new bubble after the split, not the closed one", async () => {
    await useChatStore.getState().sendMessage("原始任务", SID);
    streamMock.handler?.({ type: "text_delta", content: "第一段" });
    await useChatStore.getState().steerRun("换个方向");
    streamMock.handler?.({ type: "steer_applied", message_id: null, content: "换个方向" });
    streamMock.handler?.({ type: "text_delta", content: "第二段" });
    streamMock.handler?.({ type: "text_delta", content: "继续" });

    const assistants = messages().filter((m) => m.role === "assistant");
    expect(assistants.map((m) => m.content)).toEqual(["第一段", "第二段继续"]);
    // The closed bubble is settled, not left looking mid-flight.
    expect(assistants[0].durationMs).toBeGreaterThanOrEqual(0);
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
