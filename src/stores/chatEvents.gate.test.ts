// SPDX-License-Identifier: Apache-2.0
//
// Completion-gate stream-event tests (vitest). The legacy chatEvents.test.ts
// uses a hand-rolled harness that vitest excludes; new reducer coverage goes
// here.

import { describe, it, expect } from "vitest";
import { reduceChatStreamEvent, type ChatEventState } from "./chatEvents";

function baseState(): ChatEventState {
  return {
    messages: [
      {
        id: "assistant-1",
        role: "assistant",
        content: "",
        toolCalls: [],
        createdAt: 1,
      },
    ],
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

describe("completion gate stream events", () => {
  it("clears rejected drafts and internal work when recovery starts", () => {
    const state = baseState();
    state.messages[0] = {
      ...state.messages[0],
      content: "unrelated candidate answer",
      toolCalls: [{ id: "t1", name: "bash", args: "{}", status: "done", result: "ok" }],
      segments: [
        { kind: "text", text: "unrelated candidate answer" },
        { kind: "tool", toolCallId: "t1" },
      ],
    };

    const next = reduceChatStreamEvent(
      state,
      {
        type: "completion_gate_action",
        kind: "recovery",
        detail: "background services require a later probe",
      },
      "assistant-1",
    );

    expect(next.messages[0]).toEqual(
      expect.objectContaining({
        content: "",
        toolCalls: [],
        segments: [],
        internalReviewState: "recovery",
        internalReviewDraft: "",
      }),
    );
    expect(next.messages[0].gateActions).toBeUndefined();
    expect(next.streaming).toBe(true);

    const narrated = reduceChatStreamEvent(
      next,
      { type: "text_delta", content: "后台服务已运行，现在做内部探针。" },
      "assistant-1",
    );
    expect(narrated.messages[0].content).toBe("");
    expect(narrated.messages[0].segments).toEqual([]);
    expect(narrated.messages[0].internalReviewDraft).toBe(
      "后台服务已运行，现在做内部探针。",
    );

    const toolStarted = reduceChatStreamEvent(
      narrated,
      { type: "tool_call_start", id: "hidden-tool", name: "bash", args: {} },
      "assistant-1",
    );
    expect(toolStarted.messages[0].toolCalls).toEqual([]);
    expect(toolStarted.messages[0].internalReviewDraft).toBe("");
  });

  it("clears verification chatter at ready so only the final reply follows", () => {
    let state = baseState();
    state.messages[0] = {
      ...state.messages[0],
      content: "later client probe passed",
      toolCalls: [{ id: "t2", name: "bash", args: "{}", status: "done", result: "passed" }],
      segments: [{ kind: "text", text: "later client probe passed" }],
    };
    state = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "recovery", detail: "verify" },
      "assistant-1",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "text_delta", content: "这段验证旁白不应显示。" },
      "assistant-1",
    );
    expect(state.messages[0].content).toBe("");
    state = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "ready", detail: "" },
      "assistant-1",
    );
    expect(state.messages[0].internalReviewState).toBe("finalizing");
    const finalState = reduceChatStreamEvent(
      state,
      { type: "text_delta", content: "已完成：拆任务已内置到当前会话。" },
      "assistant-1",
    );

    expect(finalState.messages[0].content).toBe("已完成：拆任务已内置到当前会话。");
    expect(finalState.messages[0].toolCalls).toEqual([]);
    expect(finalState.messages[0].segments).toEqual([
      { kind: "text", text: "已完成：拆任务已内置到当前会话。" },
    ]);
  });

  it("keeps only the final answer plus a user-facing warning when verification is incomplete", () => {
    const state = baseState();
    state.messages[0] = {
      ...state.messages[0],
      content: "正在运行内部检查。最终回答：功能已经内置到会话。",
      toolCalls: [{ id: "t3", name: "bash", args: "{}", status: "error", result: "failed" }],
      segments: [
        { kind: "text", text: "正在运行内部检查。" },
        { kind: "tool", toolCallId: "t3" },
        { kind: "text", text: "最终回答：功能已经内置到会话。" },
      ],
    };

    let recovering = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "recovery", detail: "verify" },
      "assistant-1",
    );
    recovering = reduceChatStreamEvent(
      recovering,
      { type: "text_delta", content: "最终回答：功能已经内置到会话。" },
      "assistant-1",
    );
    expect(recovering.messages[0].content).toBe("");

    const next = reduceChatStreamEvent(
      recovering,
      {
        type: "completion_gate_action",
        kind: "warning",
        detail: "⚠ 以上回复未经完整验证：仍有一项检查未通过。",
      },
      "assistant-1",
    );

    expect(next.messages[0].content).toBe("最终回答：功能已经内置到会话。");
    expect(next.messages[0].toolCalls).toEqual([]);
    expect(next.messages[0].segments).toEqual([
      { kind: "text", text: "最终回答：功能已经内置到会话。" },
    ]);
    expect(next.messages[0].gateActions).toEqual([
      { kind: "warning", detail: "⚠ 以上回复未经完整验证：仍有一项检查未通过。" },
    ]);
  });

  it("replaces an internal recovery error with a concise user-facing failure", () => {
    let state = reduceChatStreamEvent(
      baseState(),
      { type: "completion_gate_action", kind: "recovery", detail: "verify" },
      "assistant-1",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "text_delta", content: "internal verification narration" },
      "assistant-1",
    );
    const next = reduceChatStreamEvent(
      state,
      { type: "error", message: "Completion blocked: unresolved probe fingerprint" },
      "assistant-1",
    );

    expect(next.streaming).toBe(false);
    expect(next.messages[0].content).toBe("本次处理未能完成，请重试。");
    expect(next.messages[0].segments).toEqual([
      { kind: "text", text: "本次处理未能完成，请重试。" },
    ]);
    expect(next.messages[0].content).not.toMatch(/Completion|probe|Error/);
  });

  it("leaves other messages untouched", () => {
    const state = baseState();
    state.messages.push({
      id: "assistant-2",
      role: "assistant",
      content: "old",
      createdAt: 2,
    });
    const next = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "recovery", detail: "x" },
      "assistant-2",
    );
    expect(next.messages[0].content).toBe("");
    expect(next.messages[1].content).toBe("");
  });
});
