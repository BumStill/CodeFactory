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
  /** Internal review rounds stay out of the transcript. During recovery,
   *  model text is buffered only so a warning-released answer can be promoted. */
  internalReviewState?: "recovery" | "finalizing";
  internalReviewDraft?: string;
  /** Safe user-facing progress for the bounded completion review. Raw gate
   *  prompts, model drafts, commands, and tool arguments never enter it. */
  reviewProgress?: ReviewProgressState;
  /** Turn timeline: narration and tool calls in arrival order. Only built
   *  during live streaming — hydrated history is already interleaved as
   *  separate rows. */
  segments?: TurnSegment[];
}

export interface ReviewProgressState {
  phase: "recovering" | "finalizing" | "interrupted";
  attempt: number;
  limit: number;
  reason: string;
  currentStep: string;
  updatedAt: number;
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

export function reduceChatStreamEvent(
  state: ChatEventState,
  event: StreamEvent,
  msgId: string,
): ChatEventState {
  switch (event.type) {
    case "text_delta":
      return {
        ...state,
        messages: state.messages.map((m) => {
          if (m.id !== msgId) return m;
          if (m.internalReviewState === "recovery") {
            return {
              ...m,
              internalReviewDraft: (m.internalReviewDraft ?? "") + event.content,
            };
          }
          const segments = [...(m.segments ?? [])];
          const tail = segments[segments.length - 1];
          if (tail && tail.kind === "text") {
            segments[segments.length - 1] = { kind: "text", text: tail.text + event.content };
          } else {
            segments.push({ kind: "text", text: event.content });
          }
          return {
            ...m,
            content: m.content + event.content,
            segments,
            reviewProgress:
              m.internalReviewState === "finalizing" ? undefined : m.reviewProgress,
          };
        }),
      };

    case "tool_call_start":
      if (state.messages.some((m) => m.id === msgId && m.internalReviewState === "recovery")) {
        return {
          ...state,
          messages: state.messages.map((m) =>
            m.id === msgId
              ? {
                  ...m,
                  internalReviewDraft: "",
                  reviewProgress: m.reviewProgress
                    ? {
                        ...m.reviewProgress,
                        currentStep: "正在运行验证或修复步骤",
                        updatedAt: Date.now(),
                      }
                    : m.reviewProgress,
                }
              : m,
          ),
        };
      }
      return {
        ...state,
        messages: upsertToolCall(state.messages, msgId, {
          id: event.id,
          name: event.name,
          args: formatToolArgs(event.args),
          status: "running",
        }).map((m) => {
          if (m.id !== msgId) return m;
          const segments = m.segments ?? [];
          // Re-announced ids (permission flow) must not duplicate the segment.
          if (segments.some((s) => s.kind === "tool" && s.toolCallId === event.id)) {
            return m;
          }
          return { ...m, segments: [...segments, { kind: "tool", toolCallId: event.id }] };
        }),
      };

    case "permission_request": {
      const internalRecovery = state.messages.some(
        (m) => m.id === msgId && m.internalReviewState === "recovery",
      );
      return {
        ...state,
        pendingPermission: {
          toolCallId: event.tool_call_id,
          toolName: event.tool_name,
          args: event.args,
        },
        messages: internalRecovery
          ? state.messages.map((m) =>
              m.id === msgId && m.reviewProgress
                ? {
                    ...m,
                    reviewProgress: {
                      ...m.reviewProgress,
                      currentStep: "正在等待你的工具授权",
                      updatedAt: Date.now(),
                    },
                  }
                : m,
            )
          : upsertToolCall(state.messages, msgId, {
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
        messages: state.messages.map((m) => {
          if (m.id !== msgId) return m;
          if (m.internalReviewState === "recovery" && m.reviewProgress) {
            return {
              ...m,
              reviewProgress: {
                ...m.reviewProgress,
                currentStep: event.is_error ? "步骤失败，正在收敛处理" : "正在评估步骤结果",
                updatedAt: Date.now(),
              },
            };
          }
          return {
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
              };
        }),
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
            ? {
                ...m,
                reviewProgress: undefined,
                durationMs: Math.max(0, endedAt - m.createdAt),
              }
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
        messages: state.messages.map((m) => {
          if (m.id !== msgId) return m;
          if (m.internalReviewState) {
            const content = "本次处理未能完成，请重试。";
            return {
              ...m,
              content,
              toolCalls: [],
              segments: [{ kind: "text", text: content }],
              gateActions: undefined,
              internalReviewState: undefined,
              internalReviewDraft: undefined,
              reviewProgress: m.reviewProgress
                ? {
                    ...m.reviewProgress,
                    phase: "interrupted",
                    reason: "本次处理未能完成",
                    currentStep: "执行在完成前中断",
                    updatedAt: endedAt,
                  }
                : undefined,
              durationMs: m.durationMs ?? Math.max(0, endedAt - m.createdAt),
            };
          }
          return {
            ...m,
            content: m.content + `\n\nError: ${event.message}`,
            durationMs: m.durationMs ?? Math.max(0, endedAt - m.createdAt),
          };
        }),
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

    case "completion_gate_action":
      // Recovery/ready events delimit internal review rounds. Drop everything
      // accumulated in the shared streaming bubble so rejected drafts, repair
      // commands, and verification chatter cannot become the user's answer.
      // The next model text then starts a clean, self-contained final reply.
      if (event.kind === "recovery" || event.kind === "ready") {
        return {
          ...state,
          messages: state.messages.map((m) => {
            if (m.id !== msgId) return m;
            const attempt =
              event.kind === "recovery"
                ? Math.min((m.reviewProgress?.attempt ?? 0) + 1, 3)
                : Math.max(m.reviewProgress?.attempt ?? 0, 1);
            return {
                  ...m,
                  content: "",
                  toolCalls: [],
                  segments: [],
                  gateActions: undefined,
                  internalReviewState: event.kind === "recovery" ? "recovery" : "finalizing",
                  internalReviewDraft: "",
                  reviewProgress: {
                    phase: event.kind === "recovery" ? "recovering" : "finalizing",
                    attempt,
                    limit: 3,
                    reason:
                      event.kind === "recovery"
                        ? "最终答复还缺少验证证据"
                        : "验证证据已满足",
                    currentStep:
                      event.kind === "recovery"
                        ? "正在补充验证"
                        : "正在整理最终答复",
                    updatedAt: Date.now(),
                  },
                };
          }),
        };
      }
      if (event.kind === "warning") {
        return {
          ...state,
          messages: state.messages.map((m) => {
            if (m.id !== msgId) return m;
            const finalText =
              m.internalReviewState === "recovery"
                ? (m.internalReviewDraft ?? "")
                : ([...(m.segments ?? [])]
                    .reverse()
                    .find((segment) => segment.kind === "text")?.text ?? m.content);
            return {
              ...m,
              content: finalText,
              toolCalls: [],
              segments: finalText ? [{ kind: "text", text: finalText }] : [],
              gateActions: [{ kind: "warning", detail: event.detail }],
              internalReviewState: undefined,
              internalReviewDraft: undefined,
              reviewProgress: undefined,
            };
          }),
        };
      }
      if (event.kind === "turn_notice") {
        return {
          ...state,
          messages: state.messages.map((m) =>
            m.id === msgId
              ? {
                  ...m,
                  gateActions: [
                    ...(m.gateActions ?? []),
                    { kind: "turn_notice", detail: event.detail },
                  ],
                }
              : m,
          ),
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
