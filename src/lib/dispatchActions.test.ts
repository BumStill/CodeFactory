// SPDX-License-Identifier: Apache-2.0
//
// CF-HDE-R0 / R7 / R8 / R9 / R10 —— 本地派单入口第二阶段。
//
// 真机缺口（v1.83.2，2026-10-10）：带 session_id 的 send / stop 落到了"界面
// 当前显示的会话"上（M56：14:30 发给 5bb60a3a 的修复指令落进 be9674c5；
// 14:42 发给 5bb60a3a 的 stop 停掉了 40704077），而且 `send` 永远回报 ok。
// 这一组测试全部打在**真实 store** 上：界面显示的会话是 B，被操作的会话是 A，
// 每个用例都断言 "只有 A 变了、B 一点没变"，并且复现那两条时序。
//
// 这个文件先红后绿（TDD）：`dispatchActions.ts` 就是为了让这些断言成立而写的。

import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("./tauri", () => ({
  invoke: invokeMock,
  onStream: vi.fn(async () => () => {}),
  onSessionUpdated: vi.fn(async () => () => {}),
  sendMessageAnonymous: vi.fn(async () => {}),
}));

// 保存设置会顺手应用主题，碰到原生 app API。
vi.mock("@tauri-apps/api/app", () => ({ setTheme: vi.fn(async () => {}) }));

import { useChatStore, freshRuntime } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import type { Session, Settings } from "../lib/tauri";
import type { UIMessage } from "../stores/chatEvents";
import {
  DISPATCH_DELIVERY_AHEAD_CURRENT_RUN,
  DISPATCH_STEER_PENDING_NOTE,
  dispatchSend,
  dispatchSetModel,
  dispatchSetPermission,
  dispatchStatus,
  dispatchStop,
} from "./dispatchActions";

function mkSession(id: string, extra: Partial<Session> = {}): Session {
  return {
    id,
    title: `会话 ${id}`,
    cwd: `/p/${id}`,
    model_id: "m",
    endpoint_id: "deepseek",
    created_at: 0,
    updated_at: 0,
    total_input_tokens: 0,
    total_output_tokens: 0,
    kind: "project",
    permission_mode: "standard",
    is_running: false,
    ...extra,
  } as Session;
}

/** 会话 A 是要被操作的会话，B 是界面里当前显示的那个（M56 的现场）。 */
const sessionA = mkSession("A");
const sessionB = mkSession("B");

function userMessage(id: string, content: string): UIMessage {
  return { id, role: "user", content, createdAt: 1 };
}

function assistantMessage(
  id: string,
  rootTurnId: string,
  content: string,
  extra: Partial<UIMessage> = {},
): UIMessage {
  return { id, role: "assistant", content, rootTurnId, createdAt: 2, ...extra };
}

function seed(input: {
  a?: Partial<ReturnType<typeof freshRuntime>>;
  b?: Partial<ReturnType<typeof freshRuntime>>;
  sessions?: Session[];
  activeSession?: Session | null;
  activeModel?: string;
}) {
  useChatStore.setState({
    sessions: input.sessions ?? [sessionA, sessionB],
    activeSession: input.activeSession === undefined ? sessionB : input.activeSession,
    activeModel: input.activeModel ?? "m",
    models: [],
    runtime: {
      A: { ...freshRuntime(), ...(input.a ?? {}) },
      B: { ...freshRuntime(), ...(input.b ?? {}) },
    },
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "save_settings") return (args as { newSettings: Settings }).newSettings;
    if (cmd === "send_message" || cmd === "cancel_chat") return undefined;
    if (cmd === "update_session_model") {
      return { ...sessionA, model_id: (args as { modelId: string }).modelId };
    }
    if (cmd === "update_session_permission_mode") {
      const { sessionId, mode } = args as { sessionId: string; mode: string };
      return { ...(sessionId === "B" ? sessionB : sessionA), permission_mode: mode };
    }
    return undefined;
  });
  useSettingsStore.setState({
    settings: {
      endpoints: { deepseek: { base_url: "https://api.deepseek.com", api_style: "openai", active_model: "v4-pro" } },
      default_endpoint: "deepseek",
      default_model: "v4-pro",
      permissions: { allow: [], ask: [], deny: [], full_access: false },
      shell: { shell: "bash" },
      auto_create_pr: false,
      theme: "dark",
      font_family: "inter",
      mono_font_family: "jetbrains-mono",
      font_size: 14,
    } as unknown as Settings,
  });
});

