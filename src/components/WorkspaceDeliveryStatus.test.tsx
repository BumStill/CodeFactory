// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { UIMessage } from "../stores/chat";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("../lib/tauri", async (original) => ({ ...(await original()), invoke: mocks.invoke }));
import { WorkspaceDeliveryStatus, deliveryReferenceFromMessages } from "./WorkspaceDeliveryStatus";

const delivered: UIMessage[] = [{
  id: "m1",
  role: "assistant",
  content: "",
  createdAt: 1,
  toolCalls: [{
    id: "d1",
    name: "deliver_changes",
    args: "{}",
    status: "done",
    result: "交付结果: delivered\n分支: feat/workspace-ui\n  ✅ pr: PR #175: https://github.com/acme/repo/pull/175\nPR: https://github.com/acme/repo/pull/175",
  }],
}];

describe("WorkspaceDeliveryStatus", () => {
  beforeEach(() => mocks.invoke.mockReset());

  it("restores the session PR reference after the local worktree has returned to main", () => {
    expect(deliveryReferenceFromMessages(delivered)).toEqual({ branch: "feat/workspace-ui", prNumber: 175 });
  });

  it("shows PR, CI, merge, and release as one delivery chain", async () => {
    mocks.invoke.mockResolvedValue({
      remote_available: true,
      pr: {
        number: 175,
        title: "Improve workspace",
        state: "merged",
        draft: false,
        head_branch: "feat/workspace-ui",
        base_branch: "main",
        head_sha: "abc",
        merge_commit_sha: "def",
        url: "https://github.com/acme/repo/pull/175",
      },
      ci_status: "success",
      release: { tag: "v1.63.0", url: "https://github.com/acme/repo/releases/tag/v1.63.0", published_at: "2026-07-23T00:00:00Z" },
      error: null,
    });
    render(<WorkspaceDeliveryStatus cwd="/repo" currentBranch="main" messages={delivered} />);

    const status = await screen.findByRole("button", {
      name: /会话交付状态.*PR #175.*CI 通过.*已合并.*v1\.63\.0.*未验证上线/,
    });
    expect(status).toHaveTextContent("PR #175");
    expect(status).toHaveTextContent("未验证上线");
    expect(status).not.toHaveTextContent("CI 通过");
    expect(status).not.toHaveTextContent("已合并");
    expect(status).not.toHaveTextContent("v1.63.0");
    expect(status).toHaveAttribute("data-status-tone", "progress");
    expect(status).toHaveAttribute("aria-expanded", "false");
    expect(mocks.invoke).toHaveBeenCalledWith("workspace_delivery_status", {
      cwd: "/repo",
      branch: "feat/workspace-ui",
      prNumber: 175,
    });

    await userEvent.click(status);
    const drawer = screen.getByRole("dialog", { name: "交付详情" });
    const close = screen.getByRole("button", { name: "关闭交付详情" });
    await waitFor(() => expect(close).toHaveFocus());
    expect(status).toHaveAttribute("aria-expanded", "true");
    expect(drawer).toHaveTextContent("feat/workspace-ui → main");
    expect(drawer).toHaveTextContent("release artifact 可见");
    expect(drawer).toHaveTextContent("真实上线还需要 deliver_changes 的部署观察或 live verifier 通过");
    expect(drawer).toHaveTextContent("线上验证");
    expect(drawer).toHaveTextContent("未验证上线");
    expect(drawer).toHaveTextContent("6");

    await userEvent.tab({ shift: true });
    expect(screen.getByRole("link", { name: "打开 PR #175" })).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "交付详情" })).not.toBeInTheDocument();
    await waitFor(() => expect(status).toHaveFocus());
    expect(status).toHaveAttribute("aria-expanded", "false");
  });

  it("does not misreport an unavailable remote as no PR", async () => {
    mocks.invoke.mockResolvedValue({
      remote_available: false,
      pr: null,
      ci_status: "none",
      release: null,
      error: "not authenticated",
    });
    render(<WorkspaceDeliveryStatus cwd="/repo" currentBranch="feat/workspace-ui" messages={[]} />);
    const status = await screen.findByRole("button", {
      name: /会话交付状态.*远程状态不可用/,
    });
    expect(status).toHaveTextContent("远程状态不可用");
    expect(status).toHaveAttribute("data-status-tone", "warning");
    expect(screen.queryByText("未关联 PR")).not.toBeInTheDocument();
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalled());
  });

  it("delegates controlled details to the shared auxiliary pane without mounting a legacy drawer", async () => {
    mocks.invoke.mockResolvedValue({
      remote_available: true,
      pr: null,
      ci_status: "none",
      release: null,
      error: null,
    });
    const onOpenDetails = vi.fn();
    render(
      <WorkspaceDeliveryStatus
        cwd="/repo"
        currentBranch="main"
        messages={[]}
        detailsOpen
        detailsId="workspace-auxiliary-pane"
        onOpenDetails={onOpenDetails}
      />,
    );

    const status = await screen.findByRole("button", { name: /会话交付状态/ });
    expect(status).toHaveAttribute("aria-expanded", "true");
    expect(status).toHaveAttribute("aria-controls", "workspace-auxiliary-pane");
    expect(screen.queryByRole("dialog", { name: "交付详情" })).not.toBeInTheDocument();
    await userEvent.click(status);
    expect(onOpenDetails).toHaveBeenCalledTimes(1);
  });

  it("lets an embedded details view reuse the header snapshot instead of polling twice", async () => {
    mocks.invoke.mockResolvedValue({
      remote_available: true,
      pr: null,
      ci_status: "none",
      release: null,
      error: null,
    });
    render(
      <>
        <WorkspaceDeliveryStatus cwd="/repo" currentBranch="main" messages={[]} />
        <WorkspaceDeliveryStatus
          cwd="/repo"
          currentBranch="main"
          messages={[]}
          detailsOnly
          {...({ deliveryState: { snapshot: null, unavailable: false } } as Record<string, unknown>)}
        />
      </>,
    );

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalled());
    expect(mocks.invoke).toHaveBeenCalledTimes(1);
  });

  it("does not show the previous session PR while the next session is still loading", async () => {
    let resolveNext!: (value: unknown) => void;
    mocks.invoke
      .mockResolvedValueOnce({
        remote_available: true,
        pr: {
          number: 101,
          title: "Session A",
          state: "open",
          draft: false,
          head_branch: "feat/a",
          base_branch: "main",
          head_sha: "a",
          merge_commit_sha: null,
          url: "https://example.test/pull/101",
        },
        ci_status: "pending",
        release: null,
        error: null,
      })
      .mockImplementationOnce(() => new Promise((resolve) => { resolveNext = resolve; }));

    const { rerender } = render(
      <WorkspaceDeliveryStatus cwd="/repo-a" sessionId="session-a" currentBranch="feat/a" messages={[]} />,
    );
    expect(await screen.findByText("PR #101")).toBeInTheDocument();

    rerender(
      <WorkspaceDeliveryStatus cwd="/repo-b" sessionId="session-b" currentBranch="feat/b" messages={[]} />,
    );
    expect(screen.queryByText("PR #101")).not.toBeInTheDocument();

    resolveNext({ remote_available: true, pr: null, ci_status: "none", release: null, error: null });
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(2));
  });
});
