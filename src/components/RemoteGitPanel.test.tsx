// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const remoteState = vi.hoisted(() => ({
  remotes: [
    {
      id: "remote-1",
      name: "origin",
      provider: "github",
      base_url: "https://api.github.com",
      token_ref: null,
      default_repo: "owner/repo",
      has_token: true,
    },
  ],
  issues: [
    {
      id: "issue-7",
      number: 7,
      title: "Keep specifications in the repository",
      body: "Use ordinary versioned Markdown.",
      state: "open",
      author: "leo",
      labels: ["product"],
      url: "https://github.com/owner/repo/issues/7",
      created_at: "2026-07-22T00:00:00Z",
      updated_at: "2026-07-22T00:00:00Z",
    },
  ],
  prs: [],
  repos: [],
  loading: false,
  error: null,
  loadRemotes: vi.fn(),
  loadIssues: vi.fn(),
  loadPRs: vi.fn(),
  loadRepos: vi.fn(),
  createIssue: vi.fn(),
  createPR: vi.fn(),
}));

vi.mock("../stores/gitRemote", () => ({
  useGitRemoteStore: () => remoteState,
}));
vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));

import { RemoteGitPanel } from "./RemoteGitPanel";

describe("RemoteGitPanel repository-owned intent", () => {
  it("keeps issue browsing but has no app-owned create-spec action", async () => {
    render(<RemoteGitPanel currentBranch="main" onClose={() => {}} />);

    expect(screen.getByTitle("刷新")).toHaveClass("h-9", "w-9");
    await userEvent.click(screen.getByText("Keep specifications in the repository"));

    expect(screen.getByRole("heading", { name: "Keep specifications in the repository" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "返回问题列表" })).toHaveClass("h-9", "w-9");
    expect(screen.getByTitle("在浏览器中打开")).toHaveClass("h-9", "w-9");
    expect(screen.queryByRole("button", { name: "创建为规范" })).not.toBeInTheDocument();
  });

  it("names and sizes the embedded return control as the pane initial focus target", () => {
    render(<RemoteGitPanel embedded currentBranch="main" onClose={() => {}} />);

    const close = screen.getByRole("button", { name: "返回本地 Git" });
    expect(close).toHaveAttribute("data-auxiliary-initial-focus", "true");
    expect(close).toHaveClass("h-11", "w-11");
    expect(screen.getByRole("tab", { name: "问题" })).toHaveClass("h-11");
    expect(screen.getByTitle("刷新")).toHaveClass("h-11", "w-11");
  });

  it("names and sizes nested PR form back and action controls", async () => {
    const user = userEvent.setup();
    render(<RemoteGitPanel embedded currentBranch="main" onClose={() => {}} />);

    await user.click(screen.getByRole("tab", { name: "拉取请求" }));
    const create = screen.getByTitle("创建拉取请求");
    expect(create).toHaveClass("h-11", "w-11");
    await user.click(create);
    expect(screen.getByRole("button", { name: "返回拉取请求列表" })).toHaveClass("h-11", "w-11");
    expect(screen.getByRole("button", { name: "取消" })).toHaveClass("h-11");
  });

  it("exposes a roving tab set and switches tabs with horizontal arrow keys", async () => {
    const user = userEvent.setup();
    render(<RemoteGitPanel currentBranch="main" onClose={() => {}} />);

    expect(screen.getByRole("tablist", { name: "远程仓库视图" })).toBeInTheDocument();
    const issuesTab = screen.getByRole("tab", { name: "问题" });
    const prsTab = screen.getByRole("tab", { name: "拉取请求" });
    expect(issuesTab).toHaveAttribute("id", "remote-git-tab-issues");
    expect(issuesTab).toHaveAttribute("aria-selected", "true");
    expect(issuesTab).toHaveAttribute("aria-controls", "remote-git-tabpanel-issues");
    expect(issuesTab).toHaveAttribute("tabindex", "0");
    expect(prsTab).toHaveAttribute("aria-selected", "false");
    expect(prsTab).toHaveAttribute("tabindex", "-1");
    expect(screen.getByRole("tabpanel", { name: "问题" })).toHaveAttribute(
      "id",
      "remote-git-tabpanel-issues",
    );

    issuesTab.focus();
    await user.keyboard("{ArrowRight}");
    expect(prsTab).toHaveFocus();
    expect(prsTab).toHaveAttribute("aria-selected", "true");
    expect(prsTab).toHaveAttribute("aria-controls", "remote-git-tabpanel-prs");
    expect(prsTab).toHaveAttribute("tabindex", "0");
    expect(issuesTab).toHaveAttribute("tabindex", "-1");
    expect(screen.getByRole("tabpanel", { name: "拉取请求" })).toHaveAttribute(
      "id",
      "remote-git-tabpanel-prs",
    );

    await user.keyboard("{ArrowLeft}");
    expect(issuesTab).toHaveFocus();
    expect(issuesTab).toHaveAttribute("aria-selected", "true");
  });
});