describe("CF-HDE-R0 · 带 session_id 的操作只作用于该会话", () => {
  it("界面显示 B 时对 A 执行 send：只有 A 收到，B 毫无变化", async () => {
    seed({});
    const receipt = await dispatchSend({ sessionId: "A", message: "修复指令", mode: "steer" });

    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ sessionId: "A", content: "修复指令" }),
    );
    const state = useChatStore.getState();
    expect(state.activeSession?.id).toBe("B");
    expect(
      state.runtime.A.messages.some((m) => m.role === "user" && m.content === "修复指令"),
    ).toBe(true);
    expect(state.runtime.B.messages).toHaveLength(0);
    expect(state.runtime.B.streaming).toBe(false);
    expect(receipt.delivery).toEqual({ status: "delivered", into: "new_turn" });
    expect(receipt.session_id).toBe("A");
  });

  it("复现 14:30：A 空闲但界面停在 B，send 不会落进 B", async () => {
    // 界面停在 B，B 上还带着一轮已经结算的残留（M56 现场）。
    seed({
      b: {
        streaming: true,
        messages: [
          userMessage("b-root", "别的任务"),
          assistantMessage("b-assistant", "b-root", "已完成", {
            turnActivity: { objectiveStatus: "failed", terminalReason: "chat_identity_unreconcilable" } as never,
            turnSettledAt: 1,
          }),
        ],
      },
    });
    await dispatchSend({ sessionId: "A", message: "带交付授权的修复指令", mode: "steer" });
    const state = useChatStore.getState();
    expect(state.runtime.B.messages.map((m) => m.content)).toEqual(["别的任务", "已完成"]);
    expect(
      state.runtime.A.messages.some((m) => m.content === "带交付授权的修复指令"),
    ).toBe(true);
  });

  it("不存在的会话：返回 not_found，绝不退回当前会话", async () => {
    seed({});
    await expect(
      dispatchSend({ sessionId: "ghost", message: "hi", mode: "steer" }),
    ).rejects.toMatchObject({ code: "not_found" });
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());
    expect(useChatStore.getState().runtime.B.messages).toHaveLength(0);
  });

  it("界面显示 B 时对 A 执行 stop：只停 A", async () => {
    seed({
      a: { streaming: true, messages: [userMessage("a-root", "A 的任务")] },
      b: { streaming: false, messages: [] },
    });
    const result = await dispatchStop({ sessionId: "A" });
    expect(invokeMock).toHaveBeenCalledWith("cancel_chat", { sessionId: "A" });
    expect(result).toEqual({ stopped: true, session_id: "A" });
    const state = useChatStore.getState();
    expect(state.activeSession?.id).toBe("B");
    expect(state.runtime.B.messages).toHaveLength(0);
  });

  it("复现 14:42：对不存在的会话执行 stop 不会停掉界面上正在跑的会话", async () => {
    seed({ a: { streaming: true }, b: { streaming: true, messages: [userMessage("b-root", "本任务")] } });
    await expect(dispatchStop({ sessionId: "ghost" })).rejects.toMatchObject({
      code: "not_found",
    });
    expect(invokeMock).not.toHaveBeenCalledWith("cancel_chat", expect.anything());
    expect(useChatStore.getState().runtime.B.messages).toHaveLength(1);
  });

  it("界面显示 B 时对 A 执行 set_permission：只改 A 的模式", async () => {
    seed({});
    await dispatchSetPermission({ sessionId: "A", mode: "trusted" });
    expect(invokeMock).toHaveBeenCalledWith("update_session_permission_mode", {
      sessionId: "A",
      mode: "trusted",
    });
    const state = useChatStore.getState();
    expect(state.activeSession?.id).toBe("B");
    expect(state.sessions.find((s) => s.id === "A")?.permission_mode).toBe("trusted");
    expect(state.sessions.find((s) => s.id === "B")?.permission_mode).toBe("standard");
  });

  it("不存在的会话：set_permission 返回 not_found", async () => {
    seed({});
    await expect(dispatchSetPermission({ sessionId: "ghost", mode: "trusted" })).rejects.toMatchObject({
      code: "not_found",
    });
    expect(invokeMock).not.toHaveBeenCalledWith("update_session_permission_mode", expect.anything());
  });

  it("待发队列按会话隔离：A 排队的那条不会被冲进 B", async () => {
    seed({
      a: {
        streaming: true,
        messages: [
          userMessage("a-root", "A 在跑"),
          assistantMessage("a-assistant", "a-root", "", {
            turnActivity: { objectiveStatus: "active", rootTurnId: "a-root" } as never,
          }),
        ],
      },
    });
    const receipt = await dispatchSend({ sessionId: "A", message: "本轮结束后再发", mode: "queue" });
    expect(receipt.delivery).toEqual({
      status: "queued",
      queue_position: 1,
      ahead: DISPATCH_DELIVERY_AHEAD_CURRENT_RUN,
    });
    const state = useChatStore.getState();
    expect(state.runtime.A.queue.map((q) => q.content)).toEqual(["本轮结束后再发"]);
    expect(state.runtime.B.queue).toHaveLength(0);
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());
  });
});

