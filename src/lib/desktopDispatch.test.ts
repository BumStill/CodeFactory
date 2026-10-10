// SPDX-License-Identifier: Apache-2.0
//
// 本地派单入口的前端验收：每条 CF-HDE-R6 能力都必须落到一个界面动作上，
// 而 CF-HDE-R2/R3 的反例必须在**到达**那个动作之前就被拒绝。

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";

import {
  applyDispatchRequest,
  dispatchReplyArgs,
  DISPATCH_PERMISSION_MODES,
  DISPATCH_REQUEST_EVENT,
  type DispatchHandlers,
  type DispatchRequestEvent,
} from "./desktopDispatch";

function request(operation: string, body: Record<string, unknown> = {}): DispatchRequestEvent {
  return { request_id: "req-1", operation, request: body };
}

function stubHandlers(): DispatchHandlers & { calls: string[] } {
  const calls: string[] = [];
  const record = (name: string) => async (...args: unknown[]) => {
    calls.push(name);
    return { name, args };
  };
  return {
    calls,
    createAndSend: record("createAndSend") as DispatchHandlers["createAndSend"],
    send: record("send") as DispatchHandlers["send"],
    setPermission: record("setPermission") as DispatchHandlers["setPermission"],
    status: record("status") as DispatchHandlers["status"],
    setModel: record("setModel") as DispatchHandlers["setModel"],
    switchSession: record("switchSession") as DispatchHandlers["switchSession"],
    stop: record("stop") as DispatchHandlers["stop"],
    listApprovals: async () => {
      calls.push("listApprovals");
      return [{ approvalId: "a-1", toolName: "bash", sessionId: "s-1" }];
    },
    resolveApproval: record("resolveApproval") as DispatchHandlers["resolveApproval"],
  };
}

