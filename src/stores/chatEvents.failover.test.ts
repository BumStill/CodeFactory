// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";

import {
  presentChatInvocationError,
  reduceChatStreamEvent,
  type ChatEventState,
} from "./chatEvents";

function baseState(): ChatEventState {
  return {
    messages: [
      {
        id: "assistant-1",
        role: "assistant",
        content: "",
        createdAt: 1,
      },
    ],
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

describe("model route failover stream events", () => {
  it("retains an explicit route change notice on the live assistant turn", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "completion_gate_action",
        kind: "turn_notice",
        detail:
          "ChatGPT / gpt-5.5 暂时不可用，已自动切换到 DeepSeek / deepseek-v4-pro，任务继续执行。",
      },
      "assistant-1",
    );

    expect(next.streaming).toBe(true);
    expect(next.messages[0].gateActions).toEqual([
      {
        kind: "turn_notice",
        detail:
          "ChatGPT / gpt-5.5 暂时不可用，已自动切换到 DeepSeek / deepseek-v4-pro，任务继续执行。",
      },
    ]);
  });

  it("keeps same-route retries as one evidence group instead of separate notices", () => {
    let state = reduceChatStreamEvent(
      baseState(),
      {
        type: "transport_retry",
        label: "ChatGPT Responses stream request",
        attempt: 1,
        max_attempts: 3,
        delay_ms: 300,
        reason: "HTTP 503 Service Unavailable",
      },
      "assistant-1",
    );
    state = reduceChatStreamEvent(
      state,
      {
        type: "transport_retry",
        label: "ChatGPT Responses stream request",
        attempt: 2,
        max_attempts: 3,
        delay_ms: 600,
        reason: "HTTP 503 Service Unavailable",
      },
      "assistant-1",
    );

    expect(state.messages[0].transportRetries).toHaveLength(2);
    const retries = state.messages[0].transportRetries ?? [];
    expect(retries[retries.length - 1]).toEqual(
      expect.objectContaining({ attempt: 2, maxAttempts: 3 }),
    );
  });

  it("keeps route exhaustion system-owned while retaining evidence", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "error",
        message:
          "所有可用模型端点均不可用：ChatGPT / gpt-5.5（HTTP 503 Service Unavailable）；DeepSeek / deepseek-v4-pro（HTTP 429 Too Many Requests）。请检查服务状态或额度，或在模型选择器选择其他端点后重试。",
      },
      "assistant-1",
    );

    expect(next.streaming).toBe(true);
    expect(next.messages[0].content).toContain("所有已配置且有凭据的模型端点都暂时不可用");
    expect(next.messages[0].content).toContain("目标与失败证据已保留");
    expect(next.messages[0].content).toContain("系统将按退避策略重新观测可用路由");
    expect(next.messages[0].content).not.toMatch(/重试|继续执行|回到对话/);
    expect(next.messages[0].failureEvidence).toContain("ChatGPT / gpt-5.5");
    expect(next.messages[0].failureEvidence).toContain("DeepSeek / deepseek-v4-pro");

    const settled = reduceChatStreamEvent(
      next,
      {
        type: "turn_settled",
        run_instance_id: "run-failover-1",
        root_turn_id: "root-failover-1",
        objective_id: "objective-failover-1",
        status: "waiting_system",
      },
      "assistant-1",
    );
    expect(settled.streaming).toBe(false);
    expect(settled.messages[0].content).toBe(next.messages[0].content);
  });

  it("normalizes a pre-stream invoke rejection into the same actionable state", () => {
    const presentation = presentChatInvocationError(
      "Error: 所有可用模型端点均不可用：chatgpt：缺少 ChatGPT 登录凭据；deepseek：凭据读取超时。",
    );

    expect(presentation.content).toContain("所有已配置且有凭据的模型端点都暂时不可用");
    expect(presentation.content).toContain("系统将按退避策略重新观测可用路由");
    expect(presentation.content).not.toMatch(/重试|继续执行|回到对话/);
    expect(presentation.failureEvidence).toContain("deepseek：凭据读取超时");
    expect(presentation.failureEvidence).not.toMatch(/^Error:/);
  });

  it("keeps a fixed endpoint credential failure actionable in the visible message", () => {
    const presentation = presentChatInvocationError(
      "所有可用模型端点均不可用：DeepSeek / deepseek-v4-pro（AUTH_MISSING: deepseek 尚未配置凭据，请在模型设置中配置 API Key）。请检查模型设置后重试。",
    );

    expect(presentation.content).toContain("DeepSeek / deepseek-v4-pro");
    expect(presentation.content).toContain("尚未配置凭据");
    expect(presentation.content).toContain("模型设置");
    expect(presentation.content).toContain("保存后系统会从安全检查点恢复");
    expect(presentation.content).not.toMatch(/重试|继续执行|回到对话/);
  });

  it("keeps the same credential guidance when exhaustion arrives as a stream error", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "error",
        message:
          "所有可用模型端点均不可用：DeepSeek / deepseek-v4-pro（CREDENTIAL_ACCESS_REQUIRED: deepseek 凭据读取失败）。请检查模型设置后重试。",
      },
      "assistant-1",
    );

    expect(next.messages[0].content).toContain("DeepSeek / deepseek-v4-pro");
    expect(next.messages[0].content).toContain("无法读取已配置凭据");
    expect(next.messages[0].content).toContain("需要一次密钥访问授权");
    expect(next.messages[0].content).toContain("授权完成后系统会从安全检查点恢复");
    expect(next.messages[0].content).not.toMatch(/重试|继续执行|回到对话/);
    expect(next.messages[0].failureEvidence).toContain("CREDENTIAL_ACCESS_REQUIRED");
  });
});
