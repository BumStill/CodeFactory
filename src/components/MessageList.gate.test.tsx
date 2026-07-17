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
});
