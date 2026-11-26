// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({ invoke }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => `asset://${path}` }));

import { MessageInput } from "./MessageInput";

function renderInput(props: Partial<React.ComponentProps<typeof MessageInput>> = {}) {
  render(
    <MessageInput
      cwd="/synthetic"
      sessionId="m289"
      onSend={vi.fn()}
      onCancel={() => {}}
      streaming={false}
      disabled={false}
      {...props}
    />,
  );
}

function drop(files: File[]) {
  fireEvent.dragEnter(window, { dataTransfer: { types: ["Files"], files } });
  fireEvent.drop(window, { dataTransfer: { types: ["Files"], files } });
}

describe("M289 whole-window file drop", () => {
  beforeEach(() => {
    invoke.mockReset().mockImplementation(async (command, args) => {
      if (command === "attachment_limits") return { max_bytes: 50 * 1024 * 1024, max_mebibytes: 50 };
      if (command === "save_session_attachment") {
        return { path: `/synthetic/${args.filename}`, name: args.filename, size_bytes: args.dataBase64.length };
      }
      return [];
    });
  });

  it("shows a whole-window drop target and saves every dropped file through the shared route", async () => {
    renderInput();
    const files = [new File(["image"], "one.png", { type: "image/png" }), new File(["notes"], "two.txt", { type: "text/plain" })];

    fireEvent.dragEnter(window, { dataTransfer: { types: ["Files"], files } });
    expect(screen.getByRole("status")).toHaveTextContent("松开以添加 2 个文件");
    fireEvent.drop(window, { dataTransfer: { types: ["Files"], files } });

    await screen.findByRole("img", { name: "one.png" });
    await screen.findByText("two.txt");
    expect(invoke.mock.calls.filter(([command]) => command === "save_session_attachment")).toHaveLength(2);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("does not intercept internal text dragging", () => {
    renderInput();
    fireEvent.dragEnter(window, { dataTransfer: { types: ["text/plain"], files: [] } });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("explains why a drop cannot be accepted when the composer is unavailable", async () => {
    renderInput({ disabled: true });
    drop([new File(["notes"], "blocked.txt", { type: "text/plain" })]);
    expect(await screen.findByRole("alert")).toHaveTextContent("当前输入框不可用");
    expect(invoke.mock.calls.some(([command]) => command === "save_session_attachment")).toBe(false);
  });

  it("keeps same-name files as separate attachments", async () => {
    renderInput();
    drop([new File(["first"], "same.txt"), new File(["second"], "same.txt")]);
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "save_session_attachment")).toHaveLength(2));
    expect(await screen.findAllByText("same.txt")).toHaveLength(2);
  });

  it("rejects folders with an explicit explanation", async () => {
    renderInput();
    const folder = new File([], "source-folder");
    const item = { kind: "file", getAsFile: () => folder, webkitGetAsEntry: () => ({ isDirectory: true }) };
    fireEvent.dragEnter(window, { dataTransfer: { types: ["Files"], files: [folder], items: [item] } });
    fireEvent.drop(window, { dataTransfer: { types: ["Files"], files: [folder], items: [item] } });
    expect(await screen.findByRole("alert")).toHaveTextContent("source-folder 是文件夹，暂不支持添加");
    expect(invoke.mock.calls.some(([command]) => command === "save_session_attachment")).toBe(false);
  });

  it("accepts exactly 50 MiB and rejects one byte more with the backend-provided limit", async () => {
    renderInput();
    const edge = new File(["x"], "edge.bin");
    const tooLarge = new File(["x"], "large.bin");
    Object.defineProperty(edge, "size", { value: 50 * 1024 * 1024 });
    Object.defineProperty(tooLarge, "size", { value: 50 * 1024 * 1024 + 1 });

    drop([edge, tooLarge]);

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_session_attachment", expect.objectContaining({ filename: "edge.bin" })));
    expect(await screen.findByRole("alert")).toHaveTextContent("large.bin 超过 50MiB 上限");
    expect(invoke).not.toHaveBeenCalledWith("save_session_attachment", expect.objectContaining({ filename: "large.bin" }));
  });
});
