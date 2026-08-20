// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  state: {
    phase: {
      kind: "waiting_for_safe_restart",
      update: { version: "1.81.13" },
      blockers: {
        active_objective_leases: 1,
        objective_blocker_owners: ["objective-supervisor:provider"],
      },
      safetyCheckError: null,
      checkedAt: 1,
    },
    dismissedVersion: null,
    initialize: vi.fn(),
    install: vi.fn(),
    dismiss: vi.fn(),
  },
}));

vi.mock("../stores/updater", () => ({
  countUpdateBlockers: (status: { active_objective_leases?: number } | null) =>
    status?.active_objective_leases ?? 0,
  describeUpdateObjectiveBlockers: (status: { active_objective_leases?: number } | null) =>
    status?.active_objective_leases
      ? "1 个目标仍由 objective-supervisor:provider 持有"
      : null,
  useUpdaterStore: <T,>(selector: (state: typeof mocks.state) => T): T =>
    selector(mocks.state),
}));

import { UpdaterBanner } from "./UpdaterBanner";

describe("UpdaterBanner", () => {
  it("describes a queued update truthfully before backend download begins", () => {
    render(<UpdaterBanner />);

    expect(screen.getByText("更新已排队，正在等待安全安装…")).toBeInTheDocument();
    expect(
      screen.getByText("当前 1 项本地执行仍在运行；结束后会自动下载、安装并重启。"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/更新已下载/)).not.toBeInTheDocument();
  });

  it("does not claim that zero local executions are still running", () => {
    mocks.state.phase = {
      kind: "waiting_for_safe_restart",
      update: { version: "1.81.18" },
      blockers: {
        active_objective_leases: 0,
        objective_blocker_owners: [],
        update_install_state: "queued",
      },
      safetyCheckError: null,
      checkedAt: 2,
    } as typeof mocks.state.phase;

    render(<UpdaterBanner />);

    expect(screen.getByText("安全检查已通过，正在启动下载…")).toBeInTheDocument();
    expect(screen.queryByText(/当前 0 项本地执行仍在运行/)).not.toBeInTheDocument();
  });
});
