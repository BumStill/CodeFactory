// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MessageList } from "./MessageList";

const mocks = vi.hoisted(() => ({
  start: vi.fn(),
  open: vi.fn(),
  status: vi.fn(),
  cancel: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  codexLoginStart: mocks.start,
  codexLoginOpen: mocks.open,
  codexLoginStatus: mocks.status,
  codexLoginCancel: mocks.cancel,
}));

describe("MessageList ChatGPT authorization recovery", () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) mock.mockReset();
    mocks.start.mockResolvedValue({
      flow_id: "shared-flow",
      authorization_url: "https://auth.openai.test/flow",
      status: "waiting",
      expires_at: Date.now() + 300_000,
      browser_open_error: "系统未能打开浏览器",
    });
  });

  it("hydrates an expired historical turn with an in-place manual recovery flow", async () => {
    const user = userEvent.setup();
    render(
      <MessageList
        streaming={false}
        messages={[
          {
            id: "auth-expired",
            role: "user",
            content: "HTTP 401 Unauthorized",
            completionState: "auth_expired",
            createdAt: Date.now(),
          },
        ]}
      />,
    );

    expect(screen.getByText("ChatGPT 授权已过期")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "重新验证" }));
    expect(await screen.findByRole("button", { name: "打开验证页面" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制链接" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消" })).toBeInTheDocument();
    expect(mocks.start).toHaveBeenCalledTimes(1);
  });
});
