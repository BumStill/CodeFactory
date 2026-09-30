// SPDX-License-Identifier: Apache-2.0
//
// 菜单栏接线的契约测试。
//
// 这一层测的是"点了菜单 → 调用了界面上那个动作"以及"菜单项 id 与原生侧逐字
// 一致"。前者防止菜单变成第二套逻辑(改一处忘一处),后者防止改名之后菜单静默
// 失效 —— 那种失败不会报错,只会让用户觉得点了没反应。
//
// 真正"从菜单栏点下去"的原生行为只能在真机上验证;这里覆盖的是它下游的全部
// 决策逻辑:校验、翻译、派发、状态同步。

import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  MENU_ACTION,
  MENU_ACTION_IDS,
  MENU_PERMISSION_MODES,
  MENU_SESSION_LIMIT,
  SESSION_MENU_EVENT,
  buildMenuSyncState,
  dispatchMenuEvent,
  resolveMenuIntent,
  type DesktopMenuHandlers,
} from "./desktopMenu";

const repoRoot = resolve(__dirname, "../..");

function readRust(relative: string): string {
  return readFileSync(resolve(repoRoot, relative), "utf8");
}

function mkHandlers(over: Partial<DesktopMenuHandlers> = {}): DesktopMenuHandlers {
  return {
    newSession: vi.fn(),
    switchSession: vi.fn(),
    setPermissionMode: vi.fn(),
    focusInput: vi.fn(),
    sendComposer: vi.fn(),
    sendText: vi.fn(),
    stopRun: vi.fn(),
    ...over,
  };
}

const ctx = { hasProject: true, running: false };

describe("菜单栏动作契约", () => {
  it("前端动作 id 与原生菜单 spec 一一对应", () => {
    const spec = readRust("src-tauri/src/menu_spec.rs");
    for (const id of MENU_ACTION_IDS) {
      // 原生侧把每个动作 id 定义成 const(逐字相同的字符串常量)。
      expect(spec, `menu_spec.rs 缺少动作 ${id}`).toContain(`"${id}"`);
    }
    // 反向:原生 spec 里的动作 id 必须都被前端认识,否则会出现"菜单里能点、
    // 前端不认识"的死项。
    const declared = [...spec.matchAll(/pub const ACTION_[A-Z_]+: &str = "([^"]+)"/g)].map(
      (match) => match[1],
    );
    expect(declared.length).toBeGreaterThan(0);
    for (const id of declared) {
      expect(MENU_ACTION_IDS, `前端不认识原生动作 ${id}`).toContain(id);
    }
  });

  it("事件名、会话条数上限与原生侧一致", () => {
    const menuRs = readRust("src-tauri/src/menu.rs");
    const specRs = readRust("src-tauri/src/menu_spec.rs");
    expect(menuRs).toContain(`"${SESSION_MENU_EVENT}"`);
    expect(specRs).toContain(`RECENT_SESSION_LIMIT: usize = ${MENU_SESSION_LIMIT}`);
    expect(specRs).toContain(`pub const SESSION_MENU_TITLE: &str = "会话"`);
  });

  it("权限模式子菜单的三个取值与后端校验一致", () => {
    const specRs = readRust("src-tauri/src/menu_spec.rs");
    expect(MENU_PERMISSION_MODES).toEqual(["safe", "standard", "trusted"]);
    for (const mode of MENU_PERMISSION_MODES) {
      expect(specRs).toContain(`("${mode}"`);
    }
  });

  it("每个菜单项都有对应的快捷键,或明确约定不靠快捷键", () => {
    const specRs = readRust("src-tauri/src/menu_spec.rs");
    // 任务书承诺的四个快捷键 + 两个"不靠快捷键"的项。
    expect(specRs).toContain('Some("CmdOrCtrl+N")');
    expect(specRs).toContain('Some("CmdOrCtrl+L")');
    expect(specRs).toContain('Some("CmdOrCtrl+Return")');
    expect(specRs).toContain("accelerator: None");
  });
});

describe("菜单事件的校验与翻译", () => {
  it("新建会话:没有当前项目时不动作", () => {
    expect(resolveMenuIntent({ action: MENU_ACTION.NEW_SESSION }, { ...ctx, hasProject: false })).toBeNull();
    expect(resolveMenuIntent({ action: MENU_ACTION.NEW_SESSION }, ctx)).toEqual({ kind: "new-session" });
  });

  it("切换会话:必须带真实会话 id", () => {
    expect(resolveMenuIntent({ action: MENU_ACTION.SWITCH_SESSION }, ctx)).toBeNull();
    expect(resolveMenuIntent({ action: MENU_ACTION.SWITCH_SESSION, sessionId: "   " }, ctx)).toBeNull();
    expect(
      resolveMenuIntent({ action: MENU_ACTION.SWITCH_SESSION, sessionId: "9537257c-aaaa" }, ctx),
    ).toEqual({ kind: "switch-session", sessionId: "9537257c-aaaa" });
  });

  it("权限模式:只接受后端认可的三种取值", () => {
    for (const mode of MENU_PERMISSION_MODES) {
      expect(resolveMenuIntent({ action: MENU_ACTION.SET_PERMISSION_MODE, mode }, ctx)).toEqual({
        kind: "set-permission-mode",
        mode,
      });
    }
    expect(resolveMenuIntent({ action: MENU_ACTION.SET_PERMISSION_MODE, mode: "godmode" }, ctx)).toBeNull();
    expect(resolveMenuIntent({ action: MENU_ACTION.SET_PERMISSION_MODE }, ctx)).toBeNull();
  });

  it("剪贴板发送:空文本不发,有文本按原文发送", () => {
    expect(resolveMenuIntent({ action: MENU_ACTION.SEND_CLIPBOARD, text: "   \n " }, ctx)).toBeNull();
    expect(resolveMenuIntent({ action: MENU_ACTION.SEND_CLIPBOARD }, ctx)).toBeNull();
    expect(
      resolveMenuIntent({ action: MENU_ACTION.SEND_CLIPBOARD, text: "帮我跑一下测试\n" }, ctx),
    ).toEqual({ kind: "send-text", text: "帮我跑一下测试\n" });
  });

  it("停止当前执行:没有进行中的执行时不动作", () => {
    expect(resolveMenuIntent({ action: MENU_ACTION.STOP_RUN }, ctx)).toBeNull();
    expect(resolveMenuIntent({ action: MENU_ACTION.STOP_RUN }, { ...ctx, running: true })).toEqual({
      kind: "stop-run",
    });
  });

  it("聚焦输入框与发送输入框内容始终可用", () => {
    expect(resolveMenuIntent({ action: MENU_ACTION.FOCUS_INPUT }, ctx)).toEqual({ kind: "focus-input" });
    expect(resolveMenuIntent({ action: MENU_ACTION.SEND_INPUT }, ctx)).toEqual({ kind: "send-composer" });
  });

  it("未知动作被忽略,不做任何猜测", () => {
    expect(resolveMenuIntent({ action: "system.something" }, ctx)).toBeNull();
    expect(resolveMenuIntent({ action: "" }, ctx)).toBeNull();
  });
});

