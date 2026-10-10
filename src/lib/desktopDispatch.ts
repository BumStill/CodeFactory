// SPDX-License-Identifier: Apache-2.0
//
// 本地非界面派单入口 —— 前端侧。
//
// 背景：macOS 锁屏后，窗口级的无障碍通道（菜单项、合成点击、键盘事件）全部
// 失效（`The screen is locked … actions need the screen unlocked`），菜单栏因此
// 覆盖不了"夜里无人值守"的场景。原生侧 `src-tauri/src/headless_dispatch.rs` 提供
// 一个**仅本机、仅当前用户**的 Unix socket 通道；这个文件是它的另一半。
//
// 铁律（和菜单栏完全一致）：原生侧只做"信封"——传输、鉴权、审计、协议校验；
// 业务语义一律留在前端，落到界面按钮已经在用的那条代码路径上。所以这里只做
// 两件事：
//   1. 把原生请求**校验并翻译**成一次调用（`applyDispatchRequest`，纯函数）；
//   2. 把结果编码回原生侧要求的响应形状（`dispatchReplyArgs`，纯函数）。
//
// 校验在这一侧再做一遍不是重复劳动：原生侧挡住的是"协议不合法"，前端挡住的是
// "此刻界面上到底能不能做这件事"。两侧对 `delivery_authorized` 这类安全字段
// 使用同一套规则，`desktopDispatch.test.ts` 会直接读 Rust 源文件交叉断言。

/** 事件名，必须与 `headless_dispatch.rs` 的 `DISPATCH_REQUEST_EVENT` 一致。 */
export const DISPATCH_REQUEST_EVENT = "dispatch:request";

import { isDispatchRequestError } from "./dispatchErrors";

/** 权限模式，与 `headless_dispatch.rs` / `menu_spec.rs` 一致。 */
export const DISPATCH_PERMISSION_MODES = ["safe", "standard", "trusted"] as const;
export type DispatchPermissionMode = (typeof DISPATCH_PERMISSION_MODES)[number];

/**
 * `send` 往哪个方向送一条消息（CF-HDE-R8），与界面键位一一对应：
 *   * `steer` = 界面里不加修饰键按 Enter —— 插话引导当前执行；
 *   * `queue` = 界面里按 ⌘/Ctrl+Enter —— 这一轮结束之后再发。
 * 与 `headless_dispatch.rs` 的 `SEND_MODES` 必须逐字一致。
 */
export const DISPATCH_SEND_MODES = ["steer", "queue"] as const;
export type DispatchSendMode = (typeof DISPATCH_SEND_MODES)[number];

/**
 * 默认 `steer`。理由是它就是界面默认键位的语义：用户不加修饰键按 Enter 时
 * 想要的是"现在就按我说的调整"，而不是"等它忙完再说"。空闲会话下 `steer` 与
 * `queue` 都表现为立即开始新的一轮，所以默认取更主动的那个不会让空闲派单
 * 变成排队。
 */
export const DEFAULT_DISPATCH_SEND_MODE: DispatchSendMode = "steer";

/** 原生侧发过来的请求负载。 */
export interface DispatchRequestEvent {
  request_id: string;
  operation: string;
  request: Record<string, unknown>;
}

/** 结构化失败，`code` 与原生侧稳定取值一致。 */
export interface DispatchError {
  code: "invalid_request" | "denied" | "not_found" | "delivery_failed" | "internal";
  message: string;
}

export type DispatchOutcome =
  | { ok: true; result: unknown }
  | { ok: false; error: DispatchError };

/** 一条待审批请求（CF-HDE-R6）。 */
export interface DispatchApproval {
  approvalId: string;
  toolName: string;
  sessionId: string;
}

/**
 * 处理器注入：每个字段都必须复用界面已经在用的那个函数。
 * 新增一条业务逻辑就等于多了一条与界面不一致的路径，这里不允许。
 */
export interface DispatchHandlers {
  createAndSend: (input: {
    project: string;
    message: string;
    model?: string;
    permissionMode?: DispatchPermissionMode;
    deliveryAuthorized: boolean;
  }) => Promise<unknown>;
  send: (input: {
    sessionId: string;
    message: string;
    mode: DispatchSendMode;
    deliveryAuthorized: boolean;
  }) => Promise<unknown>;
  setPermission: (input: { sessionId: string; mode: DispatchPermissionMode }) => Promise<unknown>;
  status: (input: { sessionId: string }) => Promise<unknown>;
  setModel: (input: { sessionId?: string; model: string }) => Promise<unknown>;
  switchSession: (input: { sessionId: string }) => Promise<unknown>;
  stop: (input: { sessionId?: string }) => Promise<unknown>;
  listApprovals: () => Promise<DispatchApproval[]>;
  resolveApproval: (input: { approvalId: string; approve: boolean }) => Promise<unknown>;
}

function invalid(message: string): DispatchOutcome {
  return { ok: false, error: { code: "invalid_request", message } };
}

function requiredString(value: unknown, field: string): string {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`${field} must be a non-empty string`);
  }
  return value;
}

function optionalString(value: unknown, field: string): string | undefined {
  if (value === undefined || value === null) return undefined;
  return requiredString(value, field);
}

/**
 * 交付授权只能来自结构化字段，绝不从文字推断（M48）。
 *
 * 与原生侧同一条规则：字段缺失就不是一个合法请求。消息里写着"merge the PR"
 * 但没带这个字段，一样拒绝。
 */
