// SPDX-License-Identifier: Apache-2.0
//
// Completion-gate isolation tests for MessageList.
//
// Regression for the 2026-07-16 session: the gate rejected the model's final
// response seven times, every rejected candidate rendered as a full normal
// reply, and the injected recovery instruction was invisible — so the user
// saw the assistant repeat itself for 13 minutes with no explanation. These
// tests pin the user-facing contract: internal gate traffic is never rendered,
// while user-actionable warnings and ordinary final answers remain visible.

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
  it("hides rejected candidates instead of exposing internal review history", () => {
    const { container } = render(
      <MessageList
        messages={[
          msg({ id: "candidate", completionState: "rejected_candidate" }),
          msg({ id: "final", content: "brief final answer" }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(screen.queryByText(/candidate answer with a very long plan/)).toBeNull();
    expect(screen.queryByText(/完成度检查|候选回复|点击展开/)).toBeNull();
    expect(screen.getByText(/brief final answer/)).toBeTruthy();
    expect(container.textContent).toBe("brief final answer");
  });

  it("hides persisted recovery and ready instructions entirely", () => {
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
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    expect(container.textContent).toBe("");
    expect(container.querySelector(".justify-end")).toBeNull();
  });

  it("does not expose live gate actions in the assistant timeline", () => {
    const { container } = render(
      <MessageList
        messages={[
          msg({
            id: "streaming",
            content: "正在处理用户要求。",
            gateActions: [
              {
                kind: "recovery",
                detail: "background services require a later successful bounded functional probe",
              },
            ],
          }),
        ]}
        streaming={true}
        cwd={null}
      />,
    );

    expect(screen.getByText(/正在处理用户要求/)).toBeTruthy();
    expect(screen.queryByText(/完成度检查|background services require/)).toBeNull();
    expect(container.textContent).not.toContain("completion");
  });

  it("renders only the self-contained answer after a real recovery event sequence", () => {
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
    expect(container.textContent).toBe(
      "已完成：拆任务能力已内置到当前会话，用户无需进入独立页面。",
    );
    expect(container.querySelector("[data-segment='step']")).toBeNull();
    expect(screen.queryByText(/bash|后台服务|完成度检查|background services/)).toBeNull();
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
            role: "user",
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
