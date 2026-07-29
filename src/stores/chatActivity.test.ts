// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import type { StreamEvent } from "../lib/tauri";
import {
  reduceChatStreamEvent,
  type ChatEventState,
} from "./chatEvents";

function state(): ChatEventState {
  return {
    messages: [
      {
        id: "assistant-1",
        role: "assistant",
        content: "",
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

function activity(revision: number, stateValue: string, label: string): StreamEvent {
  return {
    type: "turn_activity_updated",
    root_turn_id: "user-1",
    revision,
    phase: stateValue,
    status: "active",
    recent_activity_kind: stateValue,
    recent_activity_label: label,
    waiting_reason: null,
    updated_at: 100 + revision,
    terminal_reason: null,
  };
}

describe("turn activity events", () => {
  it("keeps one current activity snapshot and ignores stale revisions", () => {
    const current = reduceChatStreamEvent(
      state(),
      activity(3, "verifying", "正在验证结果"),
      "assistant-1",
    );
    const stale = reduceChatStreamEvent(
      current,
      activity(2, "working", "仍在执行"),
      "assistant-1",
    );

    expect(stale.messages[0].turnActivity).toEqual(
      expect.objectContaining({
        rootTurnId: "user-1",
        revision: 3,
        phase: "verifying",
        label: "正在验证结果",
      }),
    );
  });

  it("bounds a long-running turn to the latest snapshot", () => {
    let current = state();
    for (let revision = 1; revision <= 1_000; revision += 1) {
      current = reduceChatStreamEvent(
        current,
        activity(revision, "working", `步骤 ${revision}`),
        "assistant-1",
      );
    }

    expect(current.messages[0].turnActivity?.revision).toBe(1_000);
    expect(current.messages).toHaveLength(1);
    expect(JSON.stringify(current).length).toBeLessThan(2_000);
  });

  it("preserves blocked as distinct from an execution error", () => {
    const withTool = reduceChatStreamEvent(
      state(),
      {
        type: "tool_call_start",
        id: "delivery-1",
        name: "deliver_changes",
        args: {},
      },
      "assistant-1",
    );
    const blocked = reduceChatStreamEvent(
      withTool,
      {
        type: "tool_result",
        tool_call_id: "delivery-1",
        content: "缺少发布能力",
        is_error: false,
        status: "blocked",
      },
      "assistant-1",
    );

    expect(blocked.messages[0].toolCalls?.[0]).toEqual(
      expect.objectContaining({
        status: "blocked",
        isError: false,
      }),
    );
  });
});
