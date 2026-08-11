// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  state: {
    phase: { kind: "error", message: "network unavailable", checkedAt: 1 },
    currentVersion: "1.79.0",
    install: vi.fn(),
    checkNow: vi.fn(),
  },
}));

vi.mock("../stores/updater", () => ({
  countUpdateBlockers: vi.fn(),
  useUpdaterStore: <T,>(selector: (state: typeof mocks.state) => T): T => selector(mocks.state),
}));

import { UpdateStatusPill } from "./UpdateStatusPill";

describe("UpdateStatusPill", () => {
  it("describes automatic polling after a failed check without a retry CTA", () => {
    render(<UpdateStatusPill />);

    const pill = screen.getByRole("button", { name: "v1.79.0" });
    expect(pill).toHaveAttribute(
      "title",
      "上次检查失败：network unavailable\n系统会按计划自动再次检查；点击仅用于立即检查。",
    );
    expect(pill.getAttribute("title")).not.toMatch(/重试|继续执行|回到对话/);
  });
});
