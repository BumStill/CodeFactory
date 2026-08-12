// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ComposerControlBar } from "./ComposerControlBar";

describe("ComposerControlBar", () => {
  it("leaves the row's free space to a single claimant", () => {
    // Two `ml-auto` siblings do not both reach the right edge — flexbox splits
    // the free space between them. The session toolbar already claims it for
    // the usage meter, and a second claim on the shortcut hint parked that
    // meter mid-row: while usage was still loading it rendered as a lone
    // spinner floating between the model picker and the hint.
    render(
      <ComposerControlBar shortcutHint="Enter 发送">
        <button type="button">模型</button>
        <div className="ml-auto" data-testid="right-aligned-child">
          <span>用量</span>
        </div>
      </ComposerControlBar>,
    );

    const toolbar = screen.getByRole("toolbar", { name: "输入工具" });
    const claimants = Array.from(toolbar.children).filter((child) =>
      child.className.split(/\s+/).includes("ml-auto"),
    );
    expect(
      claimants.map((c) => c.getAttribute("data-testid") ?? c.tagName),
      "行内只应有一个元素抢占剩余空间；多一个就会跟它平分，把两者都停在半路",
    ).toEqual(["right-aligned-child"]);
  });

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