describe("CF-HDE-R7 · 送达如实回报", () => {
  it("空闲会话：立即开始新的一轮", async () => {
    seed({});
    const receipt = await dispatchSend({ sessionId: "A", message: "开始吧", mode: "steer" });
    expect(receipt.delivery).toEqual({ status: "delivered", into: "new_turn" });
    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ sessionId: "A" }),
    );
    expect(useChatStore.getState().runtime.A.streaming).toBe(true);
  });

  it("残留的旧回合状态不吞消息（M56 根因）", async () => {
    seed({
      a: {
        // 界面残留：streaming 还是 true，但那一轮已经结算（objective failed）。
        streaming: true,
        messages: [
          userMessage("a-root", "旧任务"),
          assistantMessage("a-old", "a-root", "结束了", {
            turnActivity: { objectiveStatus: "failed", terminalReason: "exhausted" } as never,
            turnSettledAt: 3,
          }),
        ],
      },
    });
    const receipt = await dispatchSend({ sessionId: "A", message: "新指令", mode: "steer" });
    expect(receipt.delivery).toEqual({ status: "delivered", into: "new_turn" });
    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ sessionId: "A", content: "新指令" }),
    );
  });

  it("后端一直没有受理：回报 delivery_failed，绝不说 ok", async () => {
    seed({});
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "send_message") return new Promise(() => {});
      return undefined;
    });
    await expect(
      dispatchSend({ sessionId: "A", message: "卡住", mode: "steer" }, { timeoutMs: 20 }),
    ).rejects.toMatchObject({ code: "delivery_failed" });
  });

  it("发送本身失败：回报 delivery_failed", async () => {
    seed({});
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "send_message") throw new Error("provider exploded");
      return undefined;
    });
    await expect(
      dispatchSend({ sessionId: "A", message: "会失败", mode: "steer" }),
    ).rejects.toMatchObject({ code: "delivery_failed" });
  });
});

