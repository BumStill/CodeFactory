// SPDX-License-Identifier: Apache-2.0
//
// M36: the collapsed edit row must answer "how many lines did this change"
// with "+X −Y", not with the character count of the arguments.

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ToolCallCard } from "./ToolCallCard";
import type { ToolCallState } from "../stores/chatEvents";

function row(overrides: Partial<ToolCallState>): ToolCallState {
  return {
    id: "tc-1",
    name: "edit_file",
    args: "{}",
    status: "done",
    ...overrides,
  };
}

describe("ToolCallCard line stats", () => {
  it("renders +X −Y for an edit instead of a character count", () => {
    const { container } = render(
      <ToolCallCard
        tc={row({
          args: JSON.stringify({
            path: "src-tauri/src/session_title.rs",
            old_string: "fn a() {}\n// old line",
            new_string: "fn a() {}\n// new line",
          }),
        })}
      />,
    );
    const stats = screen.getByTestId("line-change-stats");
    expect(stats.getAttribute("data-added")).toBe("1");
    expect(stats.getAttribute("data-removed")).toBe("1");
    expect(container.textContent).toContain("+1");
    expect(container.textContent).toContain("−1");
    // The old "985b → 4133b" summary is gone.
    expect(container.textContent).not.toContain("b →");
    expect(container.textContent).not.toMatch(/\db\b/);
  });

  it("still shows the edited path", () => {
    render(
      <ToolCallCard
        tc={row({ args: JSON.stringify({ path: "src/lib/a.ts", old_string: "x", new_string: "y" }) })}
      />,
    );
    expect(screen.getByText(/src\/lib\/a\.ts/)).toBeTruthy();
  });

  it("shows +N for a write that created a new file", () => {
    render(
      <ToolCallCard
        tc={row({
          name: "write_file",
          args: JSON.stringify({ path: "docs/new.md", content: "one\ntwo\nthree\n" }),
          result: "Written 14 bytes to docs/new.md\n\n```diff\n--- a/docs/new.md\n+++ b/docs/new.md\n@@ -0,0 +1,3 @@\n+one\n+two\n+three\n```",
        })}
      />,
    );
    const stats = screen.getByTestId("line-change-stats");
    expect(stats.getAttribute("data-added")).toBe("3");
    expect(stats.getAttribute("data-new-file")).toBe("true");
    expect(screen.getByTestId("line-change-stats").textContent).toBe("+3");
  });

  it("shows only a line count for an overwrite, never an invented deletion", () => {
    const { container } = render(
      <ToolCallCard
        tc={row({
          name: "write_file",
          args: JSON.stringify({ path: "docs/existing.md", content: "one\ntwo\n" }),
          result: "Written 8 bytes to docs/existing.md\n\n```diff\n--- a/docs/existing.md\n+++ b/docs/existing.md\n@@ -1,2 +1,2 @@\n-two\n+one\n+two\n```",
        })}
      />,
    );
    expect(screen.getByTestId("written-line-count").textContent).toBe("写入 2 行");
    expect(container.textContent).not.toContain("−2");
    expect(screen.queryByTestId("line-change-stats")).toBeNull();
  });

  it("leaves read-only rows without line stats", () => {
    render(<ToolCallCard tc={row({ name: "read_file", args: JSON.stringify({ path: "src/a.ts" }) })} />);
    expect(screen.queryByTestId("line-change-stats")).toBeNull();
    expect(screen.queryByTestId("written-line-count")).toBeNull();
  });

  it("shows nothing for an edit that changed no lines", () => {
    render(
      <ToolCallCard
        tc={row({ args: JSON.stringify({ path: "src/a.ts", old_string: "same", new_string: "same" }) })}
      />,
    );
    expect(screen.queryByTestId("line-change-stats")).toBeNull();
  });

  // Found by the real-browser gate: a denied edit was rendering "+0 −4" as if
  // it had removed four lines. A call that never landed changed nothing.
  it("shows no line stats for a denied or failed edit", () => {
    const { container } = render(
      <ToolCallCard
        tc={row({
          status: "denied",
          isError: true,
          result: "用户拒绝了这次编辑",
          args: JSON.stringify({ path: "src/a.ts", old_string: "a\nb\nc\nd\ne", new_string: "a" }),
        })}
      />,
    );
    expect(screen.queryByTestId("line-change-stats")).toBeNull();
    expect(container.textContent).not.toContain("−4");
    expect(screen.getByRole("button", { name: /编辑 · src\/a\.ts$/ })).toBeTruthy();
  });

  it("names the stats in the accessible label", () => {
    render(
      <ToolCallCard
        tc={row({
          args: JSON.stringify({ path: "src/a.ts", old_string: "a\nb", new_string: "a\nb\nc\nd" }),
        })}
      />,
    );
    expect(screen.getByRole("button", { name: /编辑 · src\/a\.ts · \+2 −0/ })).toBeTruthy();
  });
});
