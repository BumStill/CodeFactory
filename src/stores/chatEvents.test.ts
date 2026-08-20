// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
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

describe("chat tool call stream events", () => {
  it("creates a visible tool call card from a permission request", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "permission_request",
        intent_id: "intent-1",
        tool_call_id: "tool-1",
        tool_name: "bash",
        args: { command: "pnpm build" },
      },
      "assistant-1",
    );

    const assistant = next.messages[0];
    const toolCall = assistant.toolCalls?.[0];
    expect(toolCall).toBeTruthy();
    expect(toolCall?.id).toBe("tool-1");
    expect(toolCall?.name).toBe("bash");
    expect(toolCall?.status).toBe("waiting_permission");
    expect(toolCall?.args).toBe(JSON.stringify({ command: "pnpm build" }, null, 2));
    expect(next.pendingPermission?.toolCallId).toBe("tool-1");
  });

  it("keeps the tool card and clears pending permission on a tool result", () => {
    const waiting = reduceChatStreamEvent(
      baseState(),
      {
        type: "permission_request",
        intent_id: "intent-2",
        tool_call_id: "tool-2",
        tool_name: "write_file",
        args: { path: "README.md", content: "hello" },
      },
      "assistant-1",
    );

    const done = reduceChatStreamEvent(
      waiting,
      {
        type: "tool_result",
        tool_call_id: "tool-2",
        content: "Written 5 bytes",
        is_error: false,
        status: "done",
        metadata: {
          requested_ceiling: "through_release",
          reached_state: "merged",
          recoverable: true,
        },
      },
      "assistant-1",
    );

    const toolCall = done.messages[0].toolCalls?.[0];
    expect(toolCall).toBeTruthy();
    expect(toolCall?.status).toBe("done");
    expect(toolCall?.result).toBe("Written 5 bytes");
    expect(toolCall?.metadata).toEqual({
      requested_ceiling: "through_release",
      reached_state: "merged",
      recoverable: true,
    });
    expect(done.pendingPermission).toBeNull();
  });

  it("keeps a remote delivery wait active until durable turn settlement", () => {
    const started = reduceChatStreamEvent(
      baseState(),
      { type: "tool_call_start", id: "delivery-1", name: "deliver_changes", args: {} },
      "assistant-1",
    );
    const waiting = reduceChatStreamEvent(
      started,
      {
        type: "tool_result",
        tool_call_id: "delivery-1",
        content: "CI is still pending",
        is_error: false,
        status: "waiting",
        metadata: { recovery_class: "wait_retryable", retry_after_ms: 30_000 },
      },
      "assistant-1",
    );

    expect(waiting.streaming).toBe(true);
    expect(waiting.messages[0].toolCalls?.[0]?.status).toBe("waiting");

    const done = reduceChatStreamEvent(
      waiting,
      { type: "done", input_tokens: 0, output_tokens: 0 },
      "assistant-1",
    );
    expect(done.streaming).toBe(true);

    const settled = reduceChatStreamEvent(
      done,
      {
        type: "turn_settled",
        run_instance_id: "run-delivery-1",
        root_turn_id: "root-delivery-1",
        objective_id: "objective-delivery-1",
        status: "waiting_system",
      },
      "assistant-1",
    );
    expect(settled.streaming).toBe(false);
    expect(settled.messages[0].turnSettledAt).toBeUndefined();
  });

  it("attaches the run-cumulative terminal usage to its assistant segment", () => {
    const done = reduceChatStreamEvent(
      baseState(),
      { type: "done", input_tokens: 2000, output_tokens: 50 },
      "assistant-1",
    );

    expect(done.messages[0]).toMatchObject({
      inputTokens: 2000,
      outputTokens: 50,
    });
  });

  it.each(["waiting_authorization", "waiting_business_decision"] as const)(
    "keeps %s system-owned when turn_settled reports waiting_user",
    (objectiveStatus) => {
      const active = reduceChatStreamEvent(baseState(), {
        type: "turn_activity_updated",
        root_turn_id: "root",
        revision: 2,
        phase: "waiting",
        status: objectiveStatus,
        recent_activity_kind: "waiting",
        recent_activity_label: "等待系统输入",
        waiting_reason: null,
        updated_at: Date.now(),
        terminal_reason: null,
        objective_status: objectiveStatus,
      }, "assistant-1");
      const settled = reduceChatStreamEvent(active, {
        type: "turn_settled",
        run_instance_id: "run",
        root_turn_id: "root",
        status: "waiting_user",
      }, "assistant-1");

      expect(settled.messages[0].turnSettledAt).toBeUndefined();
    },
  );

  it("terminalizes transient tools when a durable system incident settles the turn", () => {
    const waiting: ChatEventState = {
      ...baseState(),
      messages: [{
        ...baseState().messages[0],
        toolCalls: [{
          id: "tool-handback",
          name: "bash",
          args: "git status --short",
          result: "external_state_uncertain",
          status: "waiting",
        }],
      }],
    };

    const settledIncident = reduceChatStreamEvent(
      waiting,
      {
        type: "turn_activity_updated",
        root_turn_id: "root-handback",
        revision: 9,
        phase: "waiting",
        status: "waiting_system",
        recent_activity_kind: "technical_recovery_exhausted",
        recent_activity_label: "系统已登记故障，无需补充输入",
        waiting_reason: "technical_recovery_exhausted",
        updated_at: 9,
        terminal_reason: "technical_recovery_exhausted",
        objective_id: "objective-handback",
        objective_status: "waiting_system",
      },
      "assistant-1",
    );

    expect(settledIncident.streaming).toBe(false);
    expect(settledIncident.messages[0].toolCalls?.[0]).toMatchObject({
      status: "blocked",
      isError: false,
    });
    expect(settledIncident.messages[0].toolCalls?.[0]?.result).not.toContain(
      "external_state_uncertain",
    );

    const settledEvent = reduceChatStreamEvent(
      waiting,
      {
        type: "turn_settled",
        run_instance_id: "run-handback",
        root_turn_id: "root-handback",
        objective_id: "objective-handback",
        status: "system_incident",
      },
      "assistant-1",
    );
    expect(settledEvent.messages[0].turnSettledAt).toBeDefined();
    expect(settledEvent.messages[0].toolCalls?.[0]).toMatchObject({
      status: "blocked",
      isError: false,
    });
  });

  it("settles every live segment that belongs to the same root turn", () => {
    const waiting: ChatEventState = {
      ...baseState(),
      messages: [
        {
          id: "root-handback",
          role: "user",
          content: "继续完成",
          createdAt: 1,
        },
        {
          id: "assistant-before-steer",
          role: "assistant",
          rootTurnId: "root-handback",
          content: "正在检查 CI。",
          createdAt: 2,
          turnActivity: {
            rootTurnId: "root-handback",
            revision: 8,
            phase: "waiting",
            status: "waiting_system",
            kind: "remediation",
            label: "系统仍在处理",
            waitingReason: "等待 CI",
            updatedAt: 8,
            terminalReason: null,
            objectiveStatus: "waiting_system",
          },
          toolCalls: [{
            id: "tool-before-steer",
            name: "bash",
            args: "git status --short",
            status: "waiting",
          }],
        },
        {
          id: "assistant-after-steer",
          role: "assistant",
          rootTurnId: "root-handback",
          content: "",
          createdAt: 3,
          toolCalls: [{
            id: "tool-after-steer",
            name: "bash",
            args: "git log -1",
            status: "running",
          }],
        },
      ],
    };

    const activity = reduceChatStreamEvent(
      waiting,
      {
        type: "turn_activity_updated",
        root_turn_id: "root-handback",
        revision: 9,
        phase: "waiting",
        status: "waiting_system",
        recent_activity_kind: "technical_recovery_exhausted",
        recent_activity_label: "系统已登记故障，无需补充输入",
        waiting_reason: "technical_recovery_exhausted",
        updated_at: 9,
        terminal_reason: "technical_recovery_exhausted",
        objective_status: "waiting_system",
      },
      "assistant-after-steer",
    );
    const settled = reduceChatStreamEvent(
      activity,
      {
        type: "turn_settled",
        run_instance_id: "run-handback",
        root_turn_id: "root-handback",
        status: "system_incident",
      },
      "assistant-after-steer",
    );

    for (const assistant of settled.messages.filter((message) => message.role === "assistant")) {
      expect(assistant.turnActivity?.terminalReason).toBe("technical_recovery_exhausted");
      expect(assistant.turnSettledAt).toBeDefined();
      expect(assistant.toolCalls?.[0]).toMatchObject({ status: "blocked", isError: false });
    }
    expect(settled.streaming).toBe(false);

    const afterLateActivity = reduceChatStreamEvent(
      settled,
      {
        type: "turn_activity_updated",
        root_turn_id: "root-handback",
        revision: 8,
        phase: "working",
        status: "active",
        recent_activity_kind: "tool",
        recent_activity_label: "迟到的旧执行事件",
        waiting_reason: null,
        updated_at: 8,
        terminal_reason: null,
        objective_status: "active",
      },
      "assistant-before-steer",
    );
    expect(afterLateActivity.streaming).toBe(false);
    for (const assistant of afterLateActivity.messages.filter(
      (message) => message.role === "assistant",
    )) {
      expect(assistant.turnActivity?.terminalReason).toBe(
        "technical_recovery_exhausted",
      );
      expect(assistant.turnSettledAt).toBeDefined();
      expect(assistant.toolCalls?.[0]).toMatchObject({ status: "blocked" });
    }
  });

  it("does not let a terminal event for an older root stop the active next root", () => {
    const activeNextRoot: ChatEventState = {
      ...baseState(),
      messages: [
        { id: "root-old", role: "user", content: "旧任务", createdAt: 1 },
        {
          id: "assistant-old",
          role: "assistant",
          rootTurnId: "root-old",
          content: "旧任务正在恢复",
          createdAt: 2,
        },
        { id: "root-new", role: "user", content: "新任务", createdAt: 3 },
        {
          id: "assistant-new",
          role: "assistant",
          rootTurnId: "root-new",
          content: "新任务正在执行",
          createdAt: 4,
          turnActivity: {
            rootTurnId: "root-new",
            revision: 2,
            phase: "working",
            status: "active",
            kind: "tool",
            label: "正在执行新任务",
            waitingReason: null,
            updatedAt: 4,
            terminalReason: null,
            objectiveStatus: "active",
          },
        },
      ],
    };

    const oldActivity = reduceChatStreamEvent(
      activeNextRoot,
      {
        type: "turn_activity_updated",
        root_turn_id: "root-old",
        revision: 9,
        phase: "waiting",
        status: "waiting_system",
        recent_activity_kind: "technical_recovery_exhausted",
        recent_activity_label: "旧任务已登记故障",
        waiting_reason: "technical_recovery_exhausted",
        updated_at: 9,
        terminal_reason: "technical_recovery_exhausted",
        objective_status: "waiting_system",
      },
      "assistant-old",
    );
    const oldSettlement = reduceChatStreamEvent(
      oldActivity,
      {
        type: "turn_settled",
        run_instance_id: "run-old",
        root_turn_id: "root-old",
        status: "system_incident",
      },
      "assistant-old",
    );

    expect(oldSettlement.streaming).toBe(true);
    expect(oldSettlement.messages[1].turnSettledAt).toBeDefined();
    expect(oldSettlement.messages[3].turnSettledAt).toBeUndefined();
    expect(oldSettlement.messages[3].turnActivity?.objectiveStatus).toBe("active");

    const unknownOldSettlement = reduceChatStreamEvent(
      {
        ...activeNextRoot,
        messages: activeNextRoot.messages.slice(2),
      },
      {
        type: "turn_settled",
        run_instance_id: "run-missing-old",
        root_turn_id: "root-missing-old",
        status: "system_incident",
      },
      "assistant-new",
    );
    expect(unknownOldSettlement.streaming).toBe(true);
    expect(unknownOldSettlement.messages[1].turnSettledAt).toBeUndefined();
  });

  it("marks the tool card cancelled and clears pending permission", () => {
    const waiting = reduceChatStreamEvent(
      baseState(),
      {
        type: "permission_request",
        intent_id: "intent-3",
        tool_call_id: "tool-cancelled",
        tool_name: "bash",
        args: { command: "sleep 10" },
      },
      "assistant-1",
    );
    const cancelled = reduceChatStreamEvent(
      waiting,
      {
        type: "tool_result",
        tool_call_id: "tool-cancelled",
        content: "Tool call cancelled by user.",
        is_error: true,
        status: "cancelled",
      },
      "assistant-1",
    );

    expect(cancelled.messages[0].toolCalls?.[0].status).toBe("cancelled");
    expect(cancelled.pendingPermission).toBeNull();
  });
});
