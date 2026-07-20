// SPDX-License-Identifier: Apache-2.0
//
// The one thing the user does in conversational git setup is paste a token
// here. Contract: masked input, the value goes ONLY to onSubmit (the caller
// invokes provide_secret), and cancel is always available.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SecretPromptModal } from "./SecretPromptModal";

const request = {
  requestId: "req-1",
  purpose: "为 BumStill/CodeFactory 配置 GitHub 访问令牌(repo 权限)",
  hint: "github.com → Settings → Developer settings → Personal access tokens",
};

describe("SecretPromptModal", () => {
  it("shows the purpose, masks the input, and never renders the value as text", () => {
    render(<SecretPromptModal request={request} onSubmit={() => {}} onCancel={() => {}} />);
    expect(screen.getByText(/GitHub 访问令牌/)).toBeTruthy();
    expect(screen.getByText(/不会出现在对话/)).toBeTruthy();
    const input = screen.getByLabelText<HTMLInputElement>(/令牌/);
    expect(input.type).toBe("password");
  });

  it("submits the pasted value and disables empty submits", () => {
    const onSubmit = vi.fn();
    render(<SecretPromptModal request={request} onSubmit={onSubmit} onCancel={() => {}} />);
    const submit = screen.getByRole("button", { name: /保存并验证/ });
    expect((submit as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(screen.getByLabelText(/令牌/), { target: { value: "ghp_abc" } });
    expect((submit as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(submit);
    expect(onSubmit).toHaveBeenCalledWith("ghp_abc");
  });

  it("offers cancel", () => {
    const onCancel = vi.fn();
    render(<SecretPromptModal request={request} onSubmit={() => {}} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: /取消/ }));
    expect(onCancel).toHaveBeenCalled();
  });
});
