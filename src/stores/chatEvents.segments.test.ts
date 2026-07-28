// SPDX-License-Identifier: Apache-2.0
//
// Turn-timeline segments. A 26-minute streaming turn used to render as two
// blobs — every tool card stacked on top, every narration sentence fused
// into one wall of text below — because text_delta only appended to one
// content string. Segments preserve the actual interleaving.

import { describe, it, expect } from "vitest";
import { reduceChatStreamEvent, type ChatEventState } from "./chatEvents";

function baseState(): ChatEventState {
  return {
    messages: [
      { id: "a1", role: "assistant", content: "", toolCalls: [], createdAt: 1 },
    ],
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

function play(events: Parameters<typeof reduceChatStreamEvent>[1][]): ChatEventState {
  return events.reduce((s, e) => reduceChatStreamEvent(s, e, "a1"), baseState());
}

describe("turn timeline segments", () => {
  it("interleaves narration and tool calls in arrival order", () => {
    const state = play([
      { type: "text_delta", content: "先看" },
      { type: "text_delta", content: "文件。" },
      { type: "tool_call_start", id: "t1", name: "bash", args: { command: "ls" } },
      { type: "text_delta", content: "现在改实现。" },
      { type: "tool_call_start", id: "t2", name: "edit_file", args: {} },
      { type: "text_delta", content: "完成。" },
    ]);
    const segments = state.messages[0].segments!;
    expect(segments.map((s) => s.kind)).toEqual(["text", "tool", "text", "tool", "text"]);
    expect(segments[0]).toEqual({ kind: "text", text: "先看文件。" });
    expect(segments[1]).toEqual({ kind: "tool", toolCallId: "t1" });
    expect(segments[2]).toEqual({ kind: "text", text: "现在改实现。" });
    expect(segments[4]).toEqual({ kind: "text", text: "完成。" });
  });

  it("keeps the legacy concatenated content in sync for consumers like automatic learning", () => {
    const state = play([
      { type: "text_delta", content: "步骤一。" },
      { type: "tool_call_start", id: "t1", name: "bash", args: {} },
      { type: "text_delta", content: "步骤二。" },
    ]);
    expect(state.messages[0].content).toBe("步骤一。步骤二。");
  });

  it("does not create empty text segments when a tool starts first", () => {
    const state = play([
      { type: "tool_call_start", id: "t1", name: "bash", args: {} },
      { type: "text_delta", content: "跑完了。" },
    ]);
    const segments = state.messages[0].segments!;
    expect(segments.map((s) => s.kind)).toEqual(["tool", "text"]);
  });
});
