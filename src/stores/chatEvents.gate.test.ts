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
  it("records gate actions on the streaming assistant message", () => {
    // A completion-gate action must be surfaced instead of silently swallowed
    // (the 2026-07-16 loop was invisible: the user saw seven repeated replies
    // with no explanation).
    const first = reduceChatStreamEvent(
      baseState(),
      {
        type: "completion_gate_action",
        kind: "recovery",
        detail: "at least one successful verification is required",
      },
      "assistant-1",
    );
    const assistant = first.messages[0];
    expect(assistant.gateActions).toHaveLength(1);
    expect(assistant.gateActions![0]).toEqual({
      kind: "recovery",
      detail: "at least one successful verification is required",
    });
    expect(first.streaming).toBe(true);

    const second = reduceChatStreamEvent(
      first,
      { type: "completion_gate_action", kind: "ready", detail: "" },
      "assistant-1",
    );
    expect(second.messages[0].gateActions).toHaveLength(2);
    expect(second.messages[0].gateActions![1].kind).toBe("ready");
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
    expect(next.messages[0].gateActions).toBeUndefined();
    expect(next.messages[1].gateActions).toHaveLength(1);
  });
});
