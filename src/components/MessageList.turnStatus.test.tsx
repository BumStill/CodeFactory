// SPDX-License-Identifier: Apache-2.0

import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UIMessage } from "../stores/chatEvents";
import { MessageList } from "./MessageList";

const NOW = new Date("2026-08-18T10:00:00.000Z").getTime();

function assistant(overrides: Partial<UIMessage> = {}): UIMessage {
  return {
    id: "assistant",
    role: "assistant",
    content: "",
    createdAt: NOW - 84_000,
    ...overrides,
  };
}

describe("inline active-turn status", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows one compact Thinking line with the whole turn clock", () => {
    render(
      <MessageList
        messages={[assistant()]}
        streaming
        turnActive
        cwd={null}
      />,
    );

    const status = screen.getByTestId("inline-turn-status");
    expect(status).toHaveTextContent("Thinking · 01:24");
    expect(screen.queryByText("正在处理")).not.toBeInTheDocument();
    expect(screen.queryByText(/^运行中/)).not.toBeInTheDocument();
  });

  it.each([
    [
      "执行中",
      assistant({
        toolCalls: [{ id: "tool", name: "bash", args: "{}", status: "running" }],
      }),
    ],
    [
      "等待中",
      assistant({
        toolCalls: [{ id: "tool", name: "bash", args: "{}", status: "waiting_permission" }],
      }),
    ],
    [
      "整理结果",
      assistant({
        turnActivity: {
          rootTurnId: "user",
          revision: 2,
          phase: "finalizing",
          status: "active",
          kind: "finalizing",
          label: "正在形成最终结果",
          waitingReason: null,
          updatedAt: NOW,
          terminalReason: null,
        },
      }),
    ],
  ])("maps the active phase to %s", (label, message) => {
    render(
      <MessageList
        messages={[message]}
        streaming
        turnActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      `${label} · 01:24`,
    );
  });

  it("stays visible for a durable turn between stream segments", () => {
    render(
      <MessageList
        messages={[assistant()]}
        streaming={false}
        turnActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      "等待中 · 01:24",
    );
  });

  it("stays visible when a durable turn owns an otherwise empty conversation", () => {
    render(
      <MessageList
        messages={[]}
        streaming={false}
        turnActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      "等待中 · 00:00",
    );
  });

  it.each([
    [false, []],
    [
      true,
      [{
        label: "OpenAI-compatible chat stream request",
        attempt: 1,
        maxAttempts: 3,
        delayMs: 300,
        reason: "HTTP 503 Service Unavailable",
      }],
    ],
  ])(
    "keeps a running tool in the executing phase (streaming=%s)",
    (streaming, transportRetries) => {
      render(
        <MessageList
          messages={[
            assistant({
              toolCalls: [{ id: "tool", name: "bash", args: "{}", status: "running" }],
              transportRetries,
            }),
          ]}
          streaming={streaming}
          turnActive
          cwd={null}
        />,
      );

      expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
        "执行中 · 01:24",
      );
    },
  );

  it("keeps counting from the root user turn across newer assistant segments", () => {
    render(
      <MessageList
        messages={[
          {
            id: "root-user",
            role: "user",
            content: "完成任务",
            createdAt: NOW - 120_000,
          },
          assistant({
            createdAt: NOW - 10_000,
            turnActivity: {
              rootTurnId: "root-user",
              revision: 3,
              phase: "working",
              status: "active",
              kind: "turn_running",
              label: "正在执行任务",
              waitingReason: null,
              updatedAt: NOW,
              terminalReason: null,
            },
          }),
        ]}
        streaming
        turnActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      "Thinking · 02:00",
    );
  });

  it("keeps the root clock while a steered assistant segment awaits projection", () => {
    render(
      <MessageList
        messages={[
          {
            id: "root-user",
            role: "user",
            content: "完成任务",
            createdAt: NOW - 120_000,
          },
          assistant({
            id: "assistant-before-steer",
            createdAt: NOW - 110_000,
            turnActivity: {
              rootTurnId: "root-user",
              revision: 3,
              phase: "working",
              status: "active",
              kind: "turn_running",
              label: "正在执行任务",
              waitingReason: null,
              updatedAt: NOW,
              terminalReason: null,
              objectiveStatus: "active",
            },
          }),
          {
            id: "steer-user",
            role: "user",
            content: "换个方向",
            createdAt: NOW - 10_000,
          },
          assistant({ id: "assistant-after-steer", createdAt: NOW - 9_000 }),
        ]}
        streaming
        turnActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      "Thinking · 02:00",
    );
  });

  it("disappears immediately after the turn settles", () => {
    render(
      <MessageList
        messages={[assistant({ content: "完成。", durationMs: 84_000 })]}
        streaming={false}
        turnActive={false}
        cwd={null}
      />,
    );

    expect(screen.queryByTestId("inline-turn-status")).not.toBeInTheDocument();
  });
});
