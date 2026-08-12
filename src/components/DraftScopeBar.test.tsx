// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { DraftScopeBar } from "./DraftScopeBar";

const projects = [
  { cwd: "/Users/leo/Projects/CodeFactory", name: "CodeFactory", sessions: [], updatedAt: 2 },
  { cwd: "/Users/leo/Projects/AI foundation", name: "AI foundation", sessions: [], updatedAt: 1 },
];

describe("DraftScopeBar project picker", () => {
  it("keeps the default draft quiet and exposes anonymous mode from More", () => {
    const onToggleAnonymous = vi.fn();
    render(
      <DraftScopeBar
        cwd={null}
        anonymous={false}
        projects={projects}
        modelPicker={<button type="button" aria-label="选择下一回合模型">gpt-5.6-sol</button>}
        onPickProject={() => {}}
        onToggleAnonymous={onToggleAnonymous}
      />,
    );

    const scope = screen.getByRole("button", { name: "选择项目：独立任务" });
    expect(scope).toHaveTextContent("独立任务");
    expect(scope).toHaveClass("min-h-[44px]", "lg:min-h-[36px]");
    expect(screen.getByRole("button", { name: "选择下一回合模型" })).toBeInTheDocument();
    expect(screen.queryByText("新会话")).not.toBeInTheDocument();
    expect(screen.queryByText("没选项目，不会碰任何代码")).not.toBeInTheDocument();
    expect(screen.queryByText("匿名")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "更多选项" }));
    const anonymous = screen.getByRole("switch", { name: "匿名会话" });
    expect(anonymous).toHaveAttribute("aria-checked", "false");
    fireEvent.click(anonymous);
    expect(onToggleAnonymous).toHaveBeenCalledWith(true);
  });

  it("promotes anonymous mode to persistent non-color status copy", () => {
    render(
      <DraftScopeBar
        cwd={null}
        anonymous
        projects={projects}
        onPickProject={() => {}}
        onToggleAnonymous={() => {}}
      />,
    );

    expect(screen.getByText("匿名")).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "匿名会话已开启" })).toBeInTheDocument();
    expect(screen.queryByText("聊完不留记录")).not.toBeInTheDocument();
  });

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

    fireEvent.click(screen.getByRole("button", { name: "选择项目：独立任务" }));

    const menu = screen.getByRole("menu", { name: "项目选择" });
    expect(menu.parentElement).toBe(document.body);
    expect(menu).toHaveClass("fixed");

    fireEvent.click(within(menu).getByTitle("/Users/leo/Projects/CodeFactory"));
    expect(onPickProject).toHaveBeenCalledWith("/Users/leo/Projects/CodeFactory");
  });

  it("positions the measured project menu above the whole composer card", () => {
    const rect = (left: number, top: number, width: number, height: number) => ({
      left,
      top,
      width,
      height,
      right: left + width,
      bottom: top + height,
      x: left,
      y: top,
      toJSON: () => ({}),
    }) as DOMRect;
    const geometry = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.getAttribute("data-testid") === "message-input-control-row") return rect(8, 500, 359, 116);
      if (this.getAttribute("role") === "menu") return rect(12, 0, 256, 240);
      if (this.getAttribute("aria-label")?.startsWith("选择项目：")) return rect(16, 560, 88, 44);
      return rect(0, 0, 0, 0);
    });

    render(
      <div data-testid="message-input-control-row">
        <DraftScopeBar
          cwd={null}
          anonymous={false}
          projects={projects}
          onPickProject={() => {}}
          onToggleAnonymous={() => {}}
        />
      </div>,
    );
    fireEvent.click(screen.getByRole("button", { name: "选择项目：独立任务" }));

    expect(screen.getByRole("menu", { name: "项目选择" })).toHaveStyle({ top: "256px" });
    geometry.mockRestore();
  });

  it("moves focus through the project menu and restores it after selection", async () => {
    const user = userEvent.setup();
    const onPickProject = vi.fn();
    render(
      <DraftScopeBar
        cwd={null}
        anonymous={false}
        projects={projects}
        onPickProject={onPickProject}
        onToggleAnonymous={() => {}}
      />,
    );

    const trigger = screen.getByRole("button", { name: "选择项目：独立任务" });
    await user.click(trigger);
    const menu = screen.getByRole("menu", { name: "项目选择" });
    const items = [
      ...within(menu).getAllByRole("menuitemradio"),
      ...within(menu).getAllByRole("menuitem"),
    ];
    expect(items[0]).toHaveFocus();
    expect(items[0]).toHaveAttribute("aria-checked", "true");
    expect(items[0]).toHaveClass("min-h-[44px]", "lg:min-h-[36px]");

    await user.keyboard("{ArrowDown}{Enter}");
    expect(onPickProject).toHaveBeenCalledWith("/Users/leo/Projects/CodeFactory");
    expect(trigger).toHaveFocus();
  });

  it("opens More as a keyboard-contained dialog and returns focus after enabling anonymous", async () => {
    const user = userEvent.setup();
    const onToggleAnonymous = vi.fn();
    render(
      <DraftScopeBar
        cwd={null}
        anonymous={false}
        projects={projects}
        onPickProject={() => {}}
        onToggleAnonymous={onToggleAnonymous}
      />,
    );

    const trigger = screen.getByRole("button", { name: "更多选项" });
    await user.click(trigger);
    const anonymous = screen.getByRole("switch", { name: "匿名会话" });
    expect(anonymous).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onToggleAnonymous).toHaveBeenCalledWith(true);
    expect(trigger).toHaveFocus();
  });
});
