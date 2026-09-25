// SPDX-License-Identifier: Apache-2.0
//
// The "打开文件" affordance: once a file-writing tool call succeeds, the card
// offers a clickable link that opens the produced file with the OS default
// app — the path (often relative) is resolved against the session cwd.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, fireEvent } from "@testing-library/react";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

const fakeChatState = { activeSession: { id: "session-1", cwd: "/proj" } };

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
import { useAppNavigationStore } from "../stores/appNavigation";

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
    useAppNavigationStore.getState().reset();
  });

  it("rebuilds a successful skill_fetch receipt from persisted structured content", () => {
    render(<ToolCallCard tc={tc({
      id: "tool-skill-fetch",
      name: "skill_fetch",
      args: JSON.stringify({ source: "https://example.com/skill.json" }),
      result: '已获取 1 个技能。\nCODEFACTORY_SKILL_RECEIPT_V1:{"schema_version":1,"kind":"skill_install","items":[{"id":"continuity-helper","name":"Continuity Helper","version":"1.0.0","installed":true,"activation":"disabled"}]}',
    })} />);

    fireEvent.click(screen.getByRole("button", { name: "检查并管理 Continuity Helper v1.0.0" }));
    expect(useAppNavigationStore.getState().skillReview).toEqual({
      skillId: "continuity-helper",
      originSessionId: "session-1",
      originToolCallId: "tool-skill-fetch",
    });
  });

  it("restores focus to the originating receipt after review navigation returns", async () => {
    render(<ToolCallCard tc={tc({
      id: "tool-skill-fetch",
      name: "skill_fetch",
      result: 'CODEFACTORY_SKILL_RECEIPT_V1:{"schema_version":1,"kind":"skill_install","items":[{"id":"continuity-helper","name":"Continuity Helper","version":"1.0.0","installed":true,"activation":"disabled"}]}',
    })} />);

    act(() => useAppNavigationStore.getState().restoreReceiptFocus("tool-skill-fetch"));
    expect(await screen.findByRole("button", { name: "检查并管理 Continuity Helper v1.0.0" })).toHaveFocus();
    expect(useAppNavigationStore.getState().returnFocusToolCallId).toBeNull();
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
    expect(container.firstElementChild).toHaveClass("text-note");
    expect(row).not.toHaveClass("w-full");
    expect(container.firstElementChild).not.toHaveClass("border");
    expect(container.firstElementChild).not.toHaveClass("bg-surface-1/30");
  });

  it.each([
    ["running", "border-l-2", "bg-accent/[0.025]"],
    ["waiting", "border-l-2", "bg-status-info-soft/35"],
    ["waiting_permission", "border-l-2", "bg-status-warning-soft/45"],
    ["error", "border-l", "bg-transparent"],
  ] as const)("uses a quiet left status rail instead of a full frame for %s", (status, rail, tone) => {
    const { container } = render(
      <ToolCallCard
        tc={tc({
          status,
          isError: status === "error",
          result: status === "error" ? "failed first line\nfull detail" : undefined,
        })}
      />,
    );
    expect(container.firstElementChild).toHaveClass(rail, tone);
    expect(container.firstElementChild).not.toHaveClass("border");
  });

  it("keeps collapsed failure detail readable without stretching into a full-width alert card", () => {
    render(
      <ToolCallCard
        tc={tc({
          name: "bash",
          args: JSON.stringify({ command: "false" }),
          status: "error",
          isError: true,
          result: "[shell-audit] cwd=/a/very/long/path exit_code=1 risk=low",
        })}
      />,
    );
    const detail = screen.getByText(/\[shell-audit]/);
    expect(detail).toHaveClass("max-w-[56ch]", "text-note");
    expect(detail).not.toHaveClass("border-t");
  });

  it("surfaces CI progress directly on a running deliver_changes row", async () => {
    mocks.invoke.mockResolvedValue({
      remote_available: true,
      pr: { number: 438 },
      ci_status: "pending",
      error: null,
    });

    render(<ToolCallCard tc={tc({ name: "deliver_changes", status: "running", result: undefined })} />);

    expect(await screen.findByText("CI 运行中")).toBeInTheDocument();
    expect(screen.getByText("PR #438")).toBeInTheDocument();
    expect(mocks.invoke).toHaveBeenCalledWith("workspace_delivery_status", {
      cwd: "/proj",
      sessionId: "session-1",
      branch: null,
      prNumber: null,
    });
  });

});
