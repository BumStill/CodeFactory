// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { QueueBadge } from "./QueueBadge";

describe("QueueBadge", () => {
  it("behaves as an accessible composer disclosure", () => {
    const onRemove = vi.fn();
    render(
      <QueueBadge
        queue={[
          { id: "q1", content: "先完成当前修复，再补回归测试", enqueuedAt: 1 },
        ]}
        onRemove={onRemove}
      />,
    );

    const trigger = screen.getByRole("button", { name: "查看 1 条待发消息" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(screen.getByRole("button", { name: "移除第 1 条待发消息" }));
    expect(onRemove).toHaveBeenCalledWith("q1");
  });
});
