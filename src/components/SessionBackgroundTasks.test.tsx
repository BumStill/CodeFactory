// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { SessionBackgroundTasks } from "./SessionBackgroundTasks";
import type { SessionBackgroundActivity } from "../lib/backgroundTasks";

const activity: SessionBackgroundActivity = {
  tasks: [
    {
      id: "bg-1",
      session_id: "s1",
      command: "gh pr checks 42 --watch",
      cwd: "/repo",
      source: "agent_decided",
      status: "running",
      exit_code: null,
      created_at: 1,
      finished_at: null,
      timeout_sec: 10800,
    },
    {
      id: "bg-2",
      session_id: "s1",
      command: "pnpm test",
      cwd: "/repo",
      source: "user_requested",
      status: "exited",
      exit_code: 0,
      created_at: 2,
      finished_at: 3,
      timeout_sec: 600,
    },
  ],
  wakeups: [
    {
      id: "wk-1",
      session_id: "s1",
      prompt: "巡检线上状态",
      source: "user_requested",
      after_sec: null,
      every_sec: 3600,
      next_at: 10,
      status: "pending",
      fire_count: 0,
    },
  ],
};

describe("SessionBackgroundTasks", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(activity);
  });

  it("shows a small in-progress marker and expands to command, source, status and stop", async () => {
    render(<SessionBackgroundTasks sessionId="s1" />);

    const marker = await screen.findByRole("button", { name: /后台活动：1 个后台任务进行中/ });
    expect(marker).toHaveTextContent("1 个后台任务进行中");
    expect(invokeMock).toHaveBeenCalledWith("list_session_background_activity", { sessionId: "s1" });
    // 标记是会话头部的一部分，不是消息流里的一条消息。
    expect(screen.queryByRole("article")).not.toBeInTheDocument();

    await userEvent.click(marker);
    const panel = screen.getByRole("region", { name: "后台任务详情" });
    expect(panel).toHaveTextContent("gh pr checks 42 --watch");
    expect(panel).toHaveTextContent("由 agent 启动 · 运行中");
    expect(panel).toHaveTextContent("pnpm test");
    expect(panel).toHaveTextContent("按你的要求 · 已结束 · 退出码 0");
    expect(panel).toHaveTextContent("巡检线上状态");
    expect(panel).toHaveTextContent("每 3600 秒");

    await userEvent.click(screen.getByRole("button", { name: /停止后台任务 gh pr checks 42 --watch/ }));
    expect(invokeMock).toHaveBeenCalledWith("stop_session_background_task", {
      sessionId: "s1",
      taskId: "bg-1",
    });

    await userEvent.click(screen.getByRole("button", { name: /取消定时唤醒 巡检线上状态/ }));
    expect(invokeMock).toHaveBeenCalledWith("cancel_session_wakeup", {
      sessionId: "s1",
      wakeupId: "wk-1",
    });
  });

  it("stays out of the way when this session has no live background work", async () => {
    invokeMock.mockResolvedValue({ tasks: [{ ...activity.tasks[1] }], wakeups: [] });
    render(<SessionBackgroundTasks sessionId="s2" />);

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("list_session_background_activity", { sessionId: "s2" }));
    expect(screen.queryByRole("button", { name: /后台活动/ })).not.toBeInTheDocument();
  });

  it("never queries another session's activity", async () => {
    const { rerender } = render(<SessionBackgroundTasks sessionId="s1" />);
    await screen.findByRole("button", { name: /后台活动/ });
    invokeMock.mockClear();

    rerender(<SessionBackgroundTasks sessionId={null} />);
    await waitFor(() => expect(screen.queryByRole("button", { name: /后台活动/ })).not.toBeInTheDocument());
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
