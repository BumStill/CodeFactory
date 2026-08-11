// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DocumentPreview } from "./DocumentPreview";

vi.mock("../lib/tauri", () => ({
  invoke: vi.fn(() => new Promise(() => {})),
}));

describe("DocumentPreview header actions", () => {
  it("keeps every icon action touchable on narrow screens and visibly keyboard focused", async () => {
    const user = userEvent.setup();
    render(
      <DocumentPreview
        tab={{ id: "doc-1", path: "docs/plan.md", title: "plan.md" }}
        cwd="/project"
        onClose={vi.fn()}
      />,
    );

    const actions = [
      screen.getByRole("button", { name: "复制路径 plan.md" }),
      screen.getByRole("button", { name: "系统打开 plan.md" }),
      screen.getByRole("button", { name: "关闭文档 plan.md" }),
    ];

    for (const action of actions) {
      expect(action).toHaveClass("h-11", "w-11", "lg:h-9", "lg:w-9");
      expect(action).toHaveClass(
        "focus:outline-none",
        "focus-visible:ring-2",
        "focus-visible:ring-accent/60",
      );
    }

    for (const action of actions) {
      await user.tab();
      expect(action).toHaveFocus();
    }
  });
});
