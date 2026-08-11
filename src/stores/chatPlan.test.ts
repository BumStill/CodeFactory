// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import type { StreamEvent } from "../lib/tauri";
import { reduceChatStreamEvent, type ChatEventState, type UIMessage } from "./chatEvents";

function message(): UIMessage {
  return {
    id: "assistant-1",
    role: "assistant",
    content: "",
    createdAt: 1,
  };
}

function state(): ChatEventState {
  return {
    messages: [message()],
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

function planEvent(revision: number, changeReason?: string): StreamEvent {
  return {
    type: "plan_updated",
    root_turn_id: "user-1",
    revision,
    steps: [
      { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
      { id: "implement", title: "实现功能", kind: "implementation", status: "in_progress" },
      { id: "verify", title: "真实验证", kind: "verification", status: "pending" },
    ],
    explanation: "进入实现阶段",
    waiting_reason: null,
    change_reason: changeReason ?? null,
    created_at: 100 + revision,
  };
}

describe("structured chat plan events", () => {
  it("updates the current assistant turn with a structured plan", () => {
    const reduced = reduceChatStreamEvent(state(), planEvent(1), "assistant-1");

    expect(reduced.messages[0].plan).toEqual(
      expect.objectContaining({
        rootTurnId: "user-1",
        revision: 1,
        explanation: "进入实现阶段",
        nextActionOwner: "system",
      }),
    );
    expect(reduced.messages[0].plan?.steps[1]).toEqual(
      expect.objectContaining({ id: "implement", status: "in_progress" }),
    );
  });

  it("ignores an out-of-order older revision", () => {
    const current = reduceChatStreamEvent(state(), planEvent(3, "拆分验证步骤"), "assistant-1");
    const stale = reduceChatStreamEvent(current, planEvent(2), "assistant-1");

    expect(stale.messages[0].plan?.revision).toBe(3);
    expect(stale.messages[0].plan?.changeReason).toBe("拆分验证步骤");
  });

  it("retains bounded waiting and plan-change history across revisions", () => {
    const waiting = {
      ...planEvent(1),
      waiting_reason: "等待 CI",
    } as StreamEvent;
    const first = reduceChatStreamEvent(state(), waiting, "assistant-1");
    const final = reduceChatStreamEvent(
      first,
      planEvent(2, "增加安装 smoke"),
      "assistant-1",
    );

    expect(final.messages[0].plan?.waitingHistory).toEqual(["等待 CI"]);
    expect(final.messages[0].plan?.changeHistory).toEqual(["增加安装 smoke"]);
  });

  it("preserves the structured next-action owner instead of inferring it from waiting text", () => {
    const event = {
      ...planEvent(1),
      waiting_reason: "需要检查权限配置",
      next_action_owner: "system",
    } as StreamEvent;

    const reduced = reduceChatStreamEvent(state(), event, "assistant-1");

    expect(reduced.messages[0].plan).toEqual(
      expect.objectContaining({
        waitingReason: "需要检查权限配置",
        nextActionOwner: "system",
      }),
    );
  });

  it("does not render update_plan as a low-level tool card", () => {
    const reduced = reduceChatStreamEvent(
      state(),
      {
        type: "tool_call_start",
        id: "plan-call",
        name: "update_plan",
        args: { steps: [] },
      },
      "assistant-1",
    );

    expect(reduced.messages[0].toolCalls).toBeUndefined();
    expect(reduced.messages[0].segments).toBeUndefined();
  });

  it("keeps one latest snapshot after one thousand revisions", () => {
    let current = state();
    for (let revision = 1; revision <= 1_000; revision += 1) {
      current = reduceChatStreamEvent(current, planEvent(revision), "assistant-1");
    }

    expect(current.messages[0].plan?.revision).toBe(1_000);
    expect(current.messages).toHaveLength(1);
    expect(JSON.stringify(current).length).toBeLessThan(8_000);
  });
});
