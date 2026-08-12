// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ComposerControlBar } from "./ComposerControlBar";

describe("ComposerControlBar", () => {
  it("implements toolbar arrow-key navigation without removing Tab reachability", async () => {
    const user = userEvent.setup();
    render(
      <ComposerControlBar shortcutHint="Enter 发送">
        <button type="button">项目</button>
        <button type="button">模型</button>
        <button type="button">权限</button>
      </ComposerControlBar>,
    );

    const toolbar = screen.getByRole("toolbar", { name: "输入工具" });
    const project = screen.getByRole("button", { name: "项目" });
    const model = screen.getByRole("button", { name: "模型" });
    const permission = screen.getByRole("button", { name: "权限" });
    project.focus();
    await user.keyboard("{ArrowRight}");
    expect(model).toHaveFocus();
    await user.keyboard("{End}");
    expect(permission).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(model).toHaveFocus();
    expect(toolbar).toHaveClass("overflow-x-clip");
  });
});
