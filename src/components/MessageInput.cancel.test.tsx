// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MessageInput } from "./MessageInput";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));

describe("MessageInput cancellation contract", () => {
  it("keeps the idle prompt concise and exposes attachment support on the control", () => {
    render(
      <MessageInput
        onSend={vi.fn()}
        onCancel={vi.fn()}
        streaming={false}
        disabled={false}
        cwd="/proj"
      />,
    );

    expect(screen.getByPlaceholderText("描述任务或继续对话…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "附加文件" })).toHaveAttribute(
      "title",
      expect.stringContaining("pptx"),
    );
    expect(screen.getByRole("button", { name: "发送" })).toBeInTheDocument();

    const controlRow = screen.getByTestId("message-input-control-row");
    const prompt = screen.getByPlaceholderText("描述任务或继续对话…");
    expect(controlRow).toHaveClass("group", "rounded-2xl");
    expect(screen.getByRole("button", { name: "附加文件" })).toHaveClass("h-[44px]", "w-[44px]", "lg:h-[36px]", "lg:w-[36px]");
    expect(prompt).toHaveClass("min-h-8", "py-1");
    expect(screen.getByRole("button", { name: "发送" })).toHaveClass("h-[44px]", "w-[44px]", "lg:h-[36px]", "lg:w-[36px]");
  });

  it("explains that stopping future generation does not roll back completed work", () => {
    render(
      <MessageInput
        onSend={vi.fn()}
        onCancel={vi.fn()}
        streaming={true}
        disabled={false}
        cwd="/proj"
      />,
    );

    expect(screen.getByTitle("停止后续生成")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "停止后续生成" })).toBeInTheDocument();
    expect(
      screen.getByText("停止后续生成不会撤销已经完成的修改、提交或推送"),
    ).toBeInTheDocument();
  });

  it("queues the explicit after-current action instead of steering the active run", async () => {
    const onSend = vi.fn();
    const onGuide = vi.fn().mockResolvedValue(undefined);
    render(
      <MessageInput
        onSend={onSend}
        onGuide={onGuide}
        onCancel={vi.fn()}
        streaming
        guidanceActive
        disabled={false}
        cwd="/proj"
      />,
    );

    await userEvent.type(screen.getByRole("textbox"), "完成后补一项检查");
    await userEvent.click(
      screen.getByRole("button", { name: "排到当前执行之后" }),
    );

    expect(onSend).toHaveBeenCalledWith("完成后补一项检查");
    expect(onGuide).not.toHaveBeenCalled();
  });
});
