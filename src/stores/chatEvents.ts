// SPDX-License-Identifier: Apache-2.0
import type { StreamEvent } from "../lib/tauri.js";
import { turnPlanFromEvent, type TurnPlan } from "../lib/chatPlan.js";

export interface ToolCallState {
  id: string;
  name: string;
  args: string;
  result?: string;
  isError?: boolean;
  metadata?: Record<string, unknown> | null;
  status: "waiting_permission" | "running" | "waiting" | "done" | "blocked" | "error" | "denied" | "cancelled";
}

export interface TurnActivityState {
  rootTurnId: string;
  revision: number;
  phase: string;
  status: string;
  kind: string;
  label: string;
  waitingReason: string | null;
  updatedAt: number;
  terminalReason: string | null;
  objectiveId?: string;
  objectiveStatus?: "active" | "waiting_system" | "waiting_core_input" | "waiting_authorization" | "waiting_business_decision" | "completed" | "cancelled" | "legacy_orphan";
  recoveryOwner?: string | null;
  nextObservationAt?: number | null;
  lastProgressAt?: number | null;
}

export interface PendingPermission {
  intentId: string;
  toolCallId: string;
  toolName: string;
  args: unknown;
  expiresAt?: number;
}

export interface UIMessage {
  id: string;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  /** Frontend root-turn identity retained across live assistant segments.
   * Unlike plan/activity projection, it exists before the first stream event
   * and survives the brief gap created when a steer splits the bubble. */
  rootTurnId?: string;
  toolCalls?: ToolCallState[];
  transportRetries?: TransportRetryState[];
  /** Technical route failures retained behind an expandable disclosure when
   * every configured candidate has been exhausted. */
  failureEvidence?: string;
  runtimeError?: {
    code: string;
    endpointId?: string | null;
    recoverable: boolean;
  };
  inputTokens?: number;
  outputTokens?: number;
  createdAt: number;
  /** Wall-clock the turn took, frozen only when the durable run settles.
   * Transport completion/error can still be followed by recovery work. */
  durationMs?: number;
  /** Internal completion-review provenance from the DB. Drafts and
   *  injected review instructions are excluded from the chat transcript. */
  completionState?: string;
  /** User-facing runtime warnings/notices retained for this turn. */
  gateActions?: GateActionState[];
  /** A steer typed mid-run that the loop has not reached yet. Shown as sent
   *  but undelivered, because until a round boundary drains it the model has
   *  genuinely not seen it. Cleared by `steer_applied`. */
  steerPending?: boolean;
  /** Turn timeline: narration and tool calls in arrival order. Only built
   *  during live streaming — hydrated history is already interleaved as
   *  separate rows. */
  segments?: TurnSegment[];
  /** Structured execution route for this root turn. It is persisted as a
   * bounded plan-event snapshot and hydrated onto the final assistant row. */
  plan?: TurnPlan;
  /** Single latest runtime snapshot for a turn. Revisions replace each other
   * instead of appending more transcript rows. */
  turnActivity?: TurnActivityState;
  /** Bounded, turn-wide tool evidence attached during history hydration so
   * the result snapshot remains truthful after a session is reopened. */
  turnToolCalls?: ToolCallState[];
  turnToolCallCount?: number;
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

const MODEL_ROUTE_EXHAUSTED_PREFIX = "所有可用模型端点均不可用：";
export const MODEL_ROUTE_EXHAUSTED_GUIDANCE =
  "所有已配置且有凭据的模型端点都暂时不可用。目标与失败证据已保留；系统将按退避策略重新观测可用路由。";

export function isModelRouteExhaustedError(message: string): boolean {
  return message.startsWith(MODEL_ROUTE_EXHAUSTED_PREFIX);
}

function credentialFailureGuidance(message: string): string | null {
  const details = message.slice(MODEL_ROUTE_EXHAUSTED_PREFIX.length);
  const route = details.split("（", 1)[0]?.trim();
  if (!route) return null;
  if (details.includes("AUTH_MISSING")) {
    return `${route} 当前不可用：尚未配置凭据。请打开模型设置配置该端点的 API Key；保存后系统会从安全检查点恢复。`;
  }
  if (details.includes("CREDENTIAL_ACCESS_REQUIRED")) {
    return `${route} 当前不可用：无法读取已配置凭据，需要一次密钥访问授权。请在模型设置完成授权或重新保存该端点的 API Key；授权完成后系统会从安全检查点恢复。`;
  }
  return null;
}

export function presentChatInvocationError(error: unknown): Pick<
  UIMessage,
  "content" | "failureEvidence"
> {
  const message = String(error).replace(/^Error:\s*/i, "");
  if (isModelRouteExhaustedError(message)) {
    return {
      content: credentialFailureGuidance(message) ?? MODEL_ROUTE_EXHAUSTED_GUIDANCE,
      failureEvidence: message,
    };
  }
  return { content: `Error: ${message}` };
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
  /** The current run emitted a successful transport-level `done`. Settlement
   * may arrive later; only a completed settlement with this evidence may
   * trigger post-mortem work. Optional for legacy/test state fixtures. */
  transportDoneSucceeded?: boolean;
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

const TRANSIENT_TOOL_STATUSES = new Set<ToolCallState["status"]>([
  "waiting_permission",
  "running",
  "waiting",
]);

function terminalizeTransientTools(message: UIMessage): UIMessage {
  const terminalize = (tool: ToolCallState): ToolCallState =>
    TRANSIENT_TOOL_STATUSES.has(tool.status)
      ? {
          ...tool,
          status: "blocked",
          result: "系统已停止自动恢复，当前步骤未继续执行。",
          isError: false,
        }
      : tool;
  return {
    ...message,
    toolCalls: message.toolCalls?.map(terminalize),
    turnToolCalls: message.turnToolCalls?.map(terminalize),
  };
}

export function reduceChatStreamEvent(
  state: ChatEventState,
  event: StreamEvent,
  msgId: string,
): ChatEventState {
  switch (event.type) {
    case "plan_updated":
      return {
        ...state,
        messages: updateMessageById(state.messages, msgId, (message) => {
          if (message.plan && message.plan.revision >= event.revision) return message;
          const plan = turnPlanFromEvent(event);
          const waitingHistory = [
            ...(message.plan?.waitingHistory ?? []),
            ...(plan.waitingHistory ?? []),
          ].filter((reason, index, all) => all.indexOf(reason) === index).slice(-10);
          const changeHistory = [
            ...(message.plan?.changeHistory ?? []),
            ...(plan.changeHistory ?? []),
          ].filter((reason, index, all) => all.indexOf(reason) === index).slice(-10);
          return {
            ...message,
            plan: { ...plan, waitingHistory, changeHistory },
          };
        }),
      };

    case "turn_activity_updated": {
      const releasedToUser = event.objective_status === "waiting_core_input";
      return {
        ...state,
        ...(releasedToUser ? { streaming: false, pendingPermission: null } : {}),
        messages: updateMessageById(state.messages, msgId, (message) => {
          if (
            message.turnActivity &&
            message.turnActivity.revision >= event.revision
          ) {
            return message;
          }
          const projected = {
            ...message,
            turnActivity: {
              rootTurnId: event.root_turn_id,
              revision: event.revision,
              phase: event.phase,
              status: event.status,
              kind: event.recent_activity_kind,
              label: event.recent_activity_label,
              waitingReason: event.waiting_reason ?? null,
              updatedAt: event.updated_at,
              terminalReason: event.terminal_reason ?? null,
              objectiveId: event.objective_id,
              objectiveStatus: event.objective_status,
              recoveryOwner: event.recovery_owner ?? null,
              nextObservationAt: event.next_observation_at ?? null,
              lastProgressAt: event.last_progress_at ?? null,
            },
          };
          return releasedToUser ? terminalizeTransientTools(projected) : projected;
        }),
      };
    }

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
      // update_plan is a control-plane event. Its user-facing representation
      // is the structured progress card emitted by plan_updated, never a
      // low-level tool card in the transcript.
      if (event.name === "update_plan") return state;
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
          intentId: event.intent_id,
          toolCallId: event.tool_call_id,
          toolName: event.tool_name,
          args: event.args,
          expiresAt: event.expires_at,
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
              ? {
                  ...tc,
                  result: event.content,
                  isError: event.is_error,
                  status: nextStatus,
                  metadata: event.metadata,
                }
              : tc,
          ),
        })),
      };
    }

    case "done": {
      return {
        ...state,
        transportDoneSucceeded: true,
        inputTokenTotal: state.inputTokenTotal + event.input_tokens,
        outputTokenTotal: state.outputTokenTotal + event.output_tokens,
      };
    }

    case "error": {
      const modelRoutesExhausted = isModelRouteExhaustedError(event.message);
      return {
        ...state,
        transportDoneSucceeded: false,
        messages: updateMessageById(state.messages, msgId, (m) => {
          if (modelRoutesExhausted) {
            const presentation = presentChatInvocationError(event.message);
            return {
              ...m,
              content: presentation.content,
              failureEvidence: presentation.failureEvidence,
            };
          }
          return {
            ...m,
            content: m.content + `\n\nError: ${event.message}`,
          };
        }),
      };
    }

    case "runtime_error": {
      return {
        ...state,
        transportDoneSucceeded: false,
        messages: updateMessageById(state.messages, msgId, (message) => ({
          ...message,
          content: event.message,
          runtimeError: {
            code: event.code,
            endpointId: event.endpoint_id,
            recoverable: event.recoverable,
          },
        })),
      };
    }

    case "turn_settled": {
      const endedAt = Date.now();
      const terminalForTurn = event.status !== "waiting_system";
      return {
        ...state,
        streaming: false,
        pendingPermission: null,
        messages: updateMessageById(state.messages, msgId, (message) => {
          const settled = message.durationMs == null
            ? { ...message, durationMs: Math.max(0, endedAt - message.createdAt) }
            : message;
          return terminalForTurn ? terminalizeTransientTools(settled) : settled;
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

    case "steer_applied": {
      // Confirm the oldest still-pending steer with this text. Matching by
      // content (not id) because the optimistic bubble was created before the
      // backend had persisted anything to give it an id.
      let confirmed = false;
      return {
        ...state,
        messages: state.messages.map((m) => {
          if (confirmed || !m.steerPending || m.content !== event.content) return m;
          confirmed = true;
          return { ...m, steerPending: undefined, id: event.message_id ?? m.id };
        }),
      };
    }

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
