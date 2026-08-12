// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  state: {
    phase: { kind: "error", message: "network unavailable", checkedAt: 1 },
    currentVersion: "1.79.0",
    install: vi.fn(),
    checkNow: vi.fn(),
  } as Record<string, unknown>,
  countUpdateBlockers: vi.fn(),
  describeUpdateObjectiveBlockers: vi.fn(),
}));

vi.mock("../stores/updater", () => ({
  countUpdateBlockers: mocks.countUpdateBlockers,
  describeUpdateObjectiveBlockers: mocks.describeUpdateObjectiveBlockers,
  useUpdaterStore: <T,>(selector: (state: typeof mocks.state) => T): T => selector(mocks.state),
}));

import { UpdateStatusPill } from "./UpdateStatusPill";

describe("UpdateStatusPill", () => {
  beforeEach(() => {
    mocks.state = {
      phase: { kind: "error", message: "network unavailable", checkedAt: 1 },
      currentVersion: "1.79.0",
      install: vi.fn(),
      checkNow: vi.fn(),
    };
    mocks.countUpdateBlockers.mockReset();
    mocks.describeUpdateObjectiveBlockers.mockReset();
  });

  it("describes automatic polling after a failed check without a retry CTA", () => {
    render(<UpdateStatusPill />);

    const pill = screen.getByRole("button", { name: "v1.79.0" });
    expect(pill).toHaveAttribute(
      "title",
      "上次检查失败：network unavailable\n系统会按计划自动再次检查；点击仅用于立即检查。",
    );
    expect(pill.getAttribute("title")).not.toMatch(/重试|继续执行|回到对话/);
  });

  it("shows the durable Objective blocker count and recovery owner", () => {
    mocks.state = {
      phase: {
        kind: "waiting_for_safe_restart",
        update: { version: "1.79.1" },
        blockers: {
          nonterminal_objectives: 2,
          objective_blocker_owners: ["objective-supervisor:chat"],
        },
        safetyCheckError: null,
        checkedAt: 1,
      },
      currentVersion: "1.79.0",
      install: vi.fn(),
      checkNow: vi.fn(),
    };
    mocks.countUpdateBlockers.mockReturnValue(2);
    mocks.describeUpdateObjectiveBlockers.mockReturnValue(
      "2 个目标仍由 objective-supervisor:chat 持有",
    );

    render(<UpdateStatusPill />);

    expect(screen.getByText("等待安全更新 · 2")).toHaveAttribute(
      "title",
      expect.stringContaining("2 个目标仍由 objective-supervisor:chat 持有"),
    );
  });

  it("describes an unknown install as observe-only instead of promising replay", () => {
    mocks.state = {
      phase: {
        kind: "waiting_for_safe_restart",
        update: { version: "1.79.1" },
        blockers: { update_install_state: "observe_only" },
        safetyCheckError: null,
        checkedAt: 1,
      },
      currentVersion: "1.79.0",
      install: vi.fn(),
      checkNow: vi.fn(),
    };
    mocks.describeUpdateObjectiveBlockers.mockReturnValue(null);

    render(<UpdateStatusPill />);

    const pill = screen.getByText("正在核对更新结果");
    expect(pill).toHaveAttribute(
      "title",
      expect.stringContaining("系统只读核对，不会重复安装"),
    );
    expect(pill.getAttribute("title")).not.toContain("自动安装");
  });
});
