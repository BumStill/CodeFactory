// SPDX-License-Identifier: Apache-2.0
import { reduceChatStreamEvent, type ChatEventState } from "./chatEvents.js";

function assertEqual<T>(actual: T, expected: T, label: string) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function assertTruthy(value: unknown, label: string): asserts value {
  if (!value) {
    throw new Error(`${label}: expected truthy value`);
  }
}

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
  };
}

{
  const next = reduceChatStreamEvent(
    baseState(),
    {
      type: "permission_request",
      tool_call_id: "tool-1",
      tool_name: "bash",
      args: { command: "pnpm build" },
    },
    "assistant-1",
  );

  const assistant = next.messages[0];
  const toolCall = assistant.toolCalls?.[0];
  assertTruthy(toolCall, "permission request creates a visible tool call card");
  assertEqual(toolCall.id, "tool-1", "tool call id");
  assertEqual(toolCall.name, "bash", "tool name");
  assertEqual(toolCall.status, "waiting_permission", "tool status");
  assertEqual(toolCall.args, JSON.stringify({ command: "pnpm build" }, null, 2), "formatted args");
  assertEqual(next.pendingPermission?.toolCallId, "tool-1", "pending permission id");
}

{
  const waiting = reduceChatStreamEvent(
    baseState(),
    {
      type: "permission_request",
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
    },
    "assistant-1",
  );

  const toolCall = done.messages[0].toolCalls?.[0];
  assertTruthy(toolCall, "tool result keeps the tool card");
  assertEqual(toolCall.status, "done", "tool result status");
  assertEqual(toolCall.result, "Written 5 bytes", "tool result content");
  assertEqual(done.pendingPermission, null, "tool result clears pending permission");
}
