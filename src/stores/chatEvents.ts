// SPDX-License-Identifier: Apache-2.0
import type { StreamEvent } from "../lib/tauri.js";

export interface ToolCallState {
  id: string;
  name: string;
  args: string;
  result?: string;
  isError?: boolean;
  status: "waiting_permission" | "running" | "done" | "error" | "denied";
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
  inputTokens?: number;
  outputTokens?: number;
  createdAt: number;
}

export interface ContextUsage {
  used: number;
  limit: number;
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

export function reduceChatStreamEvent(
  state: ChatEventState,
  event: StreamEvent,
  msgId: string,
): ChatEventState {
  switch (event.type) {
    case "text_delta":
      return {
        ...state,
        messages: state.messages.map((m) =>
          m.id === msgId ? { ...m, content: m.content + event.content } : m,
        ),
      };

    case "tool_call_start":
      return {
        ...state,
        messages: upsertToolCall(state.messages, msgId, {
          id: event.id,
          name: event.name,
          args: formatToolArgs(event.args),
          status: "running",
        }),
      };

    case "permission_request":
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

    case "tool_result": {
      const nextStatus = event.is_error ? "error" : "done";
      return {
        ...state,
        pendingPermission:
          state.pendingPermission?.toolCallId === event.tool_call_id
            ? null
            : state.pendingPermission,
        messages: state.messages.map((m) =>
          m.id === msgId
            ? {
                ...m,
                toolCalls: (m.toolCalls ?? []).map((tc) =>
                  tc.id === event.tool_call_id
                    ? {
                        ...tc,
                        result: event.content,
                        isError: event.is_error,
                        status: nextStatus,
                      }
                    : tc,
                ),
              }
            : m,
        ),
      };
    }

    case "done":
      return {
        ...state,
        streaming: false,
        inputTokenTotal: state.inputTokenTotal + event.input_tokens,
        outputTokenTotal: state.outputTokenTotal + event.output_tokens,
      };

    case "error":
      return {
        ...state,
        streaming: false,
        messages: state.messages.map((m) =>
          m.id === msgId
            ? { ...m, content: m.content + `\n\nError: ${event.message}` }
            : m,
        ),
      };

    case "context_usage":
      return {
        ...state,
        contextUsage: {
          used: event.used_tokens,
          limit: event.limit_tokens,
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

    case "context_usage":
      return {
        ...state,
        contextUsage: {
          used: event.used_tokens,
          limit: event.limit_tokens,
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
  return messages.map((m) => {
    if (m.id !== msgId) return m;
    const existing = m.toolCalls ?? [];
    const found = existing.some((tc) => tc.id === toolCall.id);
    return {
      ...m,
      toolCalls: found
        ? existing.map((tc) => (tc.id === toolCall.id ? { ...tc, ...toolCall } : tc))
        : [...existing, toolCall],
    };
  });
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
