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

vi.mock("../stores/git", () => ({ useGitStore: () => state }));
import { GitStatusBar } from "./GitStatusBar";

describe("GitStatusBar local-worktree summary", () => {
  beforeEach(() => vi.clearAllMocks());

  it("answers branch, working-tree changes, and upstream sync in one readable control", async () => {
    const onOpen = vi.fn();
    render(<GitStatusBar cwd="/repo" onOpenChanges={onOpen} />);

    const summary = screen.getByRole("button", { name: "本地工作树" });
    expect(summary).toHaveTextContent("feat/delivery-status");
    expect(summary).toHaveTextContent("13 个本地变更");
    expect(summary).toHaveTextContent("落后 1");
    expect(screen.queryByText("刚刚")).not.toBeInTheDocument();
    expect(screen.queryByTitle("显示提交历史")).not.toBeInTheDocument();
    expect(screen.queryByTitle("远程仓库（问题与拉取请求）")).not.toBeInTheDocument();

    await userEvent.click(summary);
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it("uses a calm synced label for a clean branch", () => {
    state.status = { ...state.status, branch: "main", ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [] };
    render(<GitStatusBar cwd="/repo" onOpenChanges={() => {}} />);
    expect(screen.getByRole("button", { name: "本地工作树" })).toHaveTextContent("main · 已同步");
  });
});
