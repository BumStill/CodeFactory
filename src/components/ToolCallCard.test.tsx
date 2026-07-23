// SPDX-License-Identifier: Apache-2.0
//
// The "打开文件" affordance: once a file-writing tool call succeeds, the card
// offers a clickable link that opens the produced file with the OS default
// app — the path (often relative) is resolved against the session cwd.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

const fakeChatState = { activeSession: { cwd: "/proj" } };

vi.mock("../stores/chat", () => ({
  useChatStore: (selector?: (s: typeof fakeChatState) => unknown) =>
    selector ? selector(fakeChatState) : fakeChatState,
}));
vi.mock("../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

import { ToolCallCard } from "./ToolCallCard";
import type { ToolCallState } from "../stores/chatEvents";

const tc = (over: Partial<ToolCallState>): ToolCallState => ({
  id: "t",
  name: "write_pptx",
  args: JSON.stringify({ path: "decks/q3.pptx" }),
  result: "saved",
  isError: false,
  status: "done",
  ...over,
});

describe("ToolCallCard — open generated file", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.invoke.mockResolvedValue(undefined);
  });

  it("offers to open a file a write tool just produced (cwd-resolved)", () => {
    render(<ToolCallCard tc={tc({})} />);
    expect(screen.getByText("q3.pptx")).toBeInTheDocument();
    const open = screen.getByTitle("打开 decks/q3.pptx");
    fireEvent.click(open);
    expect(mocks.invoke).toHaveBeenCalledWith("plugin:shell|open", {
      path: "/proj/decks/q3.pptx",
    });
  });

  it("passes an already-absolute path through unchanged", () => {
    render(<ToolCallCard tc={tc({ args: JSON.stringify({ path: "/tmp/out.docx" }), name: "write_docx" })} />);
    fireEvent.click(screen.getByTitle("打开 /tmp/out.docx"));
    expect(mocks.invoke).toHaveBeenCalledWith("plugin:shell|open", { path: "/tmp/out.docx" });
  });

  it("does not offer open for a non-writing tool", () => {
    render(<ToolCallCard tc={tc({ name: "read_file", args: JSON.stringify({ path: "a.txt" }) })} />);
    expect(screen.queryByText("打开文件")).not.toBeInTheDocument();
  });

  it("does not offer open until the write has finished without error", () => {
    render(<ToolCallCard tc={tc({ status: "running" })} />);
    expect(screen.queryByText("打开文件")).not.toBeInTheDocument();
    render(<ToolCallCard tc={tc({ isError: true, status: "error" })} />);
    expect(screen.queryByText("打开文件")).not.toBeInTheDocument();
  });
  it("renders a successful command as a compact, low-emphasis activity row", () => {
    const { container } = render(<ToolCallCard tc={tc({ name: "bash", args: JSON.stringify({ command: "npm test" }) })} />);
    const row = screen.getByRole("button", { name: /命令.*npm test/ });
    expect(row).toHaveAttribute("data-density", "compact");
    expect(row).toHaveClass("min-h-7");
    expect(container.firstElementChild).not.toHaveClass("bg-surface-2");
  });

});
