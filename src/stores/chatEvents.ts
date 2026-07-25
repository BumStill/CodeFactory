// SPDX-License-Identifier: Apache-2.0
import type { StreamEvent } from "../lib/tauri.js";

export interface ToolCallState {
  id: string;
  name: string;
  args: string;
  result?: string;
  isError?: boolean;
  status: "waiting_permission" | "running" | "done" | "error" | "denied" | "cancelled";
}

export interface PendingPermission {
  toolCallId: string;
  toolName: string;
  args: unknown;
}

export interface UIMessage {
  id: string;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  toolCalls?: ToolCallState[];
  transportRetries?: TransportRetryState[];
  inputTokens?: number;
  outputTokens?: number;
  createdAt: number;
  /** Wall-clock the turn took, frozen when the stream reached a terminal
   *  state (done/error). Absent while still streaming (the UI ticks live off
   *  `createdAt` instead) and for plain user messages. */
  durationMs?: number;
  /** Internal completion-review provenance from the DB. Drafts and
   *  injected review instructions are excluded from the chat transcript. */
  completionState?: string;
  /** User-facing runtime warnings/notices retained for this turn. */
  gateActions?: GateActionState[];
  /** Turn timeline: narration and tool calls in arrival order. Only built
   *  during live streaming — hydrated history is already interleaved as
   *  separate rows. */
  segments?: TurnSegment[];
}

/** One slice of a streaming turn, in arrival order: narration text or a
 *  tool invocation. Preserves the interleaving that a single concatenated
 *  content string loses (the "two blobs" wall-of-text bug). */
export type TurnSegment =
  | { kind: "text"; text: string }
  | { kind: "tool"; toolCallId: string };

export interface GateActionState {
  kind: string;
  detail: string;
}

export interface TransportRetryState {
  label: string;
  attempt: number;
  maxAttempts: number;
  delayMs: number;
  reason: string;
}

export interface ContextUsage {
  used: number;
  limit: number;
  maxLimit: number;
}

export interface CompressionToast {
  elidedCount: number;
  tokensFreed: number;
  /** Monotonic id so React shows a new toast even if the values match. */
  id: number;
}

export interface ChatEventState {
  messages: UIMessage[];
  streaming: boolean;
  inputTokenTotal: number;
  outputTokenTotal: number;
  pendingPermission: PendingPermission | null;
  /** Last reported provider-side prompt_tokens / resolved model limit. */
  contextUsage: ContextUsage | null;
  /** Set whenever the backend just elided messages; UI shows a toast. */
  compressionToast: CompressionToast | null;
}

function updateMessageById(
  messages: UIMessage[],
  msgId: string,
  update: (message: UIMessage) => UIMessage,
): UIMessage[] {
  const tailIndex = messages.length - 1;
  const index =
    tailIndex >= 0 && messages[tailIndex].id === msgId
      ? tailIndex
      : messages.findIndex((message) => message.id === msgId);
  if (index < 0) return messages;
  const next = messages.slice();
  next[index] = update(messages[index]);
  return next;
}

