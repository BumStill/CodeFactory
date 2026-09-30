// SPDX-License-Identifier: Apache-2.0
//
// 把原生菜单栏接到界面动作上。
//
// 这个 hook 只做桥接:监听 `menu:session`,把事件交给 `desktopMenu.ts` 校验并
// 派发给调用方注入的处理器(那些处理器必须是界面按钮已经在用的函数);同时把
// 界面状态同步回原生侧,让菜单项的可用/勾选状态与界面一致。
//
// 为什么必须有同步这一步:菜单项在原生侧是一等公民,它们的灰/亮、勾选状态由
// 原生渲染。如果不同步,用户会看到"菜单里能点、点了没反应",或者"界面已经在
// 运行,菜单里的停止却是灰的"。

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

import { invoke, type PermissionMode, type Session } from "./tauri";
import {
  buildMenuSyncState,
  dispatchMenuEvent,
  SESSION_MENU_EVENT,
  type DesktopMenuEvent,
  type DesktopMenuHandlers,
  type MenuContext,
} from "./desktopMenu";

/** 当前是否运行在真实 Tauri 宿主里。浏览器/单测环境下没有原生菜单。 */
function hasNativeHost(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export interface DesktopMenuBridgeInput {
  /** 最近会话(界面顺序,最新在前)。 */
  sessions: Session[];
  /** 当前打开的会话 id;草稿没有 id,传 null。 */
  openSessionId: string | null;
  /** 当前会话/草稿的权限模式。 */
  permissionMode: PermissionMode;
  /** 有没有当前项目。 */
  hasProject: boolean;
  /** 有没有正在进行的执行。 */
  running: boolean;
  /** 处理器的每个字段都必须是界面按钮在用的那个函数,不能另写一套逻辑。 */
  handlers: DesktopMenuHandlers;
}

export function useDesktopMenuBridge(input: DesktopMenuBridgeInput): void {
  // 事件到达时要用"此刻"的状态与处理器,而不是监听那一刻的闭包快照。
  const contextRef = useRef<MenuContext>({ hasProject: input.hasProject, running: input.running });
  contextRef.current = { hasProject: input.hasProject, running: input.running };
  const handlersRef = useRef<DesktopMenuHandlers>(input.handlers);
  handlersRef.current = input.handlers;

  useEffect(() => {
    if (!hasNativeHost()) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<DesktopMenuEvent>(SESSION_MENU_EVENT, (event) => {
      void dispatchMenuEvent(event.payload, contextRef.current, handlersRef.current);
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // 没有原生菜单可用(例如纯前端调试):界面按钮仍然完全可用。
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const syncState = buildMenuSyncState({
    sessions: input.sessions,
    openSessionId: input.openSessionId,
    permissionMode: input.permissionMode,
    hasProject: input.hasProject,
    running: input.running,
  });
  // 用序列化结果当依赖:状态没变就不重复跨进程同步(每次同步都会改原生菜单)。
  const syncKey = JSON.stringify(syncState);

  useEffect(() => {
    if (!hasNativeHost()) return;
    void invoke("sync_session_menu", { state: JSON.parse(syncKey) }).catch(() => {
      // 同步失败不能让界面报错:下一次状态变化会再同步一次。
    });
  }, [syncKey]);
}
