// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { DraftScopeBar } from "./DraftScopeBar";

const projects = [
  { cwd: "/Users/leo/Projects/CodeFactory", name: "CodeFactory", sessions: [], updatedAt: 2 },
  { cwd: "/Users/leo/Projects/AI foundation", name: "AI foundation", sessions: [], updatedAt: 1 },
];

describe("DraftScopeBar project picker", () => {
  it("renders the project menu outside the clipped composer surface and still selects a project", () => {
    const onPickProject = vi.fn();
    render(
      <div data-testid="clipped-composer" className="overflow-hidden rounded-2xl">
        <DraftScopeBar
          cwd={null}
          anonymous={false}
          projects={projects}
          onPickProject={onPickProject}
          onToggleAnonymous={() => {}}
        />
      </div>,
    );

    fireEvent.click(screen.getByRole("button", { name: "选择项目" }));

    const menu = screen.getByRole("menu", { name: "项目选择" });
    expect(menu.parentElement).toBe(document.body);
    expect(menu).toHaveClass("fixed");

    fireEvent.click(within(menu).getByTitle("/Users/leo/Projects/CodeFactory"));
    expect(onPickProject).toHaveBeenCalledWith("/Users/leo/Projects/CodeFactory");
  });
});
