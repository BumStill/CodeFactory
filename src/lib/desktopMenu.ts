// SPDX-License-Identifier: Apache-2.0
//
// 原生菜单栏 → 前端动作的唯一分发点。
//
// 为什么要有这一层:编排方(以及读屏软件用户)需要在窗口不活动时也能操作
// CodeFactory,而 WKWebView 在非活动窗口里不处理键盘事件。菜单栏是唯一可靠
// 的原生通道,于是每个日常操作都要在菜单里出现一次。
//
// 铁律:菜单项不另写一套业务逻辑。每个菜单事件都必须落到界面按钮用的那条
// 代码路径上(新建 = sidebar 的新建按钮,切换 = sidebar 的会话按钮,发送 =
// 输入框的发送按钮……)。所以这里只做两件事:
//  1. 把原生事件**校验并翻译**成一个意图(`resolveMenuIntent`,纯函数);
//  2. 把意图交给调用方注入的处理器(`dispatchMenuEvent`)。
// 校验放在这里而不是原生侧:菜单项的可用状态可能与瞬间状态有毫秒级偏差,
// 前端是唯一知道"现在到底能不能做这件事"的地方。

import type { PermissionMode, Session } from "./tauri";
import { sessionAccessibleName } from "./sessionLabel";

/** 原生侧(`src-tauri/src/menu_spec.rs`)使用的稳定动作 id。
 *
 * 两侧必须逐字一致:后端菜单项 id、事件 payload 的 `action`,以及这里的
 * 常量。`desktopMenu.test.ts` 会直接读 Rust 源文件做交叉断言 —— 任何一边
 * 改了名字而对不上,测试立刻失败,而不是等到用户点了菜单没反应。 */
export const MENU_ACTION = {
  NEW_SESSION: "session.new",
  SWITCH_SESSION: "session.switch",
  SET_PERMISSION_MODE: "session.permission",
  FOCUS_INPUT: "session.focus-input",
  SEND_INPUT: "session.send",
  SEND_CLIPBOARD: "session.send-clipboard",
  STOP_RUN: "session.stop",
} as const;

export type MenuActionId = (typeof MENU_ACTION)[keyof typeof MENU_ACTION];

/** 后端会派发的全部动作 id。 */
export const MENU_ACTION_IDS: MenuActionId[] = [
  MENU_ACTION.NEW_SESSION,
  MENU_ACTION.SWITCH_SESSION,
  MENU_ACTION.SET_PERMISSION_MODE,
  MENU_ACTION.FOCUS_INPUT,
  MENU_ACTION.SEND_INPUT,
  MENU_ACTION.SEND_CLIPBOARD,
  MENU_ACTION.STOP_RUN,
];

/** 菜单栏「权限模式」子菜单的三个模式,单选。 */
export const MENU_PERMISSION_MODES: PermissionMode[] = ["safe", "standard", "trusted"];

/** 事件名,必须与 `menu.rs` 的 `SESSION_MENU_EVENT` 一致。 */
export const SESSION_MENU_EVENT = "menu:session";

/** 原生侧读到的相对时间 / 会话列表上限,与 `menu_spec.rs` 一致。 */
export const MENU_SESSION_LIMIT = 20;

/** 原生发过来的事件负载。 */
export interface DesktopMenuEvent {
  action: string;
  /** `session.switch` 专用。 */
  sessionId?: string;
  /** `session.permission` 专用。 */
  mode?: string;
  /** `session.send-clipboard` 专用:原生读到的剪贴板文本。 */
  text?: string;
}

/** 界面此刻的真实状态。菜单项灰不灰由它决定,校验也在这里再兜一次。 */
export interface MenuContext {
  /** 有没有当前项目。没有项目时"新建会话(当前项目)"不可用。 */
  hasProject: boolean;
  /** 有没有正在进行的执行。没有时"停止当前执行"不可用。 */
  running: boolean;
}

/** 翻译后的意图。处理器只认这几种,不认识原生菜单 id。 */
export type MenuIntent =
  | { kind: "new-session" }
  | { kind: "switch-session"; sessionId: string }
  | { kind: "set-permission-mode"; mode: PermissionMode }
  | { kind: "focus-input" }
  | { kind: "send-composer" }
  | { kind: "send-text"; text: string }
  | { kind: "stop-run" };

function isPermissionMode(value: string | undefined): value is PermissionMode {
  return !!value && (MENU_PERMISSION_MODES as string[]).includes(value);
}

/**
 * 把原生菜单事件翻译成意图;现在做不了的事返回 `null`(静默忽略)。
 *
 * 纯函数:菜单项可用状态、快捷键、动态 id 都能在这里被断言,不需要真的点开
 * 原生菜单。
 */
