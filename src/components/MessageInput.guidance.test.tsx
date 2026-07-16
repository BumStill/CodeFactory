// SPDX-License-Identifier: Apache-2.0
//
// Guidance-mode regression tests for the main chat input. During autonomous
// task execution, text typed into the primary input should be queued as a
// scheduler interjection ("引导下一步") instead of becoming a normal chat turn.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MessageInput } from "./MessageInput";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));

function setup(onGuide = vi.fn()) {
  const onSend = vi.fn();
  render(
    <MessageInput
      onSend={onSend}
      onGuide={onGuide}
      onCancel={() => {}}
      streaming={false}
      guidanceActive={true}
      disabled={false}
      cwd="/proj"
    />,
  );
  return { onSend, onGuide };
}

describe("MessageInput guidance mode", () => {
  it("routes Enter submissions to onGuide instead of normal chat send", async () => {
    const user = userEvent.setup();
    const { onSend, onGuide } = setup();
    const textarea = screen.getByRole("textbox");

    await user.type(textarea, "先修中断后排队发送{Enter}");

    expect(onGuide).toHaveBeenCalledWith("先修中断后排队发送");
    expect(onSend).not.toHaveBeenCalled();
    expect(textarea).toHaveValue("");
    expect(screen.getByText("Enter 引导下一步 · Shift+Enter 换行")).toBeInTheDocument();
  });

  it("keeps the draft and reports an error when guidance cannot be queued", async () => {
    const onGuide = vi.fn().mockRejectedValue(new Error("scheduler unavailable"));
    const user = userEvent.setup();
    setup(onGuide);
    const textarea = screen.getByRole("textbox");

    await user.type(textarea, "保留这条引导{Enter}");

    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("scheduler unavailable"));
    expect(textarea).toHaveValue("保留这条引导");
  });

  it("shows a queued confirmation only after guidance succeeds", async () => {
    let resolveGuide: (() => void) | undefined;
    const onGuide = vi.fn(() => new Promise<void>((resolve) => { resolveGuide = resolve; }));
    const user = userEvent.setup();
    setup(onGuide);
    const textarea = screen.getByRole("textbox");

    await user.type(textarea, "先修失败测试{Enter}");
    expect(screen.queryByText("已加入下一任务")).not.toBeInTheDocument();
    expect(textarea).toHaveValue("先修失败测试");

    resolveGuide?.();
    await waitFor(() => expect(screen.getByText("已加入下一任务")).toBeInTheDocument());
    expect(textarea).toHaveValue("");
  });

  it("keeps slash commands local and does not send them as guidance", () => {
    const onCommand = vi.fn();
    const onSend = vi.fn();
    const onGuide = vi.fn();
    render(
      <MessageInput
        onSend={onSend}
        onGuide={onGuide}
        onCommand={onCommand}
        onCancel={() => {}}
        streaming={false}
        guidanceActive={true}
        disabled={false}
        cwd="/proj"
      />,
    );
    const textarea = screen.getByRole("textbox");

    fireEvent.change(textarea, { target: { value: "/cwd /tmp/project" } });
    fireEvent.keyDown(textarea, { key: "Enter" });

    expect(onCommand).toHaveBeenCalled();
    expect(onGuide).not.toHaveBeenCalled();
    expect(onSend).not.toHaveBeenCalled();
  });
});
