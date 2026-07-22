// SPDX-License-Identifier: Apache-2.0
//
// IME composition guard. Field report: with the input-method candidate list
// open, Enter is meant to COMMIT the composition — but the handler fired the
// chat send instead. Two engine behaviors must both be covered:
//   - Chromium: the committing Enter keydown arrives while composing.
//   - WebKit (our Tauri runtime on macOS): compositionend fires FIRST, then
//     the same physical Enter arrives as a normal keydown — only a short
//     time window can tell it apart from a real send.

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MessageInput } from "./MessageInput";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.useRealTimers();
});

function setup() {
  const onSend = vi.fn();
  render(
    <MessageInput
      onSend={onSend}
      onGuide={vi.fn()}
      onCancel={() => {}}
      streaming={false}
      guidanceActive={false}
      disabled={false}
      cwd="/proj"
    />,
  );
  return { onSend, textarea: screen.getByRole("textbox") };
}

describe("MessageInput IME guard", () => {
  it("does not send while a composition is active (Chromium ordering)", () => {
    const { onSend, textarea } = setup();
    fireEvent.change(textarea, { target: { value: "拼音" } });
    fireEvent.compositionStart(textarea);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("does not send on the trailing Enter right after compositionend (WebKit ordering)", () => {
    vi.useFakeTimers();
    const { onSend, textarea } = setup();
    fireEvent.change(textarea, { target: { value: "中文内容" } });
    fireEvent.compositionStart(textarea);
    fireEvent.compositionEnd(textarea);
    // The same physical Enter that committed the candidate arrives a few ms
    // later as a plain keydown.
    vi.advanceTimersByTime(10);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();

    // A REAL send after the window passes goes through.
    vi.advanceTimersByTime(300);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSend).toHaveBeenCalledWith("中文内容");
  });

  it("plain Enter with no composition still sends immediately", () => {
    const { onSend, textarea } = setup();
    fireEvent.change(textarea, { target: { value: "hello" } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSend).toHaveBeenCalledWith("hello");
  });
});
