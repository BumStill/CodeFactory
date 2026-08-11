// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  status: {
    branch: "feat/status",
    staged: [] as Array<{ path: string; status: string }>,
    unstaged: [] as Array<{ path: string; status: string }>,
    untracked: [] as string[],
    ahead: 0,
    behind: 0,
    upstream: null,
    is_repo: true,
  },
  refreshStatus: vi.fn().mockResolvedValue(undefined),
  refreshBranches: vi.fn().mockResolvedValue(undefined),
  checkout: vi.fn().mockResolvedValue(undefined),
  stageFiles: vi.fn().mockResolvedValue(undefined),
  commit: vi.fn().mockResolvedValue(undefined),
  getFileDiff: vi.fn().mockResolvedValue("diff --git a/src/app.ts b/src/app.ts"),
}));
vi.mock("../stores/git", () => ({
  useGitStore: () => ({
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
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.status.staged = [];
    mocks.status.unstaged = [];
    mocks.status.untracked = [];
  });
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

  it("keeps an embedded pane responsive without a horizontal two-column minimum", async () => {
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 480 } as DOMRect);
    const { container, unmount } = render(
      <GitChangesPanel embedded sessionId="s1" onClose={() => {}} onOpenHistory={() => {}} onOpenRemote={() => {}} />,
    );

    await waitFor(() => expect(container.firstElementChild).toHaveAttribute("data-embedded-layout", "stacked"));
    expect(screen.getByTestId("git-changes-header")).toHaveClass("flex-wrap");
    expect(screen.getByTestId("git-changes-actions")).toHaveClass("flex-wrap");
    expect(screen.getByTestId("git-changes-body")).toHaveClass("flex-col");
    expect(screen.getByTestId("git-changes-file-list")).toHaveClass("w-full", "border-b");

    unmount();
    rect.mockReturnValue({ width: 800 } as DOMRect);
    const wide = render(
      <GitChangesPanel embedded sessionId="s1" onClose={() => {}} onOpenHistory={() => {}} onOpenRemote={() => {}} />,
    );
    await waitFor(() => expect(wide.container.firstElementChild).toHaveAttribute("data-embedded-layout", "split"));
    expect(screen.getByTestId("git-changes-header")).toHaveClass("flex-nowrap");
    expect(screen.getByTestId("git-changes-actions")).toHaveClass("flex-nowrap");
    expect(screen.getByTestId("git-changes-body")).toHaveClass("flex-row");
    expect(screen.getByTestId("git-changes-file-list")).toHaveClass("w-[260px]", "border-r");
    expect(screen.getByRole("button", { name: "关闭本地 Git" })).toHaveClass("h-9", "w-9");
    rect.mockRestore();
  });

  it("opens a file diff from a keyboard-operable row and exposes the active file", async () => {
    mocks.status.unstaged = [{ path: "src/app.ts", status: "modified" }];
    const user = userEvent.setup();
    render(
      <GitChangesPanel embedded sessionId="s1" onClose={() => {}} onOpenHistory={() => {}} onOpenRemote={() => {}} />,
    );

    const fileButton = screen.getByRole("button", { name: "查看 src/app.ts 差异" });
    fileButton.focus();
    await user.keyboard("{Enter}");

    await waitFor(() => expect(mocks.getFileDiff).toHaveBeenCalledWith("src/app.ts", false));
    expect(fileButton).toHaveAttribute("aria-current", "true");
  });

  it("contains the embedded commit dialog, traps focus, closes with Escape, and restores focus", async () => {
    mocks.status.staged = [{ path: "src/app.ts", status: "modified" }];
    const user = userEvent.setup();
    render(
      <GitChangesPanel embedded sessionId="s1" onClose={() => {}} onOpenHistory={() => {}} onOpenRemote={() => {}} />,
    );

    const trigger = screen.getByRole("button", { name: "提交 (1)" });
    await user.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "提交 1 个文件" });
    expect(dialog).toHaveAttribute("aria-modal", "false");
    expect(dialog.parentElement).toHaveClass("absolute");
    expect(dialog.parentElement).not.toHaveClass("fixed");

    await user.type(screen.getByPlaceholderText("提交信息…"), "feat: embedded git");
    const submit = screen.getByRole("button", { name: "提交" });
    submit.focus();
    await user.tab();
    expect(screen.getByPlaceholderText("提交信息…")).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "提交 1 个文件" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("marks the embedded close control as the auxiliary pane initial focus target", () => {
    render(
      <GitChangesPanel embedded sessionId="s1" onClose={() => {}} onOpenHistory={() => {}} onOpenRemote={() => {}} />,
    );
    const close = screen.getByRole("button", { name: "关闭本地 Git" });
    expect(close).toHaveAttribute("data-auxiliary-initial-focus", "true");
    expect(close).toHaveClass("h-11", "w-11");
  });

  it("keeps all narrow embedded Git actions and file rows at a 44px target", () => {
    mocks.status.unstaged = [{ path: "src/app.ts", status: "modified" }];
    render(
      <GitChangesPanel embedded sessionId="s1" onClose={() => {}} onOpenHistory={() => {}} onOpenRemote={() => {}} />,
    );

    expect(screen.getByTitle("提交历史")).toHaveClass("h-11");
    expect(screen.getByRole("button", { name: "刷新本地 Git" })).toHaveClass("h-11", "w-11");
    expect(screen.getByRole("combobox", { name: "切换本地分支" })).toHaveClass("h-11");
    expect(screen.getByRole("button", { name: "查看 src/app.ts 差异" })).toHaveClass("min-h-11");
  });
});
