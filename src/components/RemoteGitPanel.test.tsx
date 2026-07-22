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

    await userEvent.click(screen.getByText("Keep specifications in the repository"));

    expect(screen.getByRole("heading", { name: "Keep specifications in the repository" })).toBeInTheDocument();
    expect(screen.getByTitle("在浏览器中打开")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "创建为规范" })).not.toBeInTheDocument();
  });
});
