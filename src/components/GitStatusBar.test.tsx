// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const state = vi.hoisted(() => ({
  status: {
    branch: "feat/delivery-status",
    upstream: "origin/feat/delivery-status",
    ahead: 0,
    behind: 1,
    staged: [{ path: "a.ts", status: "modified" }],
    unstaged: Array.from({ length: 10 }, (_, i) => ({ path: `u-${i}.ts`, status: "modified" })),
    untracked: ["new-a", "new-b"],
    is_repo: true,
  },
  branches: [],
  refreshing: false,
  lastRefresh: Date.now(),
  setCwd: vi.fn(),
  refreshStatus: vi.fn().mockResolvedValue(undefined),
  refreshBranches: vi.fn().mockResolvedValue(undefined),
  checkout: vi.fn().mockResolvedValue(undefined),
}));

const dirtyBehindStatus = {
  branch: "feat/delivery-status",
  upstream: "origin/feat/delivery-status",
  ahead: 0,
  behind: 1,
  staged: [{ path: "a.ts", status: "modified" }],
  unstaged: Array.from({ length: 10 }, (_, i) => ({ path: `u-${i}.ts`, status: "modified" })),
  untracked: ["new-a", "new-b"],
  is_repo: true,
};

vi.mock("../stores/git", () => ({ useGitStore: () => state }));
import { GitStatusBar } from "./GitStatusBar";

describe("GitStatusBar local-worktree summary", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    state.status = { ...dirtyBehindStatus };
  });

  it("keeps a dirty or behind state short on screen while exposing the complete status to assistive technology", async () => {
    const onOpen = vi.fn();
    render(<GitStatusBar cwd="/repo" onOpenChanges={onOpen} />);

    const summary = screen.getByRole("button", {
      name: /本地 Git.*feat\/delivery-status.*13.*落后 1/,
    });
    expect(summary).not.toHaveTextContent("feat/delivery-status");
    expect(summary).not.toHaveTextContent("个本地变更");
    expect(summary).toHaveTextContent("13");
    expect(summary).toHaveTextContent("落后 1");
    expect(summary).toHaveAttribute("data-status-tone", "warning");
    expect(summary).toHaveClass("h-11", "lg:h-9");
    expect(screen.queryByText("刚刚")).not.toBeInTheDocument();
    expect(screen.queryByTitle("显示提交历史")).not.toBeInTheDocument();
    expect(screen.queryByTitle("远程仓库（问题与拉取请求）")).not.toBeInTheDocument();

    await userEvent.click(summary);
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it("renders a clean synced branch as an icon-first control without spending header width on normal-state text", () => {
    state.status = { ...state.status, branch: "main", ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [] };
    render(<GitStatusBar cwd="/repo" onOpenChanges={() => {}} />);
    const summary = screen.getByRole("button", {
      name: /本地 Git.*main.*已同步.*无本地变更/,
    });
    expect(summary).not.toHaveTextContent("main");
    expect(summary).not.toHaveTextContent("已同步");
    expect(summary).toHaveAttribute("data-status-tone", "neutral");
  });
});
