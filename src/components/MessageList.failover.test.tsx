// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
}));

import { MessageList } from "./MessageList";
import type { UIMessage } from "../stores/chatEvents";

function assistant(overrides: Partial<UIMessage> = {}): UIMessage {
  return {
    id: "assistant-1",
    role: "assistant",
    content: "",
    createdAt: Date.now(),
    ...overrides,
  };
}

describe("MessageList model route failover", () => {
  it("shows a route switch as a natural low-contrast 13px line, not a card", () => {
    render(
      <MessageList
        messages={[
          assistant({
            gateActions: [
              {
                kind: "turn_notice",
                detail:
                  "ChatGPT / gpt-5.5 暂时不可用，已自动切换到 DeepSeek / deepseek-v4-pro，任务继续执行。",
              },
            ],
          }),
        ]}
        streaming
        cwd={null}
      />,
    );

    const line = screen.getByText(
      "ChatGPT / gpt-5.5 暂时不可用，已自动切换到 DeepSeek / deepseek-v4-pro，任务继续执行。",
    );
    expect(line).toHaveAttribute("role", "status");
    expect(line).toHaveAttribute("aria-live", "polite");
    expect(line).toHaveClass("text-[13px]", "text-gray-500");
    expect(line.className).not.toMatch(/\bborder\b|\bbg-|rounded/);
  });

  it("keeps a persisted route switch as the same natural line after reload", () => {
    render(
      <MessageList
        messages={[
          assistant({
            completionState: "turn_notice",
            content:
              "ChatGPT / gpt-5.5 暂时不可用，已自动切换到 DeepSeek / deepseek-v4-pro，任务继续执行。",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    const line = screen.getByText(/已自动切换到 DeepSeek/);
    expect(line).toHaveAttribute("role", "status");
    expect(line).toHaveClass("text-[13px]", "text-gray-500");
    expect(line.className).not.toMatch(/\bborder\b|\bbg-|rounded/);
  });

  it("does not blame the model connection while a tool command is still running", () => {
    render(
      <MessageList
        messages={[
          assistant({
            transportRetries: [
              {
                label: "OpenAI-compatible chat stream request",
                attempt: 1,
                maxAttempts: 3,
                delayMs: 300,
                reason: "HTTP 503 Service Unavailable",
              },
            ],
            toolCalls: [
              {
                id: "tool-1",
                name: "bash",
                args: JSON.stringify({ command: "poll ci" }),
                status: "running",
              },
            ],
          }),
        ]}
        streaming
        cwd={null}
      />,
    );

    expect(screen.queryByText("模型连接不稳定，正在重新连接…")).toBeNull();
    expect(screen.getByText("模型连接曾短暂不稳定，已完成重连")).toBeInTheDocument();
    expect(screen.getByText(/HTTP 503 Service Unavailable/)).toBeInTheDocument();
  });

  it("renders repeated same-route retries as one quiet expandable evidence line", () => {
    render(
      <MessageList
        messages={[
          assistant({
            transportRetries: [
              {
                label: "ChatGPT Responses stream request",
                attempt: 1,
                maxAttempts: 3,
                delayMs: 300,
                reason: "HTTP 503 Service Unavailable",
              },
              {
                label: "ChatGPT Responses stream request",
                attempt: 2,
                maxAttempts: 3,
                delayMs: 600,
                reason: "HTTP 503 Service Unavailable",
              },
            ],
          }),
        ]}
        streaming
        cwd={null}
      />,
    );

    expect(screen.getByText("模型连接不稳定，正在重新连接…")).toBeInTheDocument();
    expect(screen.getAllByText(/模型连接不稳定|模型连接重试/)).toHaveLength(1);
    const disclosure = screen.getByText("模型连接不稳定，正在重新连接…").closest("details");
    expect(disclosure).toBeInTheDocument();
    expect(disclosure?.className).not.toMatch(/\bborder\b|\bbg-/);
    expect(disclosure).toHaveClass("text-[13px]");
  });

  it("shows route exhaustion as actionable guidance with expandable technical evidence", () => {
    render(
      <MessageList
        messages={[
          assistant({
            content:
              "所有已配置且有凭据的模型端点都暂时不可用。请检查模型设置中的凭据、余额或端点状态，也可以稍后重试。",
            failureEvidence:
              "chatgpt/gpt-5.5: HTTP 503; deepseek/deepseek-v4-pro: HTTP 429",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(
      screen.getByText(/所有已配置且有凭据的模型端点都暂时不可用/),
    ).toBeInTheDocument();
    expect(screen.getByText(/请检查模型设置中的凭据、余额或端点状态/)).toBeInTheDocument();
    expect(screen.getByText("查看失败详情").closest("details")).toBeInTheDocument();
    expect(screen.getByText(/chatgpt\/gpt-5.5/)).toBeInTheDocument();
  });

  it("keeps actionable route-exhaustion guidance after a failed turn is reloaded", () => {
    render(
      <MessageList
        messages={[
          assistant({
            completionState: "turn_error",
            content:
              "回合中断:所有可用模型端点均不可用：ChatGPT / gpt-5.5（HTTP 503）；DeepSeek / deepseek-v4-pro（HTTP 429）。请检查服务状态或额度，或在模型选择器选择其他端点后重试。",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(screen.getByText(/所有已配置且有凭据的模型端点都暂时不可用/)).toBeInTheDocument();
    expect(screen.getByText(/模型设置/)).toBeInTheDocument();
    expect(screen.getByText("查看失败详情")).toBeInTheDocument();
  });
});
