// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  refreshCommits: vi.fn().mockResolvedValue(undefined),
  commits: [
    {
      hash: "abcdef1234567890",
      short_hash: "abcdef1",
      message: "Keep the workspace accessible",
      message_body: "Keep the workspace accessible\n\nExpose the commit details as an accordion.",
      author: "Leo",
      email: "leo@example.com",
      timestamp: 1_700_000_000,
    },
  ],
}));

vi.mock("../stores/git", () => ({
  useGitStore: () => ({ commits: mocks.commits, refreshCommits: mocks.refreshCommits }),
}));

import { GitHistoryPanel } from "./GitHistoryPanel";

describe("GitHistoryPanel embedded accessibility", () => {
  beforeEach(() => {
    mocks.refreshCommits.mockClear();
  });

  it("gives the embedded close control an accessible name and initial-focus target", async () => {
    render(<GitHistoryPanel embedded onClose={() => {}} />);
    const close = screen.getByRole("button", { name: "返回本地 Git" });
    expect(close).toHaveAttribute("data-auxiliary-initial-focus", "true");
    expect(close).toHaveClass("h-11", "w-11");
    await waitFor(() => expect(mocks.refreshCommits).toHaveBeenCalled());
  });

  it("exposes each commit as an accordion with a stable controlled region", async () => {
    const user = userEvent.setup();
    render(<GitHistoryPanel embedded onClose={() => {}} />);
    await waitFor(() => expect(mocks.refreshCommits).toHaveBeenCalledTimes(1));

    const toggle = screen.getByRole("button", { name: /abcdef1 Keep the workspace accessible/ });
    expect(toggle).toHaveAttribute("id", "git-history-commit-abcdef1234567890");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", "git-history-details-abcdef1234567890");

    const details = document.getElementById("git-history-details-abcdef1234567890");
    expect(details).toHaveAttribute("id", "git-history-details-abcdef1234567890");
    expect(details).toHaveAttribute("role", "region");
    expect(details).toHaveAttribute("aria-labelledby", "git-history-commit-abcdef1234567890");
    expect(details).not.toBeVisible();

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("region", { name: /abcdef1 Keep the workspace accessible/ })).toBe(details);
    expect(details).toBeVisible();
  });
});
