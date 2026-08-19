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
  });

  it("attaches every completed round usage to its assistant segment", () => {
    const first = reduceChatStreamEvent(
      baseState(),
      { type: "done", input_tokens: 1200, output_tokens: 34 },
      "assistant-1",
    );
    const second = reduceChatStreamEvent(
      first,
      { type: "done", input_tokens: 800, output_tokens: 16 },
      "assistant-1",
    );

    expect(second.messages[0]).toMatchObject({
      inputTokens: 2000,
      outputTokens: 50,
    });
  });

  it("terminalizes transient tools when durable recovery hands the turn back", () => {
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

    const handedBack = reduceChatStreamEvent(
      waiting,
      {
        type: "turn_activity_updated",
        root_turn_id: "root-handback",
        revision: 9,
        phase: "waiting",
        status: "waiting_core_input",
        recent_activity_kind: "technical_recovery_exhausted",
        recent_activity_label: "系统已停止并把当前结论交还给你",
        waiting_reason: "technical_recovery_exhausted",
        updated_at: 9,
        terminal_reason: "technical_recovery_exhausted",
        objective_id: "objective-handback",
        objective_status: "waiting_core_input",
      },
      "assistant-1",
    );

    expect(handedBack.streaming).toBe(false);
    expect(handedBack.messages[0].toolCalls?.[0]).toMatchObject({
      status: "blocked",
      isError: false,
    });
    expect(handedBack.messages[0].toolCalls?.[0]?.result).not.toContain(
      "external_state_uncertain",
    );
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
