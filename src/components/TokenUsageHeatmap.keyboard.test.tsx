// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TokenUsageHeatmap, type UsageHeatmapDay } from "./TokenUsageHeatmap";

const days: UsageHeatmapDay[] = Array.from({ length: 14 }, (_, index) => ({
  local_date: `2026-07-${String(index + 1).padStart(2, "0")}`,
  status: "recorded",
  total_tokens: (index + 1) * 1_000,
  requests: index + 1,
}));

describe("TokenUsageHeatmap keyboard contract", () => {
  it("uses one roving tab stop and supports Arrow, Home, End, Space, and Escape", async () => {
    const onSelectDate = vi.fn();
    const user = userEvent.setup();
    render(
      <TokenUsageHeatmap
        days={days}
        ariaLabel="测试 Token 地图"
        selectedDate={null}
        onSelectDate={onSelectDate}
      />,
    );

    const cells = screen.getAllByRole("gridcell");
    expect(cells.filter((cell) => cell.tabIndex === 0)).toHaveLength(1);
    expect(cells[0]).toHaveAttribute("tabindex", "0");
    expect(cells.slice(1).every((cell) => cell.tabIndex === -1)).toBe(true);

    await user.tab();
    expect(cells[0]).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(cells[1]).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(cells[8]).toHaveFocus();
    await user.keyboard("{Home}");
    expect(cells[7]).toHaveFocus();
    await user.keyboard("{End}");
    expect(cells[13]).toHaveFocus();

    await user.keyboard("[Space]");
    expect(onSelectDate).toHaveBeenCalledWith("2026-07-14");
    await user.keyboard("{Escape}");
    expect(onSelectDate).toHaveBeenLastCalledWith(null);
    expect(cells[13]).toHaveFocus();
  });

  it("does not put passive compact-map cells into the tab order", () => {
    render(<TokenUsageHeatmap days={days} ariaLabel="只读 Token 地图" compact />);
    expect(screen.getAllByRole("gridcell").every((cell) => cell.tabIndex === -1)).toBe(true);
  });
});
