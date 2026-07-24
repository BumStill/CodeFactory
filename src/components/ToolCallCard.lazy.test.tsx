// SPDX-License-Identifier: Apache-2.0

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

const diffMocks = vi.hoisted(() => ({
  parse: vi.fn(() => ({ summary: "", files: [] })),
}));

vi.mock("./DiffViewer", () => ({
  DiffViewer: () => <div data-testid="diff-viewer" />,
  parseUnifiedDiffResult: diffMocks.parse,
}));

vi.mock("../stores/chat", () => ({
  useChatStore: (selector?: (state: { activeSession: null }) => unknown) =>
    selector ? selector({ activeSession: null }) : { activeSession: null },
}));

import { ToolCallCard } from "./ToolCallCard";

describe("ToolCallCard lazy result parsing", () => {
  beforeEach(() => {
    diffMocks.parse.mockClear();
  });

  it("does not parse a large result until the collapsed card is expanded", () => {
    const result = [
      "--- a/src/a.ts",
      "+++ b/src/a.ts",
      "@@ -1 +1 @@",
      ...Array.from({ length: 1200 }, (_, index) => `+line ${index}`),
    ].join("\n");
    const tc = {
      id: "large-diff",
      name: "bash",
      args: JSON.stringify({ command: "git diff" }),
      result,
      status: "done" as const,
    };

    const { rerender } = render(<ToolCallCard tc={tc} />);
    expect(diffMocks.parse).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /命令.*git diff/ }));
    expect(diffMocks.parse).toHaveBeenCalledTimes(1);

    rerender(<ToolCallCard tc={tc} />);
    expect(diffMocks.parse).toHaveBeenCalledTimes(1);
  });

  it("renders a bounded collapsed error summary for a multi-megabyte result", () => {
    const firstLine = `fatal: ${"x".repeat(3 * 1024 * 1024)}`;
    const tc = {
      id: "large-error",
      name: "bash",
      args: JSON.stringify({ command: "failing-command" }),
      result: `${firstLine}\nsecond line must stay collapsed`,
      status: "error" as const,
    };

    render(<ToolCallCard tc={tc} />);

    const summary = screen.getByText(/^fatal: x+…$/);
    expect(summary.textContent).toHaveLength(201);
    expect(screen.queryByText("second line must stay collapsed")).toBeNull();
    expect(diffMocks.parse).not.toHaveBeenCalled();
  });
});
