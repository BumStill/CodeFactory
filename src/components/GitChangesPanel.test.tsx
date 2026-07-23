// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  refreshStatus: vi.fn().mockResolvedValue(undefined),
  refreshBranches: vi.fn().mockResolvedValue(undefined),
  checkout: vi.fn().mockResolvedValue(undefined),
  stageFiles: vi.fn(), commit: vi.fn(), getFileDiff: vi.fn(),
}));
vi.mock("../stores/git", () => ({
  useGitStore: () => ({
    status: { branch: "feat/status", staged: [], unstaged: [], untracked: [], ahead: 0, behind: 0, upstream: null, is_repo: true },
    branches: [
      { name: "feat/status", is_current: true, is_remote: false, upstream: null },
      { name: "main", is_current: false, is_remote: false, upstream: "origin/main" },
      { name: "origin/main", is_current: false, is_remote: true, upstream: null },
    ],
    ...mocks,
  }),
}));
vi.mock("./CheckpointsPanel", () => ({ CheckpointsPanel: () => null }));
import { GitChangesPanel } from "./GitChangesPanel";

describe("GitChangesPanel navigation", () => {
  beforeEach(() => vi.clearAllMocks());
  it("keeps branch switching, history, and remote collaboration in the local Git drawer", async () => {
    const user = userEvent.setup();
    const onOpenHistory = vi.fn();
    const onOpenRemote = vi.fn();
    render(<GitChangesPanel sessionId="s1" onClose={() => {}} onOpenHistory={onOpenHistory} onOpenRemote={onOpenRemote} />);
    await user.selectOptions(screen.getByRole("combobox", { name: "切换本地分支" }), "main");
    await waitFor(() => expect(mocks.checkout).toHaveBeenCalledWith("main"));
    await user.click(screen.getByTitle("提交历史"));
    await user.click(screen.getByTitle("远程仓库（问题与拉取请求）"));
    expect(onOpenHistory).toHaveBeenCalledOnce();
    expect(onOpenRemote).toHaveBeenCalledOnce();
    expect(screen.queryByRole("option", { name: "origin/main" })).not.toBeInTheDocument();
  });
});
