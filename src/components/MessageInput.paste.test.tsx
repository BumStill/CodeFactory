// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({ invoke }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => `asset://${path}` }));
import { MessageInput } from "./MessageInput";

function setup() {
  const onSend = vi.fn();
  render(<MessageInput cwd="/synthetic" sessionId="m190" onSend={onSend} onCancel={() => {}} streaming={false} disabled={false} />);
  return onSend;
}
function paste(files: File[], text = "") {
  return fireEvent.paste(screen.getByRole("textbox"), { clipboardData: {
    files, items: files.map(file => ({ kind: "file", type: file.type, getAsFile: () => file })),
    getData: () => text,
  } });
}
describe("M190 clipboard attachments R1-R4", () => {
  beforeEach(() => {
    invoke.mockReset().mockImplementation(async (cmd, args) => cmd === "save_session_attachment" ? { path: `/synthetic/${args.filename}`, name: args.filename, size_bytes: 4 } : []);
  });
  it("R1 handles all pasted images and documents, preserving mixed text default", async () => {
    setup();
    expect(paste([new File(["a"], "one.png", { type: "image/png" }), new File(["b"], "two.jpg", { type: "image/jpeg" }), new File(["c"], "report.pdf", { type: "application/pdf" })], "mixed text")).toBe(true);
    await screen.findByRole("img", { name: "two.jpg" });
    await screen.findByText("report.pdf");
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "save_session_attachment")).toHaveLength(3);
  });
  it("R1 handles clipboardData.files when items is absent", async () => {
    setup();
    fireEvent.paste(screen.getByRole("textbox"), { clipboardData: { files: [new File(["a"], "notes.txt")], getData: () => "" } });
    await screen.findByText("notes.txt");
  });
  it("R2 rejects oversize pasted files through the same size guard and error can be dismissed", async () => {
    setup();
    const huge = new File(["a"], "huge.png", { type: "image/png" });
    Object.defineProperty(huge, "size", { value: 25 * 1024 * 1024 + 1 });
    paste([huge]);
    await screen.findByText(/huge.png 超过 25MB/);
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "移除附件错误" }));
    expect(screen.queryByText(/超过 25MB/)).not.toBeInTheDocument();
  });
  it("R2 assigns a unique timestamp name to nameless clipboard images", async () => {
    setup();
    paste([new File(["a"], "", { type: "image/png" })]);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_session_attachment", expect.objectContaining({ filename: expect.stringMatching(/^粘贴图片-.+\.png$/) })));
  });
  it("R1 uses native fallback for WebView clipboard without DOM files", async () => {
    invoke.mockResolvedValue([{ path: "/synthetic/fallback.pdf", name: "fallback.pdf", size_bytes: 2048 }]);
    setup(); paste([]);
    await screen.findByText("fallback.pdf");
    expect(invoke).toHaveBeenCalledWith("paste_session_attachments", { cwd: "/synthetic", sessionId: "m190" });
  });
  it("R3 ordinary plain text never invokes native clipboard", () => {
    setup(); expect(paste([], "hello")).toBe(true); expect(invoke).not.toHaveBeenCalled();
  });
  it("R4 keeps uploading visible until every concurrent save completes and prevents premature send", async () => {
    const pending: ((value: unknown) => void)[] = [];
    invoke.mockImplementation(() => new Promise(resolve => pending.push(resolve)));
    const onSend = setup(); paste([new File(["a"], "one.png", { type: "image/png" }), new File(["b"], "two.png", { type: "image/png" })]);
    await waitFor(() => expect(pending).toHaveLength(2));
    pending[0]({ path: "/synthetic/one.png", name: "one.png", size_bytes: 1 });
    await screen.findByRole("img", { name: "one.png" });
    expect(screen.getByText("保存中…")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" }); expect(onSend).not.toHaveBeenCalled();
    pending[1]({ path: "/synthetic/two.png", name: "two.png", size_bytes: 1 });
    await waitFor(() => expect(screen.queryByText("保存中…")).not.toBeInTheDocument());
  });
  it("R4 native read failure is visible and removable", async () => {
    invoke.mockRejectedValue(new Error("clipboard unreadable"));
    setup(); paste([]);
    await screen.findByText(/clipboard unreadable/);
    fireEvent.click(screen.getByRole("button", { name: "移除附件错误" }));
    expect(screen.queryByText(/clipboard unreadable/)).not.toBeInTheDocument();
  });
});
