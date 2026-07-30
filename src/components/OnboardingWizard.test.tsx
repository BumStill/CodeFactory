// SPDX-License-Identifier: Apache-2.0
//
// First-run wizard (WorkBuddy-gap P2: zero-config onboarding). Product
// contract from the earlier overlay removal: the Workspace must stay visible
// and usable — this is a corner card, never a full-screen gate. Three checks:
// model access, delivery channel (logged-in gh preferred), delivery ceiling.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { OnboardingWizard } from "./OnboardingWizard";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

function setup(status: { gh: boolean; token: boolean; model: boolean }) {
  invokeMock.mockReset();
  invokeMock.mockImplementation((cmd: unknown) => {
    if (cmd === "delivery_channel_status") {
      return Promise.resolve({ gh_cli: status.gh, rest_token: status.token });
    }
    return Promise.resolve(null);
  });
  const onDone = vi.fn();
  render(
    <OnboardingWizard
      modelReady={status.model}
      ceiling="through_release"
      onCeilingChange={() => {}}
      onDone={onDone}
    />,
  );
  return { onDone };
}

describe("OnboardingWizard", () => {
  it("is a corner card, never a full-screen gate over the workspace", async () => {
    const { } = setup({ gh: true, token: false, model: true });
    await waitFor(() => expect(screen.getByText(/快速就绪/)).toBeTruthy());
    const root = screen.getByTestId("onboarding-wizard");
    expect(root.className).not.toContain("inset-0");
  });

  it("defaults delivery to release instead of PR-only", async () => {
    setup({ gh: true, token: false, model: true });
    await waitFor(() => expect(screen.getByDisplayValue("创建正式发布(默认)")).toBeTruthy());
    expect(screen.queryByDisplayValue("开 PR 为止(默认)")).not.toBeInTheDocument();
  });

  it("shows green checks when the model and a logged-in gh are already there", async () => {
    setup({ gh: true, token: false, model: true });
    await waitFor(() => {
      expect(screen.getByText(/模型已接入/)).toBeTruthy();
      expect(screen.getByText(/GitHub CLI 已就绪/)).toBeTruthy();
    });
  });

  it("offers the gh auth login fix when no delivery channel exists", async () => {
    setup({ gh: false, token: false, model: true });
    await waitFor(() => expect(screen.getByText(/gh auth login/)).toBeTruthy());
  });

  it("skip marks onboarding done without blocking anything", async () => {
    const { onDone } = setup({ gh: true, token: false, model: true });
    await waitFor(() => expect(screen.getByText(/快速就绪/)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: /跳过/ }));
    expect(onDone).toHaveBeenCalled();
  });
});
