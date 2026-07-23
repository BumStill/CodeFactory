// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CheckpointsPanel } from "./CheckpointsPanel";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unlisten: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

const checkpoints = [
  { id: "cp-1", session_id: "s1", message_id: "m1", cwd: "/repo", git_sha: "aaaaaaa111", label: "修复登录", created_at: "2026-07-20T20:48:00Z", reverted: false },
  { id: "cp-dup", session_id: "s1", message_id: "m2", cwd: "/repo", git_sha: "aaaaaaa111", label: "继续", created_at: "2026-07-20T20:47:00Z", reverted: false },
  { id: "cp-2", session_id: "s1", message_id: "m3", cwd: "/repo", git_sha: "bbbbbbb222", label: "整理设置页", created_at: "2026-07-20T20:40:00Z", reverted: false },
  { id: "cp-3", session_id: "s1", message_id: "m4", cwd: "/repo", git_sha: "ccccccc333", label: "补测试", created_at: "2026-07-20T20:30:00Z", reverted: false },
  { id: "cp-4", session_id: "s1", message_id: "m5", cwd: "/repo", git_sha: "ddddddd444", label: "第四个有效检查点", created_at: "2026-07-20T20:20:00Z", reverted: false },
  { id: "cp-empty", session_id: "s1", message_id: "m6", cwd: "/repo", git_sha: "eeeeeee555", label: "没有文件差异", created_at: "2026-07-20T20:10:00Z", reverted: false },
];

const changesById: Record<string, Array<{ path: string; status: string }>> = {
  "cp-1": [{ path: "src/login.ts", status: "modified" }],
  "cp-2": [{ path: "src/settings.ts", status: "added" }],
  "cp-3": [{ path: "src/login.test.ts", status: "modified" }],
  "cp-4": [{ path: "README.md", status: "modified" }],
  "cp-empty": [],
};

describe("CheckpointsPanel recovery entry", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
    mocks.unlisten.mockReset();
    mocks.listen.mockResolvedValue(mocks.unlisten);
    mocks.invoke.mockImplementation((cmd: string, args?: { checkpointId?: string }) => {
      if (cmd === "list_checkpoints") return Promise.resolve(checkpoints);
      if (cmd === "checkpoint_changeset") {
        return Promise.resolve(changesById[args?.checkpointId ?? ""] ?? []);
      }
      if (cmd === "revert_checkpoint") return Promise.resolve(undefined);
      return Promise.resolve(undefined);
    });
  });

  it("deduplicates snapshots and opens a drawer with three recent changed checkpoints", async () => {
    const user = userEvent.setup();
    render(<CheckpointsPanel sessionId="s1" />);

    const trigger = await screen.findByRole("button", { name: "恢复 4" });
    expect(screen.queryByRole("heading", { name: "检查点" })).not.toBeInTheDocument();

    await user.click(trigger);
    expect(await screen.findByRole("heading", { name: "检查点" })).toBeInTheDocument();

    await waitFor(() => {
      expect(screen.getByText("修复登录")).toBeInTheDocument();
      expect(screen.getByText("整理设置页")).toBeInTheDocument();
      expect(screen.getByText("补测试")).toBeInTheDocument();
    });
    expect(screen.queryByText("继续")).not.toBeInTheDocument();
    expect(screen.queryByText("第四个有效检查点")).not.toBeInTheDocument();
    expect(screen.queryByText("没有文件差异")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "查看最近 4 个有效检查点" }));
    expect(screen.getByText("第四个有效检查点")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "查看 1 个无差异检查点" }));
    expect(screen.getByText("没有文件差异")).toBeInTheDocument();
  });

  it("keeps file-diff confirmation before restoring a checkpoint", async () => {
    const user = userEvent.setup();
    render(<CheckpointsPanel sessionId="s1" />);

    await user.click(await screen.findByRole("button", { name: "恢复 4" }));
    await screen.findByText("修复登录");
    await user.click(screen.getByRole("button", { name: "恢复检查点 修复登录" }));

    expect(await screen.findByText("src/login.ts")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "确认恢复" }));

    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("revert_checkpoint", { checkpointId: "cp-1" });
    });
  });
  it("hides the recovery entry when no checkpoint differs from the worktree", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_checkpoints") return Promise.resolve(checkpoints);
      if (cmd === "checkpoint_changeset") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });
    render(<CheckpointsPanel sessionId="s1" />);
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("checkpoint_changeset", { checkpointId: "cp-empty" }));
    expect(screen.queryByRole("button", { name: /恢复 \d+/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/检查点 5/)).not.toBeInTheDocument();
  });

  it("bounds recovery diff checks for long-running sessions", async () => {
    const many = Array.from({ length: 36 }, (_, index) => ({
      ...checkpoints[0],
      id: `cp-${index}`,
      git_sha: `sha-${index}`,
      created_at: new Date(Date.UTC(2026, 6, 23, 12, 0, 0) - index * 1_000).toISOString(),
    }));
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_checkpoints") return Promise.resolve(many);
      if (cmd === "checkpoint_changeset") return Promise.resolve([{ path: "src/a.ts", status: "modified" }]);
      return Promise.resolve(undefined);
    });
    render(<CheckpointsPanel sessionId="s1" />);
    await screen.findByRole("button", { name: "恢复 12" });
    expect(mocks.invoke.mock.calls.filter(([command]) => command === "checkpoint_changeset")).toHaveLength(12);
  });

});