describe("本地派单入口的前端路由", () => {
  it("每条 CF-HDE-R6 能力都能在后台被调到（不需要菜单、坐标点击或解锁）", async () => {
    const handlers = stubHandlers();
    const cases: [DispatchRequestEvent, string][] = [
      [request("set_model", { session_id: "s-1", model: "deepseek/v4" }), "setModel"],
      [request("set_model", { model: "deepseek/v4" }), "setModel"],
      [request("switch_session", { session_id: "s-2" }), "switchSession"],
      [request("stop", {}), "stop"],
      [request("list_approvals"), "listApprovals"],
      [request("resolve_approval", { approval_id: "a-1", approve: true }), "resolveApproval"],
    ];
    for (const [event, expected] of cases) {
      const outcome = await applyDispatchRequest(event, handlers);
      expect(outcome.ok, `${event.operation} 应该成功`).toBe(true);
      expect(handlers.calls[handlers.calls.length - 1]).toBe(expected);
    }
  });

  it("创建并发送、发送、设置权限、查询状态都走各自的处理器", async () => {
    const handlers = stubHandlers();
    expect(
      (
        await applyDispatchRequest(
          request("create_and_send", {
            project: "/tmp/p",
            message: "fix #590",
            model: "deepseek/v4",
            permission_mode: "trusted",
            delivery_authorized: true,
          }),
          handlers,
        )
      ).ok,
    ).toBe(true);
    expect(handlers.calls[handlers.calls.length - 1]).toBe("createAndSend");

    expect(
      (await applyDispatchRequest(
        request("send", { session_id: "s-1", message: "go", delivery_authorized: false }),
        handlers,
      )).ok,
    ).toBe(true);
    expect(handlers.calls[handlers.calls.length - 1]).toBe("send");

    expect(
      (
        await applyDispatchRequest(
          request("set_permission", { session_id: "s-1", permission_mode: "trusted" }),
          handlers,
        )
      ).ok,
    ).toBe(true);
    expect(handlers.calls[handlers.calls.length - 1]).toBe("setPermission");

    expect(
      (await applyDispatchRequest(request("status", { session_id: "s-1" }), handlers)).ok,
    ).toBe(true);
    expect(handlers.calls[handlers.calls.length - 1]).toBe("status");
  });

  it("交付授权缺失时请求被拒绝，且不会碰到任何界面动作（M48）", async () => {
    const handlers = stubHandlers();
    const outcome = await applyDispatchRequest(
      request("send", { session_id: "s-1", message: "open a PR and merge it" }),
      handlers,
    );
    expect(outcome.ok).toBe(false);
    if (!outcome.ok) expect(outcome.error.code).toBe("invalid_request");
    expect(handlers.calls).toHaveLength(0);
  });

  it("审批不带隐式默认值：approve 必须是显式布尔", async () => {
    const handlers = stubHandlers();
    const outcome = await applyDispatchRequest(
      request("resolve_approval", { approval_id: "a-1" }),
      handlers,
    );
    expect(outcome.ok).toBe(false);
    expect(handlers.calls).toHaveLength(0);
  });

  it("拒绝的审批原样转达为拒绝，不会变成放行", async () => {
    const handlers = stubHandlers();
    const resolve = vi.fn(async () => ({}));
    const outcome = await applyDispatchRequest(
      request("resolve_approval", { approval_id: "a-1", approve: false }),
      { ...handlers, resolveApproval: resolve },
    );
    expect(outcome.ok).toBe(true);
    expect(resolve).toHaveBeenCalledWith({ approvalId: "a-1", approve: false });
  });

  it("未知动作被拒绝而不是猜一个最接近的动作", async () => {
    const handlers = stubHandlers();
    for (const event of [
      request("exec", { cmd: "id" }),
      request("focus_main_display"),
      request("tcp://127.0.0.1:1234"),
    ]) {
      const outcome = await applyDispatchRequest(event, handlers);
      expect(outcome.ok, `${event.operation} 不能被接受`).toBe(false);
    }
    expect(handlers.calls).toHaveLength(0);
  });

  it("非法权限模式 / 空字段在到达界面动作之前被拒绝", async () => {
    const handlers = stubHandlers();
    const bad = [
      request("set_permission", { session_id: "s-1", permission_mode: "godmode" }),
      request("set_permission", { session_id: "", permission_mode: "trusted" }),
      request("send", { session_id: "s-1", message: "  ", delivery_authorized: false }),
      request("set_model", { model: "" }),
    ];
    for (const event of bad) {
      const outcome = await applyDispatchRequest(event, handlers);
      expect(outcome.ok, JSON.stringify(event)).toBe(false);
    }
    expect(handlers.calls).toHaveLength(0);
  });

  it("处理器抛错时返回结构化失败，绝不谎报成功", async () => {
    const handlers = stubHandlers();
    const outcome = await applyDispatchRequest(request("status", { session_id: "s-1" }), {
      ...handlers,
      status: async () => {
        throw new Error("session not found");
      },
    });
    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.error.code).toBe("internal");
      expect(outcome.error.message).toContain("session not found");
    }
  });

  it("回给原生侧的应答形状与 dispatch_reply 的参数一致", () => {
    expect(dispatchReplyArgs("req-1", { ok: true, result: { pr_number: 590 } })).toEqual({
      requestId: "req-1",
      ok: true,
      result: { pr_number: 590 },
    });
    expect(
      dispatchReplyArgs("req-2", {
        ok: false,
        error: { code: "invalid_request", message: "nope" },
      }),
    ).toEqual({
      requestId: "req-2",
      ok: false,
      error: { code: "invalid_request", message: "nope" },
    });
  });
});

describe("与原生侧逐字对齐（跨语言交叉断言）", () => {
  const rust = readFileSync(
    resolve(process.cwd(), "src-tauri/src/headless_dispatch.rs"),
    "utf8",
  );

  it("事件名和权限模式两边一致", () => {
    expect(rust).toContain(`"${DISPATCH_REQUEST_EVENT}"`);
    for (const mode of DISPATCH_PERMISSION_MODES) {
      expect(rust).toContain(`"${mode}"`);
    }
  });

  it("每个操作名都在原生协议里存在", () => {
    for (const operation of [
      "create_and_send",
      "send",
      "set_permission",
      "status",
      "set_model",
      "switch_session",
      "stop",
      "list_approvals",
      "resolve_approval",
      "focus_main_display",
    ]) {
      expect(rust, `${operation} 在原生侧不存在`).toContain(`"${operation}"`);
    }
  });

  it("交付授权在原生侧同样是必需字段（没有 serde 默认值）", () => {
    expect(rust).toContain("delivery_authorized: bool");
    expect(rust).not.toContain("delivery_authorized: bool =");
    expect(rust).toContain("deny_unknown_fields");
  });
});