describe("CF-HDE-R8 · 插话与排队可选", () => {
  const runningA = {
    streaming: true,
    messages: [
      userMessage("a-root", "A 正在跑"),
      assistantMessage("a-assistant", "a-root", "正在做…", {
        turnActivity: { objectiveStatus: "active", rootTurnId: "a-root" } as never,
      }),
    ],
  };

  it("运行中 + steer：这一轮真的读到了才算 delivered", async () => {
    seed({ a: runningA });
    // 模拟这一轮在下一个回合边界读走了这条插话（`steer_applied` 清掉待处理标记）。
    setTimeout(() => {
      const st = useChatStore.getState();
      const runtime = st.runtime.A;
      useChatStore.setState({
        runtime: {
          ...st.runtime,
          A: {
            ...runtime,
            messages: runtime.messages.map((m) => ({ ...m, steerPending: undefined })),
          },
        },
      });
    }, 5);
    const receipt = await dispatchSend(
      { sessionId: "A", message: "先改这个", mode: "steer" },
      { steerAppliedTimeoutMs: 200 },
    );
    expect(receipt.delivery).toEqual({ status: "delivered", into: "current_run" });
    expect(invokeMock).toHaveBeenCalledWith("queue_interjection", {
      sessionId: "A",
      message: "先改这个",
      clientMessageId: expect.any(String),
    });
    const state = useChatStore.getState();
    expect(state.runtime.A.messages.some((m) => m.content === "先改这个")).toBe(true);
    expect(state.runtime.B.messages).toHaveLength(0);
    expect(state.activeSession?.id).toBe("B");
  });

  // CF-STOP-R4（M63，v1.84.0 真机）：这一轮卡在交付工具里等锁时，旧实现回报
  // `delivered / current_run`，但模型要等下一次调用才读得到插话，而那一刻不会来。
  // 现状是如实回报 `queued`：插话排在当前这一轮，当前工具调用返回后读取。
  it("M63 时序：卡在工具里读不到时回报 queued，而不是 delivered", async () => {
    seed({ a: runningA });
    const receipt = await dispatchSend(
      { sessionId: "A", message: "改用 chrome channel", mode: "steer" },
      { steerAppliedTimeoutMs: 30 },
    );
    expect(receipt.delivery).toEqual({
      status: "queued",
      queue_position: 1,
      ahead: DISPATCH_DELIVERY_AHEAD_CURRENT_RUN,
      note: DISPATCH_STEER_PENDING_NOTE,
    });
    expect(receipt.delivery.status).not.toBe("delivered");
    expect(invokeMock).toHaveBeenCalledWith("queue_interjection", {
      sessionId: "A",
      message: "改用 chrome channel",
      clientMessageId: expect.any(String),
    });
  });

  // 同一条消息不能因为停止后又续跑而落两遍：还没被读到之前不再送第二次。
  it("同一条插话在还没被读到时只落一次", async () => {
    seed({ a: runningA });
    const first = await dispatchSend(
      { sessionId: "A", message: "别提交", mode: "steer" },
      { steerAppliedTimeoutMs: 30 },
    );
    const second = await dispatchSend(
      { sessionId: "A", message: "别提交", mode: "steer" },
      { steerAppliedTimeoutMs: 30 },
    );

    expect(first.delivery.status).toBe("queued");
    expect(second.delivery.status).toBe("queued");
    const interjectionCalls = invokeMock.mock.calls.filter(
      ([cmd]) => cmd === "queue_interjection",
    );
    expect(interjectionCalls).toHaveLength(1);
    expect(
      useChatStore.getState().runtime.A.messages.filter((m) => m.content === "别提交"),
    ).toHaveLength(1);
  });

  it("运行中 + queue：本轮结束后再发", async () => {
    seed({ a: runningA });
    const receipt = await dispatchSend({ sessionId: "A", message: "等这轮", mode: "queue" });
    expect(receipt.delivery.status).toBe("queued");
    expect(invokeMock).not.toHaveBeenCalledWith("queue_interjection", expect.anything());
    expect(useChatStore.getState().runtime.A.queue.map((q) => q.content)).toEqual(["等这轮"]);
  });
});

