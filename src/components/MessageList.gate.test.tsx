// SPDX-License-Identifier: Apache-2.0
//
// Completion-gate visibility tests for MessageList.
//
// Regression for the 2026-07-16 session: the gate rejected the model's final
// response seven times, every rejected candidate rendered as a full normal
// reply, and the injected recovery instruction was invisible — so the user
// saw the assistant repeat itself for 13 minutes with no explanation. These
// tests pin the visible contract: rejected candidates collapse, gate nudges
// render as system notices (not user bubbles), live gate actions show up.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MessageList } from "./MessageList";
import type { UIMessage } from "../stores/chatEvents";

const msg = (over: Partial<UIMessage> = {}): UIMessage => ({
  id: "m1",
  role: "assistant",
  content: "candidate answer with a very long plan",
  createdAt: Date.now(),
  ...over,
});

describe("MessageList completion-gate visibility", () => {
  it("collapses a rejected candidate reply behind a toggle", () => {
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

    // Collapsed by default: the candidate body is hidden, the notice shows.
    expect(screen.queryByText(/candidate answer with a very long plan/)).toBeNull();
    const toggle = screen.getByText(/被完成度检查驳回的候选回复/);
    expect(toggle).toBeTruthy();
    // The final answer still renders normally.
    expect(screen.getByText(/brief final answer/)).toBeTruthy();

    // Expanding reveals the candidate content.
    fireEvent.click(toggle);
    expect(screen.getByText(/candidate answer with a very long plan/)).toBeTruthy();
  });

  it("renders a gate nudge as a system notice, not a user bubble", () => {
    const { container } = render(
      <MessageList
        messages={[
          msg({
            id: "nudge",
            role: "user",
            content: "The completion gate rejected the attempted final response…",
            completionState: "gate_recovery",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    // System notice label is visible…
    expect(screen.getByText(/完成度检查/)).toBeTruthy();
    // …and it must NOT use the right-aligned user bubble layout.
    expect(container.querySelector(".justify-end")).toBeNull();
  });

  it("shows live gate actions on the streaming assistant message", () => {
    render(
      <MessageList
        messages={[
          msg({
            id: "streaming",
            content: "first attempt…",
            gateActions: [
              {
                kind: "recovery",
                detail: "at least one successful verification is required",
              },
            ],
          }),
        ]}
        streaming={true}
        cwd={null}
      />,
    );

    expect(screen.getByText(/完成度检查介入/)).toBeTruthy();
    expect(screen.getByText(/at least one successful verification is required/)).toBeTruthy();
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
});
