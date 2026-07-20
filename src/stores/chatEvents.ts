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

/** Metadata of an open secure-secret prompt. The secret VALUE never passes
 *  through this state — it goes from the modal straight to provide_secret. */
export interface PendingSecret {
  requestId: string;
  purpose: string;
  hint: string;
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
  /** Completion-gate provenance from the DB: "rejected_candidate" collapses
   *  the reply, "gate_recovery"/"gate_ready" render as system notices. */
  completionState?: string;
  /** Live gate interventions on the streaming turn, in arrival order. */
  gateActions?: GateActionState[];
}

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
  /** Open secure-secret prompt, if any. Optional so existing state
   *  constructors stay valid; absent means none. */
  pendingSecret?: PendingSecret | null;
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
      const nextStatus = event.status;
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

    case "done": {
      const endedAt = Date.now();
      return {
        ...state,
        streaming: false,
        inputTokenTotal: state.inputTokenTotal + event.input_tokens,
        outputTokenTotal: state.outputTokenTotal + event.output_tokens,
        messages: state.messages.map((m) =>
          m.id === msgId && m.durationMs == null
            ? { ...m, durationMs: Math.max(0, endedAt - m.createdAt) }
            : m,
        ),
      };
    }

    case "error": {
      const endedAt = Date.now();
      return {
        ...state,
        streaming: false,
        pendingPermission: null,
        messages: state.messages.map((m) =>
          m.id === msgId
            ? {
                ...m,
                content: m.content + `\n\nError: ${event.message}`,
                durationMs: m.durationMs ?? Math.max(0, endedAt - m.createdAt),
              }
            : m,
        ),
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
        messages: state.messages.map((m) =>
          m.id === msgId
            ? {
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
              }
            : m,
        ),
      };

    case "secret_request":
      return {
        ...state,
        pendingSecret: {
          requestId: event.request_id,
          purpose: event.purpose,
          hint: event.hint,
        },
      };

    case "completion_gate_action":
      return {
        ...state,
        messages: state.messages.map((m) =>
          m.id === msgId
            ? {
                ...m,
                gateActions: [
                  ...(m.gateActions ?? []),
                  { kind: event.kind, detail: event.detail },
                ],
              }
            : m,
        ),
      };

    case "tool_call_args_delta":
    case "tool_call_end":
      return state;
  }

  return state;
}

export function markSecretResponse(state: ChatEventState): ChatEventState {
  return { ...state, pendingSecret: null };
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