function requiredDeliveryAuthorization(value: unknown): boolean {
  if (typeof value !== "boolean") {
    throw new Error("delivery_authorized must be an explicit boolean");
  }
  return value;
}

function requiredPermissionMode(value: unknown): DispatchPermissionMode {
  const mode = requiredString(value, "permission_mode");
  if (!(DISPATCH_PERMISSION_MODES as readonly string[]).includes(mode)) {
    throw new Error(`permission_mode must be one of ${DISPATCH_PERMISSION_MODES.join(", ")}`);
  }
  return mode as DispatchPermissionMode;
}

/**
 * `send` 的方向值。缺省由调用方补成 `steer`（见 `DEFAULT_DISPATCH_SEND_MODE`）；
 * 一旦显式给出就必须是这两个之一 —— 猜错方向等于用户以为在插话、其实在排队。
 */
function requiredSendMode(value: unknown): DispatchSendMode {
  const mode = requiredString(value, "mode");
  if (!(DISPATCH_SEND_MODES as readonly string[]).includes(mode)) {
    throw new Error(`mode must be one of ${DISPATCH_SEND_MODES.join(", ")}`);
  }
  return mode as DispatchSendMode;
}

/**
 * 校验 + 翻译 + 派发。原生请求的完整前端入口。
 *
 * 纯函数（处理器由调用方注入），因此每条 CF-HDE-R6 能力都能在单测里断言
 * "请求 -> 调了哪个界面动作"，不需要真的开一个 socket。
 */
export async function applyDispatchRequest(
  event: DispatchRequestEvent,
  handlers: DispatchHandlers,
): Promise<DispatchOutcome> {
  const body = event.request ?? {};
  try {
    let result: unknown;
    switch (event.operation) {
      case "create_and_send":
        result = await handlers.createAndSend({
          project: requiredString(body.project, "project"),
          message: requiredString(body.message, "message"),
          model: optionalString(body.model, "model"),
          permissionMode:
            body.permission_mode === undefined
              ? undefined
              : requiredPermissionMode(body.permission_mode),
          deliveryAuthorized: requiredDeliveryAuthorization(body.delivery_authorized),
        });
        break;
      case "send":
        result = await handlers.send({
          sessionId: requiredString(body.session_id, "session_id"),
          message: requiredString(body.message, "message"),
          mode:
            body.mode === undefined
              ? DEFAULT_DISPATCH_SEND_MODE
              : requiredSendMode(body.mode),
          deliveryAuthorized: requiredDeliveryAuthorization(body.delivery_authorized),
        });
        break;
      case "set_permission":
        result = await handlers.setPermission({
          sessionId: requiredString(body.session_id, "session_id"),
          mode: requiredPermissionMode(body.permission_mode),
        });
        break;
      case "status":
        result = await handlers.status({
          sessionId: requiredString(body.session_id, "session_id"),
        });
        break;
      case "set_model":
        result = await handlers.setModel({
          sessionId: optionalString(body.session_id, "session_id"),
          model: requiredString(body.model, "model"),
        });
        break;
      case "switch_session":
        result = await handlers.switchSession({
          sessionId: requiredString(body.session_id, "session_id"),
        });
        break;
      case "stop":
        result = await handlers.stop({
          sessionId: optionalString(body.session_id, "session_id"),
        });
        break;
      case "list_approvals":
        result = await handlers.listApprovals();
        break;
      case "resolve_approval":
        result = await handlers.resolveApproval({
          approvalId: requiredString(body.approval_id, "approval_id"),
          // 审批永远是一条一条来的：这里只是把"批"或"拒"原样转达，
          // 从不做批量放行，也不把高危操作变成默认通过。
          approve: requiredDeliveryBoolean(body.approve, "approve"),
        });
        break;
      default:
        // 未知动作不是"尽力而为"的理由：猜错的代价是用户以为派了单其实没派。
        // `focus_main_display` 也在原生侧直接处理，不该发到前端。
        return invalid(`unsupported operation: ${event.operation}`);
    }
    return { ok: true, result: result ?? null };
  } catch (error) {
    // 结构化的执行失败（会话不存在、送达失败）原样上报，绝不塌成一句人话：
    // 编排方靠 code 区分"没有这个会话"与"内部错误"（CF-HDE-R0 / R7）。
    if (isDispatchRequestError(error)) {
      return { ok: false, error: { code: error.code, message: error.message } };
    }
    const message = error instanceof Error ? error.message : String(error);
    // 校验失败与执行失败都是 fail-closed：请求没有被执行，客户端必须知道。
    return /must be|unsupported/.test(message)
      ? invalid(message)
      : { ok: false, error: { code: "internal", message } };
  }
}

function requiredDeliveryBoolean(value: unknown, field: string): boolean {
  if (typeof value !== "boolean") {
    throw new Error(`${field} must be an explicit boolean`);
  }
  return value;
}

/** 原生 `dispatch_reply` 命令要求的参数形状（JS 侧 camelCase）。 */
export function dispatchReplyArgs(
  requestId: string,
  outcome: DispatchOutcome,
): { requestId: string; ok: boolean; result?: unknown; error?: DispatchError } {
  return outcome.ok
    ? { requestId, ok: true, result: outcome.result ?? null }
    : { requestId, ok: false, error: outcome.error };
}