describe("CF-HDE-R9 · 状态可信且够用", () => {
  it("模型在调用就不能显示等待重试，并给出最近回复 / PR 号 / 上下文大小", async () => {
    seed({
      a: {
        streaming: true,
        contextUsage: { used: 12_345, limit: 64_000, maxLimit: 64_000 },
        messages: [
          userMessage("a-root", "做这个"),
          assistantMessage("a-assistant", "a-root", "最后一条回复的摘要", {
            turnActivity: { objectiveStatus: "waiting_system", rootTurnId: "a-root" } as never,
            toolCalls: [
              {
                id: "tc1",
                name: "deliver_changes",
                args: {},
                result: "分支: fix/dispatch\nPR #604 已创建",
              },
            ] as never,
          }),
        ],
      },
      sessions: [mkSession("A", { is_running: true }), sessionB],
    });
    const status = await dispatchStatus({ sessionId: "A" });
    expect(status.objective_state).toBe("active");
    expect(status.latest_reply).toBe("最后一条回复的摘要");
    expect(status.pr_number).toBe(604);
    expect(status.context_tokens).toBe(12_345);
  });

  it("已结束的会话如实汇报终态", async () => {
    seed({
      a: {
        streaming: false,
        messages: [
          userMessage("a-root", "做这个"),
          assistantMessage("a-assistant", "a-root", "x".repeat(400), {
            turnActivity: { objectiveStatus: "completed", terminalReason: "completed", rootTurnId: "a-root" } as never,
            turnSettledAt: 9,
          }),
        ],
      },
    });
    const status = await dispatchStatus({ sessionId: "A" });
    expect(status.objective_state).toBe("completed");
    expect((status.latest_reply ?? "").length).toBeLessThanOrEqual(281);
    expect(status.pr_number).toBeNull();
  });

  it("不存在的会话：status 返回 not_found", async () => {
    seed({});
    await expect(dispatchStatus({ sessionId: "ghost" })).rejects.toMatchObject({
      code: "not_found",
    });
  });
});

describe("CF-HDE-R10 · 默认模型可设", () => {
  it("省略 session_id 时设置新会话默认模型，端点与模型保持配套", async () => {
    seed({});
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_models") {
        return [{ id: "v4-pro", name: "v4-pro", context_length: 64_000 }];
      }
      if (cmd === "save_settings") return (args as { newSettings: Settings }).newSettings;
      return undefined;
    });
    const result = await dispatchSetModel({ model: "v4-pro" });
    expect(result).toMatchObject({ scope: "default", model: "v4-pro", endpoint: "deepseek" });
    expect(invokeMock).toHaveBeenCalledWith("set_endpoint_active_model", {
      endpointName: "deepseek",
      modelId: "v4-pro",
    });
    const saved = invokeMock.mock.calls.find(([cmd]) => cmd === "save_settings")?.[1] as {
      newSettings: Settings;
    };
    expect(saved.newSettings.default_model).toBe("v4-pro");
    expect(saved.newSettings.default_endpoint).toBe("deepseek");
    expect(useChatStore.getState().activeModel).toBe("v4-pro");
  });

  it("模型不属于当前端点：fail-closed 报错，不做静默错配", async () => {
    seed({});
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_models") return [{ id: "v4-flash", name: "v4-flash", context_length: 64_000 }];
      return undefined;
    });
    await expect(dispatchSetModel({ model: "gpt-5" })).rejects.toMatchObject({
      code: "invalid_request",
    });
    expect(invokeMock).not.toHaveBeenCalledWith("save_settings", expect.anything());
    expect(useChatStore.getState().activeModel).toBe("m");
  });

  it("带 session_id 时改的是那个会话的模型，且不切换界面", async () => {
    seed({});
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "update_session_model") {
        return { ...sessionA, model_id: (args as { modelId: string }).modelId };
      }
      return undefined;
    });
    const result = await dispatchSetModel({ sessionId: "A", model: "v4-pro" });
    expect(invokeMock).toHaveBeenCalledWith("update_session_model", {
      sessionId: "A",
      modelId: "v4-pro",
    });
    expect(result).toMatchObject({ scope: "session", session_id: "A" });
    expect(useChatStore.getState().activeSession?.id).toBe("B");
    expect(useChatStore.getState().activeModel).toBe("m");
  });

  it("不存在的会话：set_model 返回 not_found", async () => {
    seed({});
    await expect(
      dispatchSetModel({ sessionId: "ghost", model: "v4-pro" }),
    ).rejects.toMatchObject({ code: "not_found" });
    expect(invokeMock).not.toHaveBeenCalledWith("update_session_model", expect.anything());
  });
});
