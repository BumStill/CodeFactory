// SPDX-License-Identifier: Apache-2.0
// CF-WINDOW-R5（M86 返修）：全部候选模型都不可用时的等待说明必须**出现在会话
// 界面上**，而不是只写进日志。它要写明每一类原因（时段 / 额度上限 / 缺凭据）、
// 最早可用时间，以及"等待不计入技术恢复次数"。
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { MessageList } from "./MessageList";
import { USAGE_WINDOW_WAIT_LABEL, type UIMessage } from "../stores/chatEvents";

const notice =
  "等待模型可用：ChatGPT 已达订阅用量上限，10-14 14:05 重置；DeepSeek 高峰时段，10-14 12:00 可用；OpenRouter 尚未配置凭据；最早 10-14 12:00 自动继续。等待不计入技术恢复次数；可在设置里放开时段，或换一个模型";

const waitingMessage: UIMessage = {
  id: "usage-window-wait",
  role: "assistant",
  content: "",
  createdAt: Date.now() - 5_000,
  transportRetries: [
    {
      label: USAGE_WINDOW_WAIT_LABEL,
      attempt: 0,
      maxAttempts: 0,
      delayMs: 1000,
      reason: notice,
    },
  ],
};

const connectionRetryMessage: UIMessage = {
  ...waitingMessage,
  id: "connection-retry",
  transportRetries: [
    {
      label: "OpenAI-compatible chat stream request",
      attempt: 1,
      maxAttempts: 3,
      delayMs: 300,
      reason: "HTTP 503 Service Unavailable",
    },
  ],
};

describe("MessageList usage-window wait notice", () => {
  it("shows the wait sentence in the session, expanded and not hidden in a log detail", () => {
    render(
      <MessageList
        messages={[waitingMessage]}
        streaming
        cwd={null}
        conversationKey="usage-window-wait"
      />,
    );

    const status = screen.getByTestId("usage-window-wait-notice");
    expect(status).toHaveAttribute("role", "status");
    expect(status).toBeVisible();
    expect(status).toHaveTextContent("已达订阅用量上限");
    expect(status).toHaveTextContent("高峰时段");
    expect(status).toHaveTextContent("尚未配置凭据");
    expect(status).toHaveTextContent("最早 10-14 12:00 自动继续");
    expect(status).toHaveTextContent("不计入技术恢复次数");
  });

  it("does not describe a usage-window wait as an unstable model connection", () => {
    render(
      <MessageList
        messages={[waitingMessage]}
        streaming
        cwd={null}
        conversationKey="usage-window-wait-plain"
      />,
    );

    expect(screen.queryByText(/模型连接不稳定/)).not.toBeInTheDocument();
  });

  it("keeps real transport retries in the collapsible connection detail", () => {
    render(
      <MessageList
        messages={[connectionRetryMessage]}
        streaming
        cwd={null}
        conversationKey="connection-retry"
      />,
    );

    expect(screen.queryByTestId("usage-window-wait-notice")).not.toBeInTheDocument();
    expect(screen.getByText(/模型连接不稳定/)).toBeInTheDocument();
  });
});