export function resolveMenuIntent(event: DesktopMenuEvent, context: MenuContext): MenuIntent | null {
  switch (event.action) {
    case MENU_ACTION.NEW_SESSION:
      // 没有当前项目时"在当前项目里新建会话"没有意义 —— 菜单里也已经灰掉,
      // 这里再兜一次,防止状态同步晚一拍时点了个寂寞。
      return context.hasProject ? { kind: "new-session" } : null;
    case MENU_ACTION.SWITCH_SESSION: {
      const sessionId = event.sessionId?.trim();
      return sessionId ? { kind: "switch-session", sessionId } : null;
    }
    case MENU_ACTION.SET_PERMISSION_MODE: {
      // 只有后端认可的三种模式能落库(后端 `validate_permission_mode` 会拒绝
      // 其它取值),所以非法值在这里就断掉,不产生一次注定失败的写入。
      return isPermissionMode(event.mode) ? { kind: "set-permission-mode", mode: event.mode } : null;
    }
    case MENU_ACTION.FOCUS_INPUT:
      return { kind: "focus-input" };
    case MENU_ACTION.SEND_INPUT:
      // 发的是输入框"此刻真实"的内容,由输入框组件自己读 DOM。
      return { kind: "send-composer" };
    case MENU_ACTION.SEND_CLIPBOARD: {
      const text = event.text ?? "";
      // 空剪贴板不发送:和手工输入后按发送完全同一条校验。
      return text.trim() ? { kind: "send-text", text } : null;
    }
    case MENU_ACTION.STOP_RUN:
      return context.running ? { kind: "stop-run" } : null;
    default:
      // 未知动作不是"尽力而为"的理由:猜错的代价是用户以为点了没生效。
      return null;
  }
}

/** 处理器注入:每个字段都必须是界面按钮已经在用的那个函数。 */
export interface DesktopMenuHandlers {
  newSession: () => void | Promise<void>;
  switchSession: (sessionId: string) => void | Promise<void>;
  setPermissionMode: (mode: PermissionMode) => void | Promise<void>;
  focusInput: () => void;
  /** 发送输入框当前内容(真实 DOM 值)。 */
  sendComposer: () => void | Promise<void>;
  /** 把给定文本放进输入框并发送 —— 走和手工输入后发送完全相同的路径。 */
  sendText: (text: string) => void | Promise<void>;
  stopRun: () => void | Promise<void>;
}

/** 执行一个意图。返回被执行的意图,便于测试断言"点了菜单 -> 调了哪个动作"。 */
export async function applyMenuIntent(
  intent: MenuIntent,
  handlers: DesktopMenuHandlers,
): Promise<MenuIntent> {
  switch (intent.kind) {
    case "new-session":
      await handlers.newSession();
      break;
    case "switch-session":
      await handlers.switchSession(intent.sessionId);
      break;
    case "set-permission-mode":
      await handlers.setPermissionMode(intent.mode);
      break;
    case "focus-input":
      handlers.focusInput();
      break;
    case "send-composer":
      await handlers.sendComposer();
      break;
    case "send-text":
      await handlers.sendText(intent.text);
      break;
    case "stop-run":
      await handlers.stopRun();
      break;
  }
  return intent;
}

/** 校验 + 派发。菜单点击的完整前端入口。 */
export async function dispatchMenuEvent(
  event: DesktopMenuEvent,
  context: MenuContext,
  handlers: DesktopMenuHandlers,
): Promise<MenuIntent | null> {
  const intent = resolveMenuIntent(event, context);
  if (!intent) return null;
  return applyMenuIntent(intent, handlers);
}

/** 原生菜单要同步的状态。字段名与 `menu.rs` 的 `SessionMenuState` 一致。 */
export interface SessionMenuSyncState {
  entries: { id: string; label: string }[];
  currentSessionId: string | null;
  permissionMode: PermissionMode;
  hasProject: boolean;
  running: boolean;
}

/** 由界面状态算出菜单该长什么样。
 *
 * 菜单项的顺序、文案、勾选状态和界面必须一致,否则"菜单里能点"和"界面上
 * 能点"就成了两套真相。切换列表的文案与侧边栏无障碍名称共用
 * `sessionAccessibleName`,所以同名会话在菜单里也能分清。 */
export function buildMenuSyncState(input: {
  sessions: Session[];
  openSessionId: string | null;
  permissionMode: PermissionMode;
  hasProject: boolean;
  running: boolean;
  now?: number;
}): SessionMenuSyncState {
  const now = input.now ?? Date.now();
  // 最近 20 个会话:按界面用的同一份顺序(最新在前)截断。
  const entries = input.sessions.slice(0, MENU_SESSION_LIMIT).map((session) => ({
    id: session.id,
    label: sessionAccessibleName(session.title, session.id, session.updated_at, now),
  }));
  return {
    entries,
    currentSessionId: input.openSessionId,
    permissionMode: input.permissionMode,
    hasProject: input.hasProject,
    running: input.running,
  };
}
