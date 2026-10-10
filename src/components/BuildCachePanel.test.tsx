// SPDX-License-Identifier: Apache-2.0
// CF-BLD-R4: the occupancy panel shows the real numbers and its one action runs
// the same command the background entry does.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import BuildCachePanel, {
  buildCacheSummary,
  cleanupSummary,
  formatGiB,
  heavyBuildLine,
} from "./BuildCachePanel";

const invokeMock = vi.fn();
vi.mock("../lib/tauri", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const report = {
  total_bytes: 12 * 1024 ** 3,
  budget_bytes: 60 * 1024 ** 3,
  entries: [
    {
      path: "/tmp/ws/a/.codefactory-cache/cargo-target-test",
      bytes: 5 * 1024 ** 3,
      files: 10,
      last_used_unix: 1_700_000_000,
      in_use: false,
      owner: "a",
    },
    {
      path: "/tmp/ws/b/.codefactory-cache/cargo-target-test",
      bytes: 7 * 1024 ** 3,
      files: 12,
      last_used_unix: 1_700_000_100,
      in_use: true,
      owner: "b",
    },
  ],
  heavy_builds_running: 1,
  heavy_builds_waiting: 0,
  heavy_build_limit: 2,
  heavy_build_status: null,
};

describe("BuildCachePanel", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("shows the measured occupancy and the budget, not a placeholder", async () => {
    invokeMock.mockResolvedValueOnce(report);

    render(<BuildCachePanel />);

    expect(await screen.findByText("编译缓存占用 12 GB，上限 60 GB")).toBeTruthy();
    expect(screen.getByText("正在编译 1/2。")).toBeTruthy();
    expect(screen.getByText("5.0 GB")).toBeTruthy();
    expect(screen.getByText("7.0 GB（构建中）")).toBeTruthy();
    expect(invokeMock).toHaveBeenCalledWith("build_cache_report");
  });

  it("reclaims on one click and reports what was actually freed", async () => {
    invokeMock.mockResolvedValueOnce(report).mockResolvedValueOnce({
      scanned: 2,
      before_bytes: 12 * 1024 ** 3,
      after_bytes: 7 * 1024 ** 3,
      reclaimed_bytes: 5 * 1024 ** 3,
      evicted_bytes: 0,
      protected_bytes: 7 * 1024 ** 3,
      overflow_bytes: 0,
      removed: ["/tmp/ws/a/.codefactory-cache/cargo-target-test"],
    });
    invokeMock.mockResolvedValueOnce({ ...report, total_bytes: 7 * 1024 ** 3, entries: [report.entries[1]] });

    render(<BuildCachePanel />);
    await screen.findByText("编译缓存占用 12 GB，上限 60 GB");

    await userEvent.click(screen.getByTestId("build-cache-cleanup"));

    await waitFor(() =>
      expect(screen.getByTestId("build-cache-message").textContent).toBe(
        "已清理 5.0 GB，现在占用 7.0 GB。",
      ),
    );
    expect(invokeMock).toHaveBeenCalledWith("build_cache_cleanup");
    expect(await screen.findByText("编译缓存占用 7.0 GB，上限 60 GB")).toBeTruthy();
  });

  it("never claims a cleanup it did not perform", () => {
    expect(
      cleanupSummary({
        scanned: 0,
        before_bytes: 0,
        after_bytes: 0,
        reclaimed_bytes: 0,
        evicted_bytes: 0,
        protected_bytes: 0,
        overflow_bytes: 0,
        removed: [],
      }),
    ).toBe("没有可以清理的编译缓存，当前占用 0.0 GB。");
  });

  it("mentions an in-flight build instead of pretending the cache is idle", () => {
    expect(heavyBuildLine({ ...report, heavy_build_status: "等待编译空位（前面还有 2 个构建）" })).toBe(
      "等待编译空位（前面还有 2 个构建）",
    );
    expect(heavyBuildLine({ ...report, heavy_builds_running: 0 })).toBeNull();
  });

  it("formats sizes the way the budget is described", () => {
    expect(formatGiB(12 * 1024 ** 3)).toBe("12 GB");
    expect(formatGiB(512 * 1024 ** 2)).toBe("0.5 GB");
    expect(buildCacheSummary(report)).toBe("编译缓存占用 12 GB，上限 60 GB");
  });
});
