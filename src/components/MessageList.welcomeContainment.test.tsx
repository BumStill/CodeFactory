// SPDX-License-Identifier: Apache-2.0

import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), selectSession: vi.fn() }));
vi.mock("../stores/chat", () => ({
  useChatStore: () => ({
    sessions: [],
    activeSession: {
      id: "empty-session",
      title: "Untitled",
      cwd: "/tmp/codefactory",
      model_id: "gpt-5.6-sol",
      created_at: 0,
      updated_at: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      kind: "project" as const,
    },
    activeModel: "gpt-5.6-sol",
    selectSession: mocks.selectSession,
  }),
}));
vi.mock("../lib/tauri", () => ({ invoke: mocks.invoke }));

import { MessageList } from "./MessageList";

// Structural guard only. jsdom computes no layout, so it cannot see the defect
// itself — welcome content growing past the column and painting over the
// composer, which buries the input. scripts/verify-composer-overlap-headless.mjs
// measures that in a real browser. What this pins is the structure the fix rests
// on: the positioned wrapper owns the height, and the welcome content scrolls
// inside an `absolute inset-0` child instead of sitting in the wrapper as a
// plain block child that sizes to its own content.
describe("MessageList empty state containment", () => {
  it("scrolls the welcome content inside an absolutely positioned child", () => {
    mocks.invoke.mockRejectedValue(new Error("usage unavailable"));
    const { container } = render(
      <MessageList
        messages={[]}
        streaming={false}
        turnActive={false}
        cwd={null}
        conversationKey="empty-state-containment"
      />,
    );

    const wrapper = container.querySelector(".relative.flex-1.min-h-0");
    expect(wrapper).not.toBeNull();

    const scroller = Array.from(wrapper!.children).find(
      (child) => child.classList.contains("absolute") && child.classList.contains("inset-0"),
    ) as HTMLElement | undefined;
    expect(scroller, "welcome content must be wrapped in an absolute inset-0 box").toBeDefined();
    expect(scroller!.className).toContain("overflow-y-auto");

    // The welcome content lives inside that box, not beside it.
    expect(scroller!.textContent).toContain("CodeFactory");
    // ...and must not open a second scroll container of its own.
    expect(scroller!.querySelectorAll(".overflow-y-auto").length).toBe(0);
  });
});
