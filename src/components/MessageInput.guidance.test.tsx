// SPDX-License-Identifier: Apache-2.0
//
// Steering regression tests for the main chat input. Whenever a run is in
// flight — a streaming chat turn or an autonomous task run — typed text steers
// it by default rather than waiting out the whole turn. ⌘/Ctrl+Enter is the
// per-message escape hatch back to "do this after you finish".

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
  it("shows shortcut help only for a focused desktop composer and keeps controls 44/36px", () => {
    const { onSend } = setup();
    const shortcut = screen.getByTestId("composer-shortcut-hint");
    expect(shortcut).toHaveClass("hidden", "lg:group-focus-within:block");
    expect(screen.getByTestId("message-input-control-row")).toHaveClass("border-control-border");

    const attach = screen.getByRole("button", { name: "附加文件" });
    const send = screen.getByRole("button", { name: "引导当前执行" });
    for (const target of [attach, send]) {
      expect(target).toHaveClass("h-11", "w-11", "lg:h-9", "lg:w-9");
    }
    expect(onSend).not.toHaveBeenCalled();
  });

  it("routes Enter submissions to onGuide instead of normal chat send", async () => {
    const user = userEvent.setup();
    const { onSend, onGuide } = setup();
    const textarea = screen.getByRole("textbox");

    await user.type(textarea, "先修中断后排队发送{Enter}");

    expect(onGuide).toHaveBeenCalledWith("先修中断后排队发送");
    expect(onSend).not.toHaveBeenCalled();
    expect(textarea).toHaveValue("");
    expect(
      screen.getByText("Enter 引导当前执行 · ⌘Enter 等这轮结束再发 · Shift+Enter 换行"),
    ).toBeInTheDocument();
  });

  it("routes ⌘Enter to the normal send path so it queues for after the run", () => {
    const onSend = vi.fn();
    const onGuide = vi.fn();
    render(
      <MessageInput
        onSend={onSend}
        onGuide={onGuide}
        onCancel={() => {}}
        streaming={true}
        guidanceActive={true}
        disabled={false}
        cwd="/proj"
      />,
    );
    const textarea = screen.getByRole("textbox");

    fireEvent.change(textarea, { target: { value: "这件事等你忙完再说" } });
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });

    expect(onSend).toHaveBeenCalledWith("这件事等你忙完再说");
    expect(onGuide).not.toHaveBeenCalled();
  });

  it("treats Ctrl+Enter the same way for non-mac keyboards", () => {
    const onSend = vi.fn();
    const onGuide = vi.fn();
    render(
      <MessageInput
        onSend={onSend}
        onGuide={onGuide}
        onCancel={() => {}}
        streaming={true}
        guidanceActive={true}
        disabled={false}
        cwd="/proj"
      />,
    );
    const textarea = screen.getByRole("textbox");

    fireEvent.change(textarea, { target: { value: "稍后处理" } });
    fireEvent.keyDown(textarea, { key: "Enter", ctrlKey: true });

    expect(onSend).toHaveBeenCalledWith("稍后处理");
    expect(onGuide).not.toHaveBeenCalled();
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
    expect(screen.queryByText("已送出")).not.toBeInTheDocument();
    expect(textarea).toHaveValue("先修失败测试");

    resolveGuide?.();
    await waitFor(() => expect(screen.getByText("已送出")).toBeInTheDocument());
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
