// SPDX-License-Identifier: Apache-2.0
//
// MessageInput 的"发送读取真实内容"契约测试(R3)与菜单栏发送入口的语义。
//
// 背景:菜单栏的「从剪贴板发送」和「发送输入框内容」不经过键盘,写入输入框的
// 方式也不是 React 的 onChange(setValue / 无障碍写 AXValue / 直接改 .value)。
// 如果发送只看 React state,用户会看到输入框里有字、点发送却什么都没发出去。
//
// jsdom 不能验证真实键盘/无障碍行为,这里验证的是决策与数据流:发送时读的是
// DOM 值、空内容不发、剪贴板文本走与手工输入完全相同的提交路径(包括进行中
// 回合的「引导」语义)。

import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createRef } from "react";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: vi.fn((path: string) => `asset://${path}`) }));

import { MessageInput, readComposerText, type MessageInputHandle } from "./MessageInput";

function setup(props: Partial<React.ComponentProps<typeof MessageInput>> = {}) {
  const onSend = vi.fn();
  const onGuide = vi.fn().mockResolvedValue(undefined);
  const onCancel = vi.fn();
  const ref = createRef<MessageInputHandle>();
  const utils = render(
    <MessageInput
      ref={ref}
      onSend={onSend}
      onGuide={onGuide}
      onCancel={onCancel}
      streaming={false}
      disabled={false}
      cwd="/proj"
      {...props}
    />,
  );
  return { ...utils, onSend, onGuide, onCancel, ref };
}

/** 像粘贴 / 无障碍写值那样写进输入框:改 DOM 值,然后发原生 input 事件。 */
function writeIntoComposer(textarea: HTMLTextAreaElement, text: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  setter?.call(textarea, text);
  fireEvent.input(textarea);
}

describe("readComposerText", () => {
  it("DOM 值与 React state 不一致时以 DOM 为准", () => {
    const textarea = document.createElement("textarea");
    textarea.value = "用户眼前看到的内容";
    expect(readComposerText(textarea, "过期的 state")).toBe("用户眼前看到的内容");
  });

  it("两者一致时取同一个值,并去掉首尾空白", () => {
    const textarea = document.createElement("textarea");
    textarea.value = " 内容 ";
    expect(readComposerText(textarea, " 内容 ")).toBe("内容");
  });

  it("没有输入框时退回 state", () => {
    expect(readComposerText(null, "只有 state")).toBe("只有 state");
  });
});

describe("发送读取输入框的真实内容", () => {
  it("用非键盘方式写入后点发送,发出的正是写进去的内容", async () => {
    const { onSend } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    act(() => {
      writeIntoComposer(textarea, "把这一列总结到下一列");
    });
    fireEvent.click(screen.getByLabelText("发送"));
    await waitFor(() => expect(onSend).toHaveBeenCalledWith("把这一列总结到下一列"));
    expect(textarea.value).toBe("");
  });

  it("只改 DOM 值、连 input 事件都没有时,菜单栏的发送仍然发出真实内容", async () => {
    const { onSend, ref } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // 模拟"无障碍工具直接写值":React state 完全不知道这件事。
    textarea.value = "这条内容只存在于 DOM 里";
    await act(async () => {
      await ref.current?.submit();
    });
    expect(onSend).toHaveBeenCalledWith("这条内容只存在于 DOM 里");
  });

  it("空内容不发送", async () => {
    const { onSend, ref } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // 只有一个空行:trim 之后没有内容,不能发出去。
    textarea.value = "   \n  ";
    await act(async () => {
      await ref.current?.submit();
    });
    expect(onSend).not.toHaveBeenCalled();
    expect(textarea.value).toBe("   \n  ");
  });

  it("空白内容点发送按钮也不发送", async () => {
    const { onSend } = setup();
    fireEvent.click(screen.getByLabelText("发送"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(onSend).not.toHaveBeenCalled();
  });
});

describe("菜单栏的发送入口", () => {
  it("从剪贴板发送:文本先进输入框,再走同一条发送路径", async () => {
    const { onSend, ref } = setup();
    await act(async () => {
      await ref.current?.sendText("来自剪贴板的一句话");
    });
    expect(onSend).toHaveBeenCalledWith("来自剪贴板的一句话");
  });

  it("从剪贴板发送空文本时不发送", async () => {
    const { onSend, ref } = setup();
    await act(async () => {
      await ref.current?.sendText("   ");
    });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("回合进行中时,剪贴板发送与手工输入一样走「引导当前执行」", async () => {
    const { onSend, onGuide, ref } = setup({ streaming: true, guidanceActive: true });
    await act(async () => {
      await ref.current?.sendText("改一下方向");
    });
    expect(onGuide).toHaveBeenCalledWith("改一下方向");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("聚焦输入框把焦点交给输入框", () => {
    const { ref } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    act(() => {
      ref.current?.focus();
    });
    expect(document.activeElement).toBe(textarea);
  });
});
