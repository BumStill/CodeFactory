// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UIMessage } from "../stores/chatEvents";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({ invoke: invokeMock }));

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

  it("shows a durable long-running delivery tool as executing", () => {
    render(
      <MessageList
        messages={[
          assistant({
            turnActivity: {
              rootTurnId: "user",
              revision: 4,
              phase: "delivering",
              status: "active",
              kind: "tool_wait",
              label: "交付任务仍在运行（约 2 分钟）",
              waitingReason: "交付任务已连续运行约 2 分钟",
              updatedAt: NOW,
              terminalReason: null,
            },
          }),
        ]}
        streaming={false}
        turnActive
        turnExecutionActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      "执行中 · 01:24",
    );
  });

  it("keeps an explicit system recovery wait in the waiting phase", () => {
    render(
      <MessageList
        messages={[
          assistant({
            turnActivity: {
              rootTurnId: "user",
              revision: 5,
              phase: "recovering",
              status: "waiting",
              kind: "tool_recovery_waiting",
              label: "外部状态尚未确认，系统正在只读对账",
              waitingReason: "等待安全观察器确认副作用状态",
              updatedAt: NOW,
              terminalReason: "external_state_uncertain",
              objectiveStatus: "waiting_system",
            },
          }),
        ]}
        streaming={false}
        turnActive
        turnExecutionActive
        cwd={null}
      />,
    );

    expect(screen.getByTestId("inline-turn-status")).toHaveTextContent(
      "等待中 · 01:24",
    );
  });

  it("does not revive a stale long-tool heartbeat without a current execution owner", () => {
    render(
      <MessageList
        messages={[
          assistant({
            turnActivity: {
              rootTurnId: "user",
              revision: 6,
              phase: "delivering",
              status: "active",
              kind: "tool_wait",
              label: "交付任务仍在运行（约 2 分钟）",
              waitingReason: "交付任务已连续运行约 2 分钟",
              updatedAt: NOW - 60_000,
              terminalReason: null,
              objectiveStatus: "waiting_system",
            },
          }),
        ]}
        streaming={false}
        turnActive
        turnExecutionActive={false}
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
          turnExecutionActive
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
            },
          }),
          {
            id: "steer-user",
            role: "user",
            content: "换个方向",
            createdAt: NOW - 10_000,
          },
          assistant({
            id: "assistant-after-steer",
            createdAt: NOW - 9_000,
            rootTurnId: "root-user",
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

  it("keeps one completed receipt with whole-turn tokens and elapsed time", () => {
    render(
      <MessageList
        messages={[
          {
            id: "user",
            role: "user",
            content: "开始",
            createdAt: NOW - 84_000,
          },
          assistant({
            content: "完成。",
            durationMs: 84_000,
            turnSettledAt: NOW,
            inputTokens: 12_000,
            outputTokens: 345,
          }),
        ]}
        streaming={false}
        turnActive={false}
        cwd={null}
      />,
    );

    expect(screen.queryByTestId("inline-turn-status")).not.toBeInTheDocument();
    expect(screen.getByTestId("settled-turn-status")).toHaveTextContent(
      "已结束 · 12K tokens · 01:24",
    );
    expect(screen.queryByText(/^用时 /)).not.toBeInTheDocument();
  });

  it("sums token usage across every assistant segment in the root turn", () => {
    render(
      <MessageList
        messages={[
          { id: "user", role: "user", content: "开始", createdAt: NOW - 120_000 },
          assistant({ id: "round-1", content: "阶段一", inputTokens: 8_000, outputTokens: 500 }),
          assistant({
            id: "round-2",
            content: "完成。",
            durationMs: 120_000,
            turnSettledAt: NOW,
            inputTokens: 4_000,
            outputTokens: 345,
          }),
        ]}
        streaming={false}
        turnActive={false}
        cwd={null}
      />,
    );

    expect(screen.getByTestId("settled-turn-status")).toHaveTextContent(
      "已结束 · 13K tokens · 02:00",
    );
  });

  it("keeps one settled receipt anchored to the root turn across a steer", () => {
    render(
      <MessageList
        messages={[
          { id: "root", role: "user", content: "开始", createdAt: NOW - 120_000 },
          assistant({ id: "before-steer", content: "阶段一", rootTurnId: "root", durationMs: 60_000, inputTokens: 2_000 }),
          { id: "steer", role: "user", content: "继续", createdAt: NOW - 60_000 },
          assistant({ id: "after-steer", content: "完成", rootTurnId: "root", durationMs: 10_000, turnSettledAt: NOW, outputTokens: 500 }),
        ]}
        streaming={false}
        turnActive={false}
        cwd={null}
      />,
    );

    expect(screen.getAllByTestId("settled-turn-status")).toHaveLength(1);
    expect(screen.getByTestId("settled-turn-status")).toHaveTextContent(
      "已结束 · 2.5K tokens · 02:00",
    );
  });
});

// M151：卡住的会话在「正在做」旁边给出自救入口——导出事实，或在没有活执行者时
// 重置状态。被拒绝时必须说明原因，不能假装成功。
describe("stuck session recovery", () => {
  const stuck = assistant({
    turnActivity: {
      rootTurnId: "user",
      revision: 1,
      phase: "acting",
      status: "active",
      kind: "thin_running",
      label: "正在做：等待模型返回",
      waitingReason: null,
      updatedAt: NOW,
      terminalReason: null,
    },
  });

  it("exports diagnostics and reports a refused reset instead of claiming success", async () => {
    invokeMock.mockReset();
    invokeMock
      .mockResolvedValueOnce({ session_id: "s1", derived_state: { state: "running" }, cache_matches_facts: false })
      .mockResolvedValueOnce({ reset: false, reason: "会话仍有活着的执行者，请先停止该回合再重置" });

    render(<MessageList messages={[stuck]} streaming={false} cwd={null} conversationKey="s1" />);

    expect(screen.getByTestId("thin-state")).toHaveTextContent("正在做");
    fireEvent.click(screen.getByRole("button", { name: "会话诊断" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("session_diagnostics", { sessionId: "s1" }),
    );

    fireEvent.click(screen.getByRole("button", { name: "重置会话状态" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("reset_session_state", { sessionId: "s1" }),
    );
    expect(
      await screen.findByText("会话仍有活着的执行者，请先停止该回合再重置"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/已重置为/)).toBeNull();
  });
});
