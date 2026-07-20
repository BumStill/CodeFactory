// SPDX-License-Identifier: Apache-2.0
//
// A failed tool call must explain itself without a click. 2026-07-20 field
// report: a wall of collapsed tool cards, several red, zero visible reason —
// "没有任何提示,也不知道咋了". The collapsed card shows the first line of
// the error so the transcript reads as a story, not a mystery.

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ToolCallCard } from "./ToolCallCard";
import type { ToolCallState } from "../stores/chatEvents";

const base: ToolCallState = {
  id: "tc-1",
  name: "bash",
  args: JSON.stringify({ command: "gh pr checks 126 --watch" }),
  status: "done",
};

describe("ToolCallCard error visibility", () => {
  it("shows the first line of the error on the collapsed card", () => {
    render(
      <ToolCallCard
        tc={{
          ...base,
          isError: true,
          result: "SIGNAL: command timed out after 120000ms\ngh pr checks exited 8\nmore detail",
        }}
      />,
    );
    expect(screen.getByText(/command timed out after 120000ms/)).toBeTruthy();
    // Only the summary line — the rest stays behind the expand toggle.
    expect(screen.queryByText(/more detail/)).toBeNull();
  });

  it("shows no error summary on successful cards", () => {
    const { container } = render(
      <ToolCallCard tc={{ ...base, result: "all good\nextra output" }} />,
    );
    expect(container.textContent).not.toContain("all good");
  });
});
