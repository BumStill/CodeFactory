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
  it("announces the current session value and is keyboard reachable before changing it", async () => {
    const user = userEvent.setup();
    updatePermissionMode.mockReset().mockResolvedValue(undefined);
    render(<PermissionModePicker />);

    const picker = screen.getByRole("combobox", { name: "会话权限" });
    expect(picker).toHaveAttribute("id", "workspace-permission-mode");
    expect(picker).toHaveValue("trusted");
    expect(picker).toHaveAccessibleName("会话权限");
    expect(picker).toHaveAccessibleDescription(
      "当前为信任模式：普通命令也可自动执行，高风险仍拦截。更改将在下一次权限判断生效。",
    );
    expect(picker.closest("label")).toHaveAttribute(
      "title",
      "会话权限：普通命令也可自动执行，高风险仍拦截；下一次权限判断生效",
    );
    expect(picker).toHaveClass("min-h-11", "lg:min-h-9");

    await user.tab();
    expect(picker).toHaveFocus();
    await user.selectOptions(picker, "safe");
    expect(updatePermissionMode).toHaveBeenCalledWith("safe");
  });
});
