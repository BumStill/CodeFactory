// SPDX-License-Identifier: Apache-2.0
//
// 派单处理器可以抛出的**结构化**失败。
//
// 为什么单独一个模块：`desktopDispatch.ts`（协议）和 `dispatchActions.ts`
// （业务动作）都要用它，而业务动作不能反向依赖协议以外的任何东西。错误码与
// `headless_dispatch.rs::ErrorBody` 是同一套稳定取值——编排方靠它区分
// "会话不存在"和"内部错误"，而不是去正则匹配一句人话。
//
// CF-HDE-R0 特别需要 `not_found`：带 session_id 的请求定位不到目标会话时，
// 必须明确失败，**绝不退回"界面上当前显示的那个会话"**。

export type DispatchErrorCode =
  | "invalid_request"
  | "denied"
  | "not_found"
  | "delivery_failed"
  | "internal";

export class DispatchRequestError extends Error {
  readonly code: DispatchErrorCode;

  constructor(code: DispatchErrorCode, message: string) {
    super(message);
    this.name = "DispatchRequestError";
    this.code = code;
  }
}

export function isDispatchRequestError(error: unknown): error is DispatchRequestError {
  return error instanceof DispatchRequestError;
}

/** 把任意异常压成一句能给编排方看的话。 */
export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
