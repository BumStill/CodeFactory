// SPDX-License-Identifier: Apache-2.0
//
// Completion-gate isolation tests for MessageList.
//
// Two field reports shape this contract. 2026-07-16: every rejected candidate
// rendered as a full normal reply, so the assistant appeared to repeat itself
// for 13 minutes. 2026-07-25: the fix for that erased the whole turn instead —
// one session lost 1111 rows of visible work, up to 152 steps at once.
//
// The contract that satisfies both: the gate's own control traffic (injected
// prompts) never renders, and nothing the model actually did is ever deleted.
// Drafts and recovery rounds stay in the timeline as ordinary steps; only the
// last prose block is the answer.

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { MessageList } from "./MessageList";
import {
  reduceChatStreamEvent,
  type ChatEventState,
  type UIMessage,
} from "../stores/chatEvents";

const msg = (over: Partial<UIMessage> = {}): UIMessage => ({
  id: "m1",
  role: "assistant",
  content: "candidate answer with a very long plan",
  createdAt: Date.now(),
  ...over,
});

describe("MessageList completion-review isolation", () => {
  it("keeps a rejected draft visible as an ordinary step in the turn", () => {
    render(
      <MessageList
        messages={[
          msg({ id: "candidate", completionState: "rejected_candidate" }),
          msg({ id: "final", content: "brief final answer" }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(screen.getByText(/candidate answer with a very long plan/)).toBeTruthy();
    expect(screen.getByText(/brief final answer/)).toBeTruthy();
    expect(screen.queryByText(/完成度检查|执行已中断|第 \d\/3 次/)).toBeNull();
  });

  it("hides persisted gate instructions entirely", () => {
    const { container } = render(
      <MessageList
        messages={[
          msg({
            id: "recovery",
            role: "user",
            content: "The completion gate rejected the attempted final response…",
            completionState: "gate_recovery",
          }),
          msg({
            id: "ready",
            role: "user",
            content: "The structured completion evidence is satisfied…",
            completionState: "gate_ready",
          }),
          msg({
            id: "blocked",
            role: "user",
            content: "Completion blocked because required verification is still missing…",
            completionState: "gate_blocked",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(container.textContent).toBe("");
    expect(container.querySelector(".justify-end")).toBeNull();
  });

  it("keeps the recovery round's work on screen and ends with the final answer", () => {
    let state: ChatEventState = {
      messages: [
        msg({
          id: "streaming",
          content: "先执行与用户无关的内部步骤。",
          toolCalls: [
            { id: "old", name: "bash", args: "{}", status: "done", result: "ok" },
          ],
          segments: [
            { kind: "text", text: "先执行与用户无关的内部步骤。" },
            { kind: "tool", toolCallId: "old" },
          ],
        }),
      ],
      streaming: true,
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
    };

    state = reduceChatStreamEvent(
      state,
      {
        type: "completion_gate_action",
        kind: "recovery",
        detail: "background services require a later successful bounded functional probe",
      },
      "streaming",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "text_delta", content: "后台服务已运行，现在执行后续探针。" },
      "streaming",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "tool_call_start", id: "probe", name: "bash", args: {} },
      "streaming",
    );
    state = reduceChatStreamEvent(
      state,
      { type: "completion_gate_action", kind: "ready", detail: "" },
      "streaming",
    );
    state = reduceChatStreamEvent(
      state,
      {
        type: "text_delta",
        content: "已完成：拆任务能力已内置到当前会话，用户无需进入独立页面。",
      },
      "streaming",
    );

    const { container } = render(
      <MessageList messages={state.messages} streaming={false} cwd={null} />,
    );
    // Everything the model did survives, in order…
    expect(screen.getByText(/先执行与用户无关的内部步骤/)).toBeTruthy();
    expect(screen.getByText(/后台服务已运行，现在执行后续探针/)).toBeTruthy();
    // …as dim step lines, with only the last prose block as the answer.
    expect(container.querySelectorAll("[data-segment='step']").length).toBe(2);
    expect(
      screen.getByText("已完成：拆任务能力已内置到当前会话，用户无需进入独立页面。"),
    ).toBeTruthy();
    // The gate's own vocabulary still never reaches the screen.
    expect(screen.queryByText(/完成度检查|执行已中断|background services/)).toBeNull();
  });

  it("shows a user-facing verification warning without internal gate wording", () => {
    render(
      <MessageList
        messages={[
          msg({
            id: "warning",
            content: "功能已经内置到会话。",
            gateActions: [
              { kind: "warning", detail: "⚠ 仍有一项检查未通过。" },
            ],
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(screen.getByText("功能已经内置到会话。")).toBeTruthy();
    expect(screen.getByText("⚠ 仍有一项检查未通过。")).toBeTruthy();
    expect(screen.queryByText(/完成度检查|completion gate/)).toBeNull();
  });

  it("renders a persisted turn error as a visible error notice, not a user bubble", () => {
    // 2026-07-21 field report: four interruptions, zero forensic trace — the
    // error only existed as a transient stream event. Persisted turn errors
    // must render as an error notice and survive reloads.
    const { container } = render(
      <MessageList
        messages={[
          msg({
            id: "err",
            role: "user",
            content: "[回合错误] 400 This model does not support image input",
            completionState: "turn_error",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );
    expect(screen.getByText(/回合中断/)).toBeTruthy();
    expect(screen.getByText(/does not support image input/)).toBeTruthy();
    expect(container.querySelector(".justify-end")).toBeNull();
  });

  it("renders a turn notice (e.g. images stripped for a no-vision model) as a neutral notice", () => {
    const { container } = render(
      <MessageList
        messages={[
          msg({
            id: "notice",
            role: "system",
            content: "已自动移除历史中的图片后重试:当前模型不支持图片输入。",
            completionState: "turn_notice",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );
    expect(screen.getByText(/已自动移除历史中的图片/)).toBeTruthy();
    expect(container.querySelector(".justify-end")).toBeNull();
  });

  it("renders a verification-incomplete warning as an amber notice, with the reply left visible", () => {
    // 2026-07-21 field report: exhausting gate recovery used to FOLD the
    // reply and kill the turn with an untranslated internal error. Now the
    // reply stands and a plain-Chinese warning follows it.
    const { container } = render(
      <MessageList
        messages={[
          msg({ id: "answer", content: "这是最终回复内容。" }),
          msg({
            id: "warn",
            role: "user",
            content: "⚠ 以上回复未经完整验证:本轮修改后仍有检查未复验。",
            completionState: "gate_warning",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );
    expect(screen.getByText(/这是最终回复内容/)).toBeTruthy();
    expect(screen.getByText(/未经完整验证/)).toBeTruthy();
    expect(container.querySelector(".justify-end")).toBeNull();
  });
});
