// SPDX-License-Identifier: Apache-2.0
//
// CF-HDE-R0 / R7 / R8 / R9 / R10 —— 本地派单入口第二阶段：业务动作。
//
// 第二阶段真机实测暴露的头号缺陷是**作用对象错**：带 session_id 的 send / stop
// 落到了"界面当前显示的那个会话"上。旧实现是"先 onOpenSession(sessionId)，再让
// 输入框把消息发出去"——切会话是异步的，输入框还绑在上一个会话上，于是 14:30
// 发给 5bb60a3a 的修复指令（带 delivery_authorized=true）落进了 be9674c5，
// 14:42 发给 5bb60a3a 的 stop 停掉了 40704077。跨会话投递还带着交付授权，这是
// 安全边界缺陷，不是体验问题。
//
// 这一层因此彻底不碰"界面显示哪个会话"：每个动作都以 session_id 为主语，直接调
// store 里**按会话寻址**的那条路径（`sendMessage(content, id)` / `cancelStream(id)`
// / 会话自己的 queue）。界面显示谁与这里无关；定位不到就 `not_found`，绝不退回
// 当前会话。
//
// 第二个缺陷是**送达撒谎**（R7）：旧 `send` 无条件回 ok。现在每次都回报
// "已送达并开始处理"还是"已排队、排在什么之后"，并且在有界时间内拿不到"这一轮
// 真的开始了"的证据时回 `delivery_failed`。

import { invoke } from "./tauri";
import { useChatStore, type SessionRuntime } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import { deliveryReferenceFromMessages } from "./deliveryReference";
import { DispatchRequestError, describeError } from "./dispatchErrors";
import { sessionTurnState, type SessionTurnState } from "./sessionTurnState";
import type { DispatchSendMode } from "./desktopDispatch";
import type { ModelInfo, PermissionMode, Session } from "./tauri";

/** 有界等待：拿不到"这一轮真的开始了"的证据就不算送达（R7）。 */
export const DISPATCH_DELIVERY_TIMEOUT_MS = 4_000;

/** `queued` 的 `ahead`：排在**当前这一轮**之后。 */
export const DISPATCH_DELIVERY_AHEAD_CURRENT_RUN = "current_run";

export interface DispatchDeliveryReceipt {
  status: "delivered" | "queued";
  /** `delivered` 时：进了新一轮，还是插进了当前这一轮。 */
  into?: "new_turn" | "current_run";
  /** `queued` 时：排在第几位（1 = 下一个）。 */
  queue_position?: number;
  /** `queued` 时：排在什么之后。 */
  ahead?: string;
}

export interface DispatchSendResult {
  session_id: string;
  delivery: DispatchDeliveryReceipt;
}

export interface DispatchStatusResult {
  session_id: string;
  title: string;
  cwd: string;
  permission_mode: PermissionMode | undefined;
  model: string;
  /** 与真实执行一致的状态（R9）：在模型调用就不会说"等待重试"。 */
  objective_state: SessionTurnState["objectiveState"];
  running: boolean;
  waiting_permission: boolean;
  /** 最近一条助理回复的摘要（截断），没有则 null。 */
  latest_reply: string | null;
  /** 交付记录里的 PR 号，没有则 null。 */
  pr_number: number | null;
  /** 最近一次模型调用的上下文大小（token），未知则 null。 */
  context_tokens: number | null;
}

/**
 * 按 session_id 定位会话。定位不到**永远**抛 `not_found`：绝不退回当前会话。
 */
function requireSession(sessionId: string): Session {
  const store = useChatStore.getState();
  const session =
    store.activeSession?.id === sessionId
      ? store.activeSession
      : store.sessions.find((candidate) => candidate.id === sessionId);
  if (!session) {
    throw new DispatchRequestError("not_found", `unknown session ${sessionId}`);
  }
  return session;
}

function runtimeOf(sessionId: string): SessionRuntime | undefined {
  return useChatStore.getState().runtime[sessionId];
}

/** 这个会话此刻真实的回合状态（服务器权威标志 + 未释放的进行中回合）。 */
export function sessionTurnStateOf(session: Session): SessionTurnState {
  const runtime = runtimeOf(session.id);
  return sessionTurnState({
    streaming: runtime?.streaming ?? false,
    sessionIsRunning: session.is_running === true,
    messages: runtime?.messages ?? [],
    waitingPermission: runtime?.pendingPermission != null,
  });
}