describe("菜单事件派发到界面已有的动作", () => {
  it("七种菜单事件各触发一个且只触发一个动作", async () => {
    const handlers = mkHandlers();
    const cases: { event: Parameters<typeof dispatchMenuEvent>[0]; running?: boolean }[] = [
      { event: { action: MENU_ACTION.NEW_SESSION } },
      { event: { action: MENU_ACTION.SWITCH_SESSION, sessionId: "s-1" } },
      { event: { action: MENU_ACTION.SET_PERMISSION_MODE, mode: "trusted" } },
      { event: { action: MENU_ACTION.FOCUS_INPUT } },
      { event: { action: MENU_ACTION.SEND_INPUT } },
      { event: { action: MENU_ACTION.SEND_CLIPBOARD, text: "来自剪贴板" } },
      { event: { action: MENU_ACTION.STOP_RUN }, running: true },
    ];
    for (const { event, running } of cases) {
      await dispatchMenuEvent(event, { hasProject: true, running: Boolean(running) }, handlers);
    }

    expect(handlers.newSession).toHaveBeenCalledTimes(1);
    expect(handlers.switchSession).toHaveBeenCalledWith("s-1");
    expect(handlers.setPermissionMode).toHaveBeenCalledWith("trusted");
    expect(handlers.focusInput).toHaveBeenCalledTimes(1);
    expect(handlers.sendComposer).toHaveBeenCalledTimes(1);
    expect(handlers.sendText).toHaveBeenCalledWith("来自剪贴板");
    expect(handlers.stopRun).toHaveBeenCalledTimes(1);
  });

  it("不可用的菜单项不触发任何动作", async () => {
    const handlers = mkHandlers();
    await dispatchMenuEvent({ action: MENU_ACTION.NEW_SESSION }, { hasProject: false, running: false }, handlers);
    await dispatchMenuEvent({ action: MENU_ACTION.STOP_RUN }, { hasProject: true, running: false }, handlers);
    await dispatchMenuEvent({ action: MENU_ACTION.SEND_CLIPBOARD, text: "" }, ctx, handlers);
    for (const fn of Object.values(handlers)) expect(fn).not.toHaveBeenCalled();
  });
});

describe("菜单状态与界面一致", () => {
  const sessions = [
    { id: "9537257c-1111-2222", title: "新会话", updated_at: 300_000, cwd: "/p" },
    { id: "11112222-3333-4444", title: "新会话", updated_at: 120_000, cwd: "/p" },
  ] as never[];

  it("切换列表显示标题 + 短 id + 相对时间,勾选当前会话", () => {
    const state = buildMenuSyncState({
      sessions,
      openSessionId: "11112222-3333-4444",
      permissionMode: "standard",
      hasProject: true,
      running: false,
      now: 600_000,
    });
    expect(state.entries).toHaveLength(2);
    expect(state.entries[0].label).toBe("新会话(9537257c,5 分钟前)");
    expect(state.entries[1].label).toBe("新会话(11112222,8 分钟前)");
    // 同名会话靠短 id 区分 —— 否则菜单里两行一模一样。
    expect(state.entries[0].label).not.toBe(state.entries[1].label);
    expect(state.currentSessionId).toBe("11112222-3333-4444");
    expect(state.permissionMode).toBe("standard");
    expect(state.hasProject).toBe(true);
    expect(state.running).toBe(false);
  });

  it("切换列表最多 20 项(与原生槽位数一致)", () => {
    const many = Array.from({ length: MENU_SESSION_LIMIT + 8 }, (_, index) => ({
      id: `id-${index}`,
      title: `会话 ${index}`,
      updated_at: 1_000,
      cwd: "/p",
    })) as never[];
    const state = buildMenuSyncState({
      sessions: many,
      openSessionId: null,
      permissionMode: "safe",
      hasProject: false,
      running: true,
      now: 2_000,
    });
    expect(state.entries).toHaveLength(MENU_SESSION_LIMIT);
    expect(state.permissionMode).toBe("safe");
    expect(state.running).toBe(true);
    expect(state.hasProject).toBe(false);
  });
});
