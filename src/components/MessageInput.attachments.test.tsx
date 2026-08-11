// SPDX-License-Identifier: Apache-2.0
//
// Attachment-handling regression tests for MessageInput.
//
// Per AGENTS.md "UX 行为变更必须实地验证" — these are NOT a substitute
// for live-app verification (jsdom doesn't have a real clipboard or
// drag layer), but they exercise the wire-up that exists today:
//   - paste with a clipboard item triggers save_chat_attachment
//   - missing cwd surfaces an error instead of silently swallowing the paste
//   - chip removal clears state
//   - submit attaches the markdown link to outgoing text
//
// What we explicitly CANNOT verify here:
//   - native paste behavior in the Tauri webview
//   - file:// URL rendering in the message bubble
//   - drag-drop events fired by the OS

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.hoisted(() => vi.fn());
const convertFileSrcMock = vi.hoisted(() => vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`));
vi.mock("../lib/tauri", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: convertFileSrcMock }));

import { MessageInput } from "./MessageInput";

function setup(props: Partial<React.ComponentProps<typeof MessageInput>> = {}) {
  const onSend = vi.fn();
  const onCancel = vi.fn();
  const utils = render(
    <MessageInput
      onSend={onSend}
      onCancel={onCancel}
      streaming={false}
      disabled={false}
      cwd="/proj"
      {...props}
    />,
  );
  return { ...utils, onSend, onCancel };
}

function makeImageFile(): File {
  return new File([new Uint8Array([137, 80, 78, 71])], "screenshot.png", { type: "image/png" });
}

describe("MessageInput attachments", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    convertFileSrcMock.mockClear();
  });

  it("paste of an image file invokes save_chat_attachment and shows an image preview", async () => {
    invokeMock.mockResolvedValue({ path: "/proj/.codefactory/attachments/x.png", name: "x.png", size_bytes: 4 });
    setup();
    const textarea = screen.getByRole("textbox");

    // jsdom's ClipboardEvent doesn't carry items by default; build a fake
    // DataTransfer-like object and pass it through React's paste event.
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [
          { kind: "file", type: "image/png", getAsFile: () => makeImageFile() },
        ],
      },
    });

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      "save_chat_attachment",
      expect.objectContaining({ cwd: "/proj", filename: "screenshot.png" }),
    ));
    // The attachment tray shows a real preview, not just a filename chip.
    const preview = await screen.findByRole("img", { name: "screenshot.png" });
    expect(convertFileSrcMock).toHaveBeenCalledWith("/proj/.codefactory/attachments/x.png");
    expect(preview).toHaveAttribute("src", "asset://localhost/%2Fproj%2F.codefactory%2Fattachments%2Fx.png");
    expect(screen.getByText("screenshot.png")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "移除 screenshot.png" })).toHaveClass(
      "h-11",
      "w-11",
      "lg:h-8",
      "lg:w-8",
    );
  });

  it("paste with no cwd shows an error and does NOT invoke save", () => {
    setup({ cwd: null });
    const textarea = screen.getByRole("textbox");
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [
          { kind: "file", type: "image/png", getAsFile: () => makeImageFile() },
        ],
      },
    });
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.getByText(/打开一个项目/)).toBeInTheDocument();
  });

  it("paste of plain text doesn't trigger save (only images)", () => {
    setup();
    const textarea = screen.getByRole("textbox");
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [{ kind: "string", type: "text/plain", getAsFile: () => null }],
      },
    });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("opens attachment tray image previews in a larger viewer", async () => {
    invokeMock.mockResolvedValue({
      path: "/proj/.codefactory/attachments/preview.png",
      name: "preview.png",
      size_bytes: 4,
    });
    const user = userEvent.setup();
    setup();

    fireEvent.paste(screen.getByRole("textbox"), {
      clipboardData: {
        items: [
          { kind: "file", type: "image/png", getAsFile: () => makeImageFile() },
        ],
      },
    });

    await screen.findByRole("img", { name: "screenshot.png" });
    await user.click(screen.getByRole("button", { name: "放大查看 screenshot.png" }));

    const dialog = screen.getByRole("dialog", { name: "图片预览" });
    expect(within(dialog).getByRole("img", { name: "screenshot.png" })).toHaveAttribute(
      "src",
      "asset://localhost/%2Fproj%2F.codefactory%2Fattachments%2Fpreview.png",
    );
  });


  it("wraps attachment markdown destinations with spaces so previews survive reload", async () => {
    invokeMock.mockResolvedValue({
      path: "/proj/AI foundation/.codefactory/attachments/space path.png",
      name: "space path.png",
      size_bytes: 4,
    });
    const { onSend } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;

    fireEvent.paste(textarea, {
      clipboardData: {
        items: [
          { kind: "file", type: "image/png", getAsFile: () => makeImageFile() },
        ],
      },
    });
    await waitFor(() => expect(screen.queryByText("screenshot.png")).toBeInTheDocument());

    fireEvent.keyDown(textarea, { key: "Enter" });

    expect(onSend).toHaveBeenCalledWith(
      "![screenshot.png](<file:///proj/AI foundation/.codefactory/attachments/space path.png>)",
    );
  });

  it("submit with attachments appends file:// markdown link to outgoing message", async () => {
    invokeMock.mockResolvedValue({
      path: "/proj/.codefactory/attachments/y.png",
      name: "y.png",
      size_bytes: 4,
    });
    const { onSend } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;

    fireEvent.paste(textarea, {
      clipboardData: {
        items: [
          { kind: "file", type: "image/png", getAsFile: () => makeImageFile() },
        ],
      },
    });
    await waitFor(() => expect(screen.queryByText("screenshot.png")).toBeInTheDocument());

    fireEvent.change(textarea, { target: { value: "look at this:" } });
    fireEvent.keyDown(textarea, { key: "Enter" });

    expect(onSend).toHaveBeenCalledTimes(1);
    const arg: string = onSend.mock.calls[0][0];
    expect(arg).toContain("look at this:");
    // Label = original filename; the link still points at the saved on-disk path.
    expect(arg).toMatch(/!\[screenshot\.png\]\(file:\/\/\/proj\/\.codefactory\/attachments\/y\.png\)/);
  });

  it("submit with ONLY an attachment (no text) still sends", async () => {
    invokeMock.mockResolvedValue({
      path: "/proj/.codefactory/attachments/z.png",
      name: "z.png",
      size_bytes: 4,
    });
    const { onSend } = setup();
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;

    fireEvent.paste(textarea, {
      clipboardData: {
        items: [
          { kind: "file", type: "image/png", getAsFile: () => makeImageFile() },
        ],
      },
    });
    await waitFor(() => expect(screen.queryByText("screenshot.png")).toBeInTheDocument());

    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend.mock.calls[0][0]).toMatch(/!\[screenshot\.png\]\(file:\/\//);
  });

});