async function withDeadline<T>(work: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return work;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new DispatchRequestError("delivery_failed", message)),
      timeoutMs,
    );
  });
  try {
    return await Promise.race([work, timeout]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/**
 * 一次发送到底发生了什么（R7）。只认两件事：新的一轮真的追加了（并且没有被
 * 后端拒绝），或者消息确实进了**这个会话**的待发队列。其余一律是失败。
 */
function classifyDelivery(input: {
  before: SessionRuntime | undefined;
  after: SessionRuntime | undefined;
  message: string;
  sinceMs: number;
}): DispatchDeliveryReceipt | null {
  const after = input.after;
  if (!after) return null;
  const text = input.message.trim();
  const appended = after.messages.filter(
    (message) =>
      message.role === "user" && message.content.trim() === text && message.createdAt >= input.sinceMs,
  );
  const queued = after.queue.length > (input.before?.queue.length ?? 0);
  if (queued && appended.length === 0) {
    return {
      status: "queued",
      queue_position: after.queue.length,
      ahead: DISPATCH_DELIVERY_AHEAD_CURRENT_RUN,
    };
  }
  const user = appended[appended.length - 1];
  if (!user) return null;
  // 后端明确拒绝过这一轮：气泡还在，但带着失败证据。`presentChatInvocationError`
  // 是界面唯一的那份失败呈现，它要么写 `runtimeError`/`failureEvidence`，要么把
  // 内容写成 `Error: …`——哪一种是哪种都算"这一轮没起来"，不能报 ok。
  const assistant = after.messages.find(
    (message) => message.role === "assistant" && message.rootTurnId === user.id,
  );
  if (
    assistant &&
    (assistant.runtimeError || assistant.failureEvidence || /^\s*Error:/i.test(assistant.content))
  ) {
    return null;
  }
  return { status: "delivered", into: "new_turn" };
}

/**
 * CF-HDE-R8：运行中的会话往哪送。
 *   * `steer`（默认）= 界面按 Enter —— 插话引导当前执行；
 *   * `queue` = 界面按 ⌘/Ctrl+Enter —— 这一轮结束之后再发。
 * 空闲会话下两种都是"立即开始新的一轮"。
 */
export async function dispatchSend(
  input: { sessionId: string; message: string; mode: DispatchSendMode },
  options: { timeoutMs?: number } = {},
): Promise<DispatchSendResult> {
  const session = requireSession(input.sessionId);
  const timeoutMs = options.timeoutMs ?? DISPATCH_DELIVERY_TIMEOUT_MS;
  const before = runtimeOf(session.id);
  const state = sessionTurnStateOf(session);

  if (state.running) {
    if (input.mode === "queue") {
      const queued = useChatStore.getState().enqueueMessage(session.id, input.message);
      if (queued !== "queued") {
        throw new DispatchRequestError(
          "delivery_failed",
          queued === "full"
            ? `session ${session.id}'s queue is full`
            : `session ${session.id} could not accept a queued message`,
        );
      }
      const after = runtimeOf(session.id);
      return {
        session_id: session.id,
        delivery: {
          status: "queued",
          queue_position: after?.queue.length ?? 1,
          ahead: DISPATCH_DELIVERY_AHEAD_CURRENT_RUN,
        },
      };
    }
    await withDeadline(
      useChatStore.getState().steerRun(input.message, session.id),
      timeoutMs,
      `session ${session.id} did not accept the interjection in time`,
    );
    return {
      session_id: session.id,
      delivery: { status: "delivered", into: "current_run" },
    };
  }

  // 空闲（或只剩一个已经结算的旧回合残留）：现在就开新的一轮。
  const sinceMs = Date.now();
  await withDeadline(
    useChatStore.getState().sendMessage(input.message, session.id),
    timeoutMs,
    `session ${session.id} did not start a new turn in time`,
  );
  const receipt = classifyDelivery({
    before,
    after: runtimeOf(session.id),
    message: input.message,
    sinceMs,
  });
  if (!receipt) {
    throw new DispatchRequestError(
      "delivery_failed",
      `session ${session.id} did not start a new turn`,
    );
  }
  return { session_id: session.id, delivery: receipt };
}

/**
 * CF-HDE-R0：停止的是 session_id 指定的会话。省略 session_id 时仍是"当前会话"
 * （这是文档承诺的默认值）；给了却定位不到，就是 not_found。
 */
export async function dispatchStop(input: {
  sessionId?: string;
}): Promise<{ stopped: boolean; session_id: string }> {
  const store = useChatStore.getState();
  const id = input.sessionId ? requireSession(input.sessionId).id : store.activeSession?.id;
  if (!id) throw new DispatchRequestError("not_found", "no session is open to stop");
  const stopped = await store.cancelStream(id);
  return { stopped, session_id: id };
}

/** CF-HDE-R0：改的是 session_id 指定的会话的权限模式，界面不会被切走。 */
export async function dispatchSetPermission(input: {
  sessionId: string;
  mode: PermissionMode;
}): Promise<{ session_id: string; permission_mode: PermissionMode | undefined }> {
  const session = requireSession(input.sessionId);
  const updated = await useChatStore.getState().updateSessionPermissionMode(session.id, input.mode);
  return { session_id: updated.id, permission_mode: updated.permission_mode };
}

/**
 * CF-HDE-R10：带 session_id 时改那个会话的模型（端点保持该会话自己的）；
 * 省略时设置**新会话的默认模型**，并且端点与模型必须配套——验不过就 fail-closed，
 * 绝不写出"端点是 deepseek、默认模型是 gpt"这种错配。
 */
export async function dispatchSetModel(input: {
  sessionId?: string;
  model: string;
}): Promise<
  | { scope: "session"; session_id: string; model: string; endpoint: string | null }
  | { scope: "default"; model: string; endpoint: string }
> {
  if (input.sessionId) {
    const session = requireSession(input.sessionId);
    if (session.kind === "anonymous" || session.kind === "quick") {
      // 这些会话没有数据库行可写（匿名会话永不落库），只更新内存投影。
      const updated: Session = { ...session, model_id: input.model };
      useChatStore.setState((state) => ({
        sessions: state.sessions.map((item) => (item.id === updated.id ? updated : item)),
        activeSession: state.activeSession?.id === updated.id ? updated : state.activeSession,
        activeModel: state.activeSession?.id === updated.id ? updated.model_id : state.activeModel,
      }));
      return { scope: "session", session_id: updated.id, model: updated.model_id, endpoint: updated.endpoint_id ?? null };
    }
    const updated = await invoke<Session>("update_session_model", {
      sessionId: session.id,
      modelId: input.model,
    });
    useChatStore.setState((state) => ({
      sessions: state.sessions.map((item) => (item.id === updated.id ? updated : item)),
      activeSession: state.activeSession?.id === updated.id ? updated : state.activeSession,
      activeModel: state.activeSession?.id === updated.id ? updated.model_id : state.activeModel,
    }));
    return {
      scope: "session",
      session_id: updated.id,
      model: updated.model_id,
      endpoint: updated.endpoint_id ?? null,
    };
  }

  // 默认模型：走界面选择器同一条路（端点自己的 active_model + 全局默认）。
  const settings = useSettingsStore.getState().settings;
  const endpoint = settings?.default_endpoint?.trim();
  if (!settings || !endpoint) {
    throw new DispatchRequestError(
      "invalid_request",
      "no endpoint is configured to set a default model on",
    );
  }
  let models: ModelInfo[];
  try {
    models = await invoke<ModelInfo[]>("list_models", { endpointName: endpoint });
  } catch (error) {
    throw new DispatchRequestError(
      "internal",
      `could not verify that endpoint ${endpoint} serves ${input.model}: ${describeError(error)}`,
    );
  }
  if (!Array.isArray(models) || !models.some((model) => model.id === input.model)) {
    throw new DispatchRequestError(
      "invalid_request",
      `endpoint ${endpoint} does not serve ${input.model}; refusing to pair endpoint and default model silently`,
    );
  }
  await invoke("set_endpoint_active_model", { endpointName: endpoint, modelId: input.model });
  // 走界面模型选择器同一条保存路径（`useSettingsStore().save`）。
  await useSettingsStore.getState().save({
    ...settings,
    default_endpoint: endpoint,
    default_model: input.model,
  });
  useChatStore.getState().setModel(input.model);
  return { scope: "default", model: input.model, endpoint };
}

/**
 * CF-HDE-R9：状态必须可信且够用。objective_state / latest_reply / pr_number /
 * context_tokens 都来自库内事实（消息投影、交付记录、上下文用量），并与 R11 的
 * 界面提示同源。
 */
export async function dispatchStatus(input: { sessionId: string }): Promise<DispatchStatusResult> {
  const session = requireSession(input.sessionId);
  const runtime = runtimeOf(session.id);
  const messages = runtime?.messages ?? [];
  const reference = deliveryReferenceFromMessages(messages);
  const state = sessionTurnState({
    streaming: runtime?.streaming ?? false,
    sessionIsRunning: session.is_running === true,
    messages,
    waitingPermission: runtime?.pendingPermission != null,
    contextTokens: runtime?.contextUsage?.used ?? null,
    prNumber: reference?.prNumber ?? null,
  });
  return {
    session_id: session.id,
    title: session.title,
    cwd: session.cwd,
    permission_mode: session.permission_mode,
    model: session.model_id,
    objective_state: state.objectiveState,
    running: state.running,
    waiting_permission: state.waitingPermission,
    latest_reply: state.latestReply,
    pr_number: state.prNumber,
    context_tokens: state.contextTokens,
  };
}

/** 待发队列上限由 store 的 `QUEUE_MAX` 决定；入口只回报"排在第几位"，不复制上限。 */
