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

  it("turns route exhaustion into actionable Chinese guidance while retaining evidence", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "error",
        message:
          "所有可用模型端点均不可用：ChatGPT / gpt-5.5（HTTP 503 Service Unavailable）；DeepSeek / deepseek-v4-pro（HTTP 429 Too Many Requests）。请检查服务状态或额度，或在模型选择器选择其他端点后重试。",
      },
      "assistant-1",
    );

    expect(next.streaming).toBe(false);
    expect(next.messages[0].content).toContain("所有已配置且有凭据的模型端点都暂时不可用");
    expect(next.messages[0].content).toContain("模型设置");
    expect(next.messages[0].content).toContain("稍后重试");
    expect(next.messages[0].failureEvidence).toContain("ChatGPT / gpt-5.5");
    expect(next.messages[0].failureEvidence).toContain("DeepSeek / deepseek-v4-pro");
  });

  it("normalizes a pre-stream invoke rejection into the same actionable state", () => {
    const presentation = presentChatInvocationError(
      "Error: 所有可用模型端点均不可用：chatgpt：缺少 ChatGPT 登录凭据；deepseek：凭据读取超时。",
    );

    expect(presentation.content).toContain("所有已配置且有凭据的模型端点都暂时不可用");
    expect(presentation.failureEvidence).toContain("deepseek：凭据读取超时");
    expect(presentation.failureEvidence).not.toMatch(/^Error:/);
  });
});