export function reduceChatStreamEvent(
  state: ChatEventState,
  event: StreamEvent,
  msgId: string,
): ChatEventState {
  switch (event.type) {
    case "text_delta":
      return {
        ...state,
        messages: updateMessageById(state.messages, msgId, (m) => {
          const segments = [...(m.segments ?? [])];
          const tail = segments[segments.length - 1];
          if (tail && tail.kind === "text") {
            segments[segments.length - 1] = { kind: "text", text: tail.text + event.content };
          } else {
            segments.push({ kind: "text", text: event.content });
          }
          return { ...m, content: m.content + event.content, segments };
        }),
      };

    case "tool_call_start":
      return {
        ...state,
        messages: updateMessageById(state.messages, msgId, (m) => {
          const withTool = upsertToolCallOnMessage(m, {
            id: event.id,
            name: event.name,
            args: formatToolArgs(event.args),
            status: "running",
          });
          const segments = withTool.segments ?? [];
          // Re-announced ids (permission flow) must not duplicate the segment.
          if (segments.some((s) => s.kind === "tool" && s.toolCallId === event.id)) {
            return withTool;
          }
          return { ...withTool, segments: [...segments, { kind: "tool", toolCallId: event.id }] };
        }),
      };

    case "permission_request": {
      return {
        ...state,
        pendingPermission: {
          toolCallId: event.tool_call_id,
          toolName: event.tool_name,
          args: event.args,
        },
        messages: upsertToolCall(state.messages, msgId, {
          id: event.tool_call_id,
          name: event.tool_name,
          args: formatToolArgs(event.args),
          status: "waiting_permission",
        }),
      };
    }

    case "tool_result": {
      const nextStatus = event.status;
      return {
        ...state,
        pendingPermission:
          state.pendingPermission?.toolCallId === event.tool_call_id
            ? null
            : state.pendingPermission,
        messages: updateMessageById(state.messages, msgId, (m) => ({
          ...m,
          toolCalls: (m.toolCalls ?? []).map((tc) =>
            tc.id === event.tool_call_id
              ? { ...tc, result: event.content, isError: event.is_error, status: nextStatus }
              : tc,
          ),
        })),
      };
    }

    case "done": {
      const endedAt = Date.now();
      return {
        ...state,
        streaming: false,
        inputTokenTotal: state.inputTokenTotal + event.input_tokens,
        outputTokenTotal: state.outputTokenTotal + event.output_tokens,
        messages: updateMessageById(state.messages, msgId, (m) =>
          m.durationMs == null ? { ...m, durationMs: Math.max(0, endedAt - m.createdAt) } : m,
        ),
      };
    }

    case "error": {
      const endedAt = Date.now();
      return {
        ...state,
        streaming: false,
        pendingPermission: null,
        messages: updateMessageById(state.messages, msgId, (m) => ({
          ...m,
          content: m.content + `\n\nError: ${event.message}`,
          durationMs: m.durationMs ?? Math.max(0, endedAt - m.createdAt),
        })),
      };
    }

    case "context_usage":
      return {
        ...state,
        contextUsage: {
          used: event.used_tokens,
          limit: event.limit_tokens,
          maxLimit: event.max_limit_tokens ?? event.limit_tokens,
        },
      };

    case "context_compressed":
      return {
        ...state,
        compressionToast: {
          elidedCount: event.elided_count,
          tokensFreed: event.tokens_freed,
          id: Date.now(),
        },
      };

    case "transport_retry":
      return {
        ...state,
        messages: updateMessageById(state.messages, msgId, (m) => ({
          ...m,
          transportRetries: [
            ...(m.transportRetries ?? []),
            {
              label: event.label,
              attempt: event.attempt,
              maxAttempts: event.max_attempts,
              delayMs: event.delay_ms,
              reason: event.reason,
            },
          ],
        })),
      };

    case "completion_gate_action":
      // The completion gate is a control loop, not a chat participant. Recovery
      // and ready rounds change what the model does next; they change nothing
      // about what the user sees. The steps they produce stay in the timeline
      // like any other work — the last text segment is still the final answer,
      // because a recovery round always runs a tool before speaking again.
      if (event.kind === "recovery" || event.kind === "ready") {
        return state;
      }
      if (event.kind === "warning") {
        return {
          ...state,
          messages: updateMessageById(state.messages, msgId, (m) => ({
            ...m,
            gateActions: [...(m.gateActions ?? []), { kind: "warning", detail: event.detail }],
          })),
        };
      }
      if (event.kind === "turn_notice") {
        return {
          ...state,
          messages: updateMessageById(state.messages, msgId, (m) => ({
            ...m,
            gateActions: [
              ...(m.gateActions ?? []),
              { kind: "turn_notice", detail: event.detail },
            ],
          })),
        };
      }
      return state;

    case "tool_call_args_delta":
    case "tool_call_end":
      return state;
  }

  return state;
}

export function markPermissionResponse(
  state: ChatEventState,
  toolCallId: string,
  allow: boolean,
): ChatEventState {
  return {
    ...state,
    pendingPermission:
      state.pendingPermission?.toolCallId === toolCallId ? null : state.pendingPermission,
    messages: state.messages.map((m) => ({
      ...m,
      toolCalls: m.toolCalls?.map((tc) =>
        tc.id === toolCallId
          ? {
              ...tc,
              status: allow ? "running" : "denied",
              result: allow ? tc.result : "Denied by user",
              isError: allow ? tc.isError : true,
            }
          : tc,
      ),
    })),
  };
}

function upsertToolCall(
  messages: UIMessage[],
  msgId: string,
  toolCall: ToolCallState,
): UIMessage[] {
  return updateMessageById(messages, msgId, (message) =>
    upsertToolCallOnMessage(message, toolCall),
  );
}

function upsertToolCallOnMessage(
  message: UIMessage,
  toolCall: ToolCallState,
): UIMessage {
  const existing = message.toolCalls ?? [];
  const found = existing.some((tc) => tc.id === toolCall.id);
  return {
    ...message,
    toolCalls: found
      ? existing.map((tc) => (tc.id === toolCall.id ? { ...tc, ...toolCall } : tc))
      : [...existing, toolCall],
  };
}

export function formatToolArgs(args: unknown): string {
  if (typeof args === "string") {
    return args;
  }
  try {
    return JSON.stringify(args, null, 2);
  } catch {
    return String(args);
  }
}
