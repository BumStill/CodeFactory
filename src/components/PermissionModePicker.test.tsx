// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PermissionModePicker } from "./PermissionModePicker";

const updatePermissionMode = vi.hoisted(() => vi.fn());
const chatState = vi.hoisted(() => ({
  activeSession: {
    id: "s1",
    kind: "project",
    permission_mode: "trusted",
  },
  updateActiveSessionPermissionMode: updatePermissionMode,
}));

vi.mock("../stores/chat", () => ({
  useChatStore: (selector: (state: typeof chatState) => unknown) => selector(chatState),
}));

describe("PermissionModePicker", () => {
  it("keeps standard permission icon-only but promotes trusted permission copy", () => {
    updatePermissionMode.mockReset().mockResolvedValue(undefined);
    chatState.activeSession.permission_mode = "standard";
    const { rerender } = render(<PermissionModePicker />);

    const standard = screen.getByRole("button", { name: /会话权限.*标准/ });
    expect(standard).not.toHaveTextContent("标准");
    expect(standard).toHaveClass("min-h-[44px]", "min-w-[44px]", "lg:min-h-[36px]", "lg:min-w-[36px]");

    chatState.activeSession.permission_mode = "trusted";
    rerender(<PermissionModePicker />);
    expect(screen.getByRole("button", { name: /会话权限.*信任/ })).toHaveTextContent("信任");
  });

  it("announces the current session value and is keyboard reachable before changing it", async () => {
    const user = userEvent.setup();
    updatePermissionMode.mockReset().mockResolvedValue(undefined);
    chatState.activeSession.permission_mode = "trusted";
    render(<PermissionModePicker />);

    const picker = screen.getByRole("button", { name: /会话权限.*信任/ });
    expect(picker).toHaveAccessibleDescription(
      "当前为信任模式：普通命令也可自动执行，高风险仍拦截。更改将在下一次权限判断生效。",
    );
    expect(picker).toHaveAttribute(
      "title",
      "会话权限：普通命令也可自动执行，高风险仍拦截；下一次权限判断生效",
    );
    expect(picker).toHaveClass("min-h-[44px]", "min-w-[44px]", "lg:min-h-[36px]", "lg:min-w-[36px]");

    await user.tab();
    expect(picker).toHaveFocus();
    await user.keyboard("{Enter}");
    await user.click(screen.getByRole("menuitemradio", { name: /安全/ }));
    expect(updatePermissionMode).toHaveBeenCalledWith("safe");
  });

  it("closes the portal menu and restores its trigger when Tab would leave it", async () => {
    const user = userEvent.setup();
    chatState.activeSession.permission_mode = "standard";
    render(<PermissionModePicker />);

    const trigger = screen.getByRole("button", { name: /会话权限.*标准/ });
    await user.click(trigger);
    expect(screen.getByRole("menu", { name: "选择会话权限" })).toBeInTheDocument();
    await user.keyboard("{Tab}");
    expect(screen.queryByRole("menu", { name: "选择会话权限" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
});
