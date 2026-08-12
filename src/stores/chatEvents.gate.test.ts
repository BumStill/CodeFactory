// SPDX-License-Identifier: Apache-2.0
//
// Completion-gate stream-event tests (vitest). The legacy chatEvents.test.ts
// uses a hand-rolled harness that vitest excludes; new reducer coverage goes
// here.
//
// Contract: the gate is a control loop, not a chat participant. Recovery and
// ready rounds change what the model does next and nothing about what the user
// sees — no erasure, no progress card, no framework vocabulary on screen.

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
  it("keeps every step the turn already produced when recovery starts", () => {
    const state = baseState();
    state.messages[0] = {
      ...state.messages[0],
      content: "candidate answer",
      toolCalls: [{ id: "t1", name: "bash", args: "{}", status: "done", result: "ok" }],
      segments: [
        { kind: "text", text: "candidate answer" },
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

    expect(next.messages[0]).toEqual(state.messages[0]);
    expect(next.streaming).toBe(true);
  });

  it("keeps the recovery round's work visible and starts a fresh final answer", () => {
    let state = baseState();
    state.messages[0] = {
      ...state.messages[0],
      content: "candidate answer",
      toolCalls: [{ id: "t1", name: "bash", args: "{}", status: "done", result: "ok" }],
      segments: [
        { kind: "text", text: "candidate answer" },
        { kind: "tool", toolCallId: "t1" },
      ],
    };
    state = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "recovery", detail: "verify" },
      "assistant-1",
    );
    // A recovery round always runs a tool before it may speak again
    // (`require_tool_next`), so its work lands in the timeline as usual.
    state = reduceChatStreamEvent(
      state,
      { type: "tool_call_start", id: "t2", name: "bash", args: { command: "npm test" } },
      "assistant-1",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "tool_result", tool_call_id: "t2", content: "313 passed", is_error: false, status: "done" },
      "assistant-1",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "text_delta", content: "已完成：测试全绿。" },
      "assistant-1",
    );

    expect(state.messages[0].toolCalls?.map((tc) => tc.id)).toEqual(["t1", "t2"]);
    expect(state.messages[0].toolCalls?.[1].result).toBe("313 passed");
    // The tool segment between them means the final answer is its own segment,
    // never concatenated onto the rejected draft.
    expect(state.messages[0].segments).toEqual([
      { kind: "text", text: "candidate answer" },
      { kind: "tool", toolCallId: "t1" },
      { kind: "tool", toolCallId: "t2" },
      { kind: "text", text: "已完成：测试全绿。" },
    ]);
  });

  it("treats ready as a no-op for the transcript", () => {
    const state = baseState();
    state.messages[0] = {
      ...state.messages[0],
      content: "probe passed",
      segments: [{ kind: "text", text: "probe passed" }],
    };

    const next = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "ready", detail: "" },
      "assistant-1",
    );

    expect(next.messages[0]).toEqual(state.messages[0]);
  });

  it("appends the unverified warning without collapsing the turn", () => {
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

    const next = reduceChatStreamEvent(
      state,
      {
        type: "completion_gate_action",
        kind: "warning",
        detail: "⚠ 以上回复未经完整验证：仍有一项检查未通过。",
      },
      "assistant-1",
    );

    expect(next.messages[0].content).toBe(state.messages[0].content);
    expect(next.messages[0].toolCalls).toEqual(state.messages[0].toolCalls);
    expect(next.messages[0].segments).toEqual(state.messages[0].segments);
    expect(next.messages[0].gateActions).toEqual([
      { kind: "warning", detail: "⚠ 以上回复未经完整验证：仍有一项检查未通过。" },
    ]);
  });

  it("keeps the completed steps when the turn errors out mid-recovery", () => {
    let state = baseState();
    state = reduceChatStreamEvent(
      state,
      { type: "tool_call_start", id: "t4", name: "bash", args: {} },
      "assistant-1",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "recovery", detail: "verify" },
      "assistant-1",
    );
    const next = reduceChatStreamEvent(
      state,
      { type: "error", message: "503 Service Unavailable" },
      "assistant-1",
    );

    expect(next.streaming).toBe(true);
    expect(next.messages[0].toolCalls?.map((tc) => tc.id)).toEqual(["t4"]);
    expect(next.messages[0].content).toContain("503 Service Unavailable");
    expect(next.messages[0].durationMs).toBeUndefined();

    const settled = reduceChatStreamEvent(
      next,
      {
        type: "turn_settled",
        run_instance_id: "run-recovery-1",
        root_turn_id: "root-recovery-1",
        objective_id: "objective-recovery-1",
        status: "waiting_system",
      },
      "assistant-1",
    );
    expect(settled.streaming).toBe(false);
    expect(settled.messages[0].durationMs).toBeGreaterThanOrEqual(0);
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
    expect(next.messages[1].content).toBe("old");
  });
});
