// SPDX-License-Identifier: Apache-2.0
//
// 把原生"本地派单入口"接到界面动作上。
//
// 和 `useDesktopMenuBridge` 是同一个模式：监听原生事件 -> 交给纯函数校验并翻译
// -> 调用界面已经在用的处理器 -> 把结果回给原生侧。区别只有一点：菜单事件是
// 单向下发，派单请求需要**应答**（编排方要拿到会话状态、PR 号这类结果）。
//
// 处理器的每个字段都必须是界面按钮在用的那个函数，不能另写一套逻辑——否则
// "走同一条准入"就只是口号。

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

import { invoke } from "./tauri";
import {
  applyDispatchRequest,
  dispatchReplyArgs,
  DISPATCH_REQUEST_EVENT,
  type DispatchHandlers,
  type DispatchRequestEvent,
} from "./desktopDispatch";

/** 当前是否运行在真实 Tauri 宿主里。浏览器/单测环境下没有原生通道。 */
function hasNativeHost(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export interface DesktopDispatchBridgeInput {
  /** 处理器的每个字段都必须是界面按钮/菜单复用的那个函数,不能另写一套逻辑。 */
  handlers: DispatchHandlers;
}

export function useDesktopDispatchBridge(input: DesktopDispatchBridgeInput): void {
  // 事件到达时要用"此刻"的处理器，而不是监听那一刻的闭包快照。
  const handlersRef = useRef(input.handlers);
  handlersRef.current = input.handlers;

  useEffect(() => {
    if (!hasNativeHost()) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<DispatchRequestEvent>(DISPATCH_REQUEST_EVENT, (event) => {
      const request = event.payload;
      void applyDispatchRequest(request, handlersRef.current)
        .then((outcome) => invoke("dispatch_reply", dispatchReplyArgs(request.request_id, outcome)))
        .catch((error) => {
          // 连上报都失败时不能说"已完成"：原生侧会超时并把这个请求判为失败。
          console.warn("dispatch request could not be answered", error);
        });
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // 没有原生通道（例如纯前端调试）：界面按钮仍然完全可用。
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
