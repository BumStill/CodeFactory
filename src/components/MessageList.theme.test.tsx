// SPDX-License-Identifier: Apache-2.0
//
// Theme-readability regression tests for MessageList.
//
// jsdom doesn't compute CSS, so we can't measure actual rendered contrast
// here. What we CAN guarantee is that styling decisions which break light
// mode (e.g. unconditional `prose-invert`, hardcoded `text-amber-200` on
// inline code) don't sneak back in. Each test below corresponds to a real
// visual bug we shipped in v0.5.1 and want to prevent regressing.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const convertFileSrcMock = vi.hoisted(() => vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: convertFileSrcMock }));

import { MessageList } from "./MessageList";
import type { UIMessage } from "../stores/chatEvents";

const baseMsg = (over: Partial<UIMessage> = {}): UIMessage => ({
  id: "m1",
  role: "assistant",
  content: "Hello **world**, use `foo()` to start.",
  createdAt: Date.now(),
  ...over,
});

describe("MessageList theme readability", () => {

  it("never applies prose-invert unconditionally — only via dark: prefix", () => {
    // prose-invert forces a dark-mode color palette on rendered markdown
    // (white headings, light-gray body, etc.). On a light surface this
    // becomes effectively invisible — which is exactly what shipped in v0.5.1.
    //
    // The fix is to gate it behind Tailwind's `dark:` prefix, which our
    // tailwind.config already maps to `[data-theme="dark"]`. The test fails
    // if a future change resurrects the unconditional class.
    const { container } = render(
      <MessageList messages={[baseMsg()]} streaming={false} cwd={null} />,
    );
    const prose = container.querySelector(".prose");
    expect(prose, "expected a .prose markdown container").toBeTruthy();

    const cls = prose!.className;
    // Allowed:    "prose dark:prose-invert ..."
    // Forbidden:  "prose prose-invert ..."  (unconditional)
    expect(cls).toMatch(/\bdark:prose-invert\b/);
    expect(cls).not.toMatch(/(^|\s)prose-invert(\s|$)/);
  });

  it("inline code color is theme-aware (not hardcoded text-amber-200)", () => {
    // amber-200 (#fde68a) on a white background = ~1.3:1 contrast — invisible.
    // The correct pattern is `text-amber-700 dark:text-amber-200` (or any
    // light-mode-readable base color), which produces ~7.5:1 on white and
    // ~6:1 on dark surface.
    const { container } = render(
      <MessageList messages={[baseMsg()]} streaming={false} cwd={null} />,
    );
    const code = container.querySelector("code");
    expect(code, "expected an inline <code> element").toBeTruthy();

    const cls = code!.className;
    // Forbidden: unconditional text-amber-200 (was the v0.5.1 bug).
    expect(cls).not.toMatch(/(^|\s)text-amber-200(\s|$)/);
    expect(cls).not.toMatch(/\btext-amber-/);
  });

  it("centers the transcript in a bounded modern reading column", () => {
    render(
      <MessageList messages={[baseMsg()]} streaming={false} cwd={null} />,
    );

    expect(screen.getByTestId("conversation-reading-column")).toHaveClass(
      "max-w-[880px]",
    );
  });

  it("uses Chinese processing copy instead of an English thinking hint", () => {
    render(
      <MessageList
        messages={[baseMsg({ content: "", toolCalls: [] })]}
        streaming
        cwd={null}
      />,
    );

    expect(screen.getByText("正在处理")).toBeInTheDocument();
    expect(screen.queryByText("Thinking")).not.toBeInTheDocument();
  });

  it("shows model transport retries as a quiet expandable assistant status", () => {
    render(
      <MessageList
        messages={[
          baseMsg({
            content: "",
            transportRetries: [
              {
                label: "OpenAI-compatible chat stream request",
                attempt: 1,
                maxAttempts: 3,
                delayMs: 300,
                reason: "HTTP 503 Service Unavailable",
              },
            ],
          }),
        ]}
        streaming={true}
        cwd={null}
      />,
    );

    const summary = screen.getByText("模型连接不稳定，正在重新连接…");
    expect(summary.closest("details")).toBeTruthy();
    expect(summary.closest("details")?.className).not.toMatch(/\bborder\b|\bbg-/);
    expect(screen.getByText(/HTTP 503 Service Unavailable/)).toBeTruthy();
    expect(screen.queryByText(/Thinking/)).toBeNull();
  });

  it("renders GFM tables as HTML table elements, not raw pipe text", () => {
    const { container } = render(
      <MessageList
        messages={[baseMsg({ content: "| A | B |\n| --- | --- |\n| 1 | 2 |" })]}
        streaming={false}
        cwd={null}
      />,
    );
    expect(container.querySelector("table"), "expected <table> in rendered output").toBeTruthy();
    expect(container.querySelector("th"), "expected <th> in rendered table").toBeTruthy();
    expect(container.querySelector("th")?.textContent).toBe("A");
  });

  it("renders grouped successful tools as an unframed conversational activity line", () => {
    const { container } = render(
      <MessageList
        messages={[
          baseMsg({
            role: "assistant",
            content: "",
            toolCalls: [
              { id: "t1", name: "read_file", args: JSON.stringify({ path: "a.ts" }), result: "ok", status: "done", isError: false },
              { id: "t2", name: "read_file", args: JSON.stringify({ path: "b.ts" }), result: "ok", status: "done", isError: false },
              { id: "t3", name: "bash", args: JSON.stringify({ command: "git status" }), result: "ok", status: "done", isError: false },
            ],
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );
    const group = screen.getByRole("button", { name: /查看 3 个已完成操作/ }).parentElement;
    const classes = group?.className.split(/\s+/) ?? [];
    expect(classes).not.toContain("border-b");
    expect(classes).not.toContain("border");
    expect(group?.className).not.toMatch(/bg-surface/);
    expect(container.querySelector("[data-tool-group='success']"), "expected a low-emphasis success group").toBeTruthy();
  });

  it("keeps persisted assistant/tool rounds close and reserves metadata for the settled answer", () => {
    const { container } = render(
      <MessageList
        messages={[
          baseMsg({ id: "u1", role: "user", content: "修复并验证", createdAt: 1 }),
          baseMsg({
            id: "a1",
            content: "已保存检查点，接下来继续验证。",
            createdAt: 2,
            durationMs: 1_000,
          }),
          baseMsg({
            id: "a2",
            content: "接着运行聚焦测试。",
            createdAt: 3,
            durationMs: 2_000,
            toolCalls: [
              { id: "t2", name: "bash", args: JSON.stringify({ command: "pnpm test" }), result: "ok", status: "done" },
            ],
          }),
          baseMsg({
            id: "a3",
            content: "修复完成，测试通过。",
            createdAt: 4,
            durationMs: 3_000,
          }),
        ]}
        streaming={false}
        cwd="/project"
      />,
    );

    expect(container.querySelector("[data-message-row='a1']")).toHaveAttribute(
      "data-message-flow",
      "turn-start",
    );
    expect(container.querySelector("[data-message-row='a2']")).toHaveAttribute(
      "data-message-flow",
      "turn-continuation",
    );
    expect(screen.queryByText("Remember")).not.toBeInTheDocument();
    expect(screen.getAllByText(/用时/)).toHaveLength(1);
  });

  it("keeps operational narration at body size while elapsed time stays auxiliary", () => {
    render(
      <MessageList
        messages={[
          baseMsg({
            id: "assistant-live",
            content: "done",
            createdAt: Date.now() - 2_000,
            segments: [
              { kind: "text", text: "Checking the workspace." },
              { kind: "text", text: "done" },
            ],
          }),
        ]}
        streaming
        cwd={null}
      />,
    );

    expect(
      screen.getByText("Checking the workspace.").closest("[data-segment='step']"),
    ).toHaveClass("text-[15px]");
    expect(screen.getByText(/运行中/)).toHaveClass("text-[11px]");
  });

  it("renders markdown image links as visible image previews", () => {
    const { container } = render(
      <MessageList
        messages={[baseMsg({ content: "![image.png](file:///proj/.codefactory/attachments/image.png)" })]}
        streaming={false}
        cwd={null}
      />,
    );
    const image = container.querySelector("img[alt='image.png']");
    expect(image, "expected markdown image to render as <img>").toBeTruthy();
    expect(convertFileSrcMock).toHaveBeenCalledWith("/proj/.codefactory/attachments/image.png");
    expect(image).toHaveAttribute("src", "asset://localhost/%2Fproj%2F.codefactory%2Fattachments%2Fimage.png");
    expect(image?.className).toMatch(/max-h-80/);
  });

  it("renders markdown image links with spaces in file paths as visible image previews", () => {
    const { container } = render(
      <MessageList
        messages={[baseMsg({ content: "![IMG_6190.png](file:///Users/leo/Projects/AI foundation/.codefactory/attachments/1785309543980-84d170b1.png)" })]}
        streaming={false}
        cwd={null}
      />,
    );

    const image = container.querySelector("img[alt='IMG_6190.png']");
    expect(image, "expected image markdown with spaces in its file path to render as <img>").toBeTruthy();
    expect(convertFileSrcMock).toHaveBeenCalledWith("/Users/leo/Projects/AI foundation/.codefactory/attachments/1785309543980-84d170b1.png");
    expect(screen.queryByText(/!\[IMG_6190\.png\]/)).not.toBeInTheDocument();
  });

  it("opens message image previews in a larger viewer", async () => {
    const user = userEvent.setup();
    render(
      <MessageList
        messages={[baseMsg({ content: "![image.png](file:///proj/.codefactory/attachments/image.png)" })]}
        streaming={false}
        cwd={null}
      />,
    );

    await user.click(screen.getByRole("button", { name: "放大查看 image.png" }));

    const dialog = screen.getByRole("dialog", { name: "图片预览" });
    expect(dialog).toBeInTheDocument();
    expect(within(dialog).getByRole("img", { name: "image.png" })).toHaveAttribute(
      "src",
      "asset://localhost/%2Fproj%2F.codefactory%2Fattachments%2Fimage.png",
    );
  });

  it("renders image attachments inside user message bubbles instead of raw markdown links", () => {
    const { container } = render(
      <MessageList
        messages={[
          baseMsg({
            role: "user",
            content: "请看这张图：\n\n![image.png](file:///proj/.codefactory/attachments/user-image.png)",
          }),
        ]}
        streaming={false}
        cwd={null}
      />,
    );

    const image = container.querySelector("img[alt='image.png']");
    expect(image, "expected the user attachment to render as an inline preview").toBeTruthy();
    expect(convertFileSrcMock).toHaveBeenCalledWith("/proj/.codefactory/attachments/user-image.png");
    expect(screen.queryByText(/!\[image\.png\]/)).not.toBeInTheDocument();
  });

  it("shows a visible fallback if an attached image preview fails to load", () => {
    render(
      <MessageList
        messages={[baseMsg({ content: "![broken.png](file:///proj/.codefactory/attachments/broken.png)" })]}
        streaming={false}
        cwd={null}
      />,
    );
    const image = screen.getByRole("img", { name: "broken.png" });
    fireEvent.error(image);
    expect(screen.getByText("图片预览失败")).toBeInTheDocument();
    expect(screen.getByText("broken.png")).toBeInTheDocument();
  });

});
