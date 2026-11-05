// SPDX-License-Identifier: Apache-2.0
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  state: { forceRequested: false, forceInstall: vi.fn(), updateMinimized: false, setUpdateMinimized: vi.fn() },
}));

vi.mock("../stores/updater", () => ({
  describeUpdateBlockers: () => ["1 个对话正在回答", "1 个终端正在运行命令"],
  useUpdaterStore: <T,>(selector: (state: typeof mocks.state) => T): T =>
    selector(mocks.state),
}));

import { UpdateBlockerChoice } from "./UpdateBlockerChoice";

const blockers = { active_chat_turns: 1, terminal_sessions: 1 } as never;

describe("UpdateBlockerChoice", () => {
  beforeEach(() => {
    mocks.state.forceRequested = false;
    mocks.state.forceInstall.mockClear();
    mocks.state.updateMinimized = false;
    mocks.state.setUpdateMinimized.mockClear();
  });

  it("names the running work and offers both choices", () => {
    render(<UpdateBlockerChoice blockers={blockers} />);
    expect(screen.getByText("1 个对话正在回答")).toBeInTheDocument();
    expect(screen.getByText("1 个终端正在运行命令")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "等完成后更新" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "立即更新" })).toBeInTheDocument();
  });

  it("confirms what will be interrupted before forcing", () => {
    render(<UpdateBlockerChoice blockers={blockers} />);
    fireEvent.click(screen.getByRole("button", { name: "立即更新" }));
    expect(mocks.state.forceInstall).not.toHaveBeenCalled();
    expect(screen.getByText(/上面的工作会被中断/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "中断并更新" }));
    expect(mocks.state.forceInstall).toHaveBeenCalledTimes(1);
  });

  it("waiting minimizes the update window without forcing", () => {
    render(<UpdateBlockerChoice blockers={blockers} />);
    fireEvent.click(screen.getByRole("button", { name: "等完成后更新" }));
    expect(mocks.state.setUpdateMinimized).toHaveBeenCalledWith(true);
    expect(mocks.state.forceInstall).not.toHaveBeenCalled();
  });

  it("still allows updating now after choosing to wait", () => {
    mocks.state.updateMinimized = true;
    render(<UpdateBlockerChoice blockers={blockers} />);
    expect(screen.getByText("完成后会自动更新。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "立即更新" })).toBeInTheDocument();
  });

  it("shows progress once forcing was requested", () => {
    mocks.state.forceRequested = true;
    render(<UpdateBlockerChoice blockers={blockers} />);
    expect(screen.getByText("正在立即更新…")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
