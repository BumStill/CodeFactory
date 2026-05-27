// SPDX-License-Identifier: Apache-2.0
//
// First-run onboarding flow tests. We're verifying the user-visible
// contract (not implementation details):
//   - The overlay renders the welcome step on mount.
//   - The footer "下一步" walks welcome → api-key → first-action.
//   - Entering an API key + clicking save invokes save_api_key with
//     the right keyRef and advances to step 3.
//   - Skipping (empty key) also advances.
//   - Picking an action on step 3 calls the right onPick* callback AND
//     flips settings.onboarded=true via save().
//   - Top-right × dismisses + flips onboarded=true.
//
// Per AGENTS.md UX-verify rule — these are NOT a substitute for a live
// run in the dev wrapper, but they do guard against the common silent
// regressions (overlay showing forever, save_api_key called with wrong
// keyRef, action callbacks not firing).

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, act } from "@testing-library/react";

const saveMock = vi.hoisted(() => vi.fn(async () => {}));
const saveApiKeyMock = vi.hoisted(() => vi.fn(async () => {}));
const fakeSettings = {
  endpoints: {
    openrouter: {
      base_url: "https://openrouter.ai/api/v1",
      key_ref: undefined,
      api_style: "openai" as const,
      custom_models: [],
      active_model: "anthropic/claude-opus-4-7",
    },
  },
  default_endpoint: "openrouter",
  default_model: "anthropic/claude-opus-4-7",
  permissions: { allow: [], ask: [], deny: [], full_access: false },
  shell: { shell: "powershell" },
  auto_create_pr: false,
  theme: "dark" as const,
  font_family: "inter",
  font_size: 14,
  onboarded: false,
};

vi.mock("../stores/settings", () => ({
  useSettingsStore: () => ({
    settings: fakeSettings,
    save: saveMock,
    saveApiKey: saveApiKeyMock,
  }),
}));

import { OnboardingOverlay } from "./OnboardingOverlay";

function setup() {
  const onClose = vi.fn();
  const onPickNewProject = vi.fn();
  const onPickQuickTask = vi.fn();
  const onPickProfile = vi.fn();
  const utils = render(
    <OnboardingOverlay
      onClose={onClose}
      onPickNewProject={onPickNewProject}
      onPickQuickTask={onPickQuickTask}
      onPickProfile={onPickProfile}
    />
  );
  return { ...utils, onClose, onPickNewProject, onPickQuickTask, onPickProfile };
}

beforeEach(() => {
  saveMock.mockReset(); saveMock.mockResolvedValue(undefined);
  saveApiKeyMock.mockReset(); saveApiKeyMock.mockResolvedValue(undefined);
});

describe("OnboardingOverlay", () => {

  it("renders welcome step on mount", () => {
    setup();
    expect(screen.getByText(/欢迎使用 CodeFactory/)).toBeInTheDocument();
    expect(screen.getByText(/软件工厂/)).toBeInTheDocument();
  });

  it("advances welcome → api-key → first-action via footer buttons", async () => {
    setup();
    // Welcome → API key
    fireEvent.click(screen.getByText(/下一步/));
    expect(await screen.findByText(/API Key/)).toBeInTheDocument();
    // API key (empty) → first action via skip button text
    fireEvent.click(screen.getByText(/稍后再配，继续/));
    expect(await screen.findByText(/三种用法/)).toBeInTheDocument();
  });

  it("saves API key with the right keyRef on advance", async () => {
    setup();
    fireEvent.click(screen.getByText(/下一步/));
    const input = await screen.findByPlaceholderText(/sk-or-v1/);
    fireEvent.change(input, { target: { value: "sk-or-v1-test123" } });
    fireEvent.click(screen.getByText(/保存并继续/));
    await waitFor(() =>
      expect(saveApiKeyMock).toHaveBeenCalledWith(
        "codefactory.endpoint.openrouter",
        "sk-or-v1-test123",
      ),
    );
    // Also patches the endpoint's key_ref via save()
    await waitFor(() => expect(saveMock).toHaveBeenCalled());
  });

  it("picking 新建项目 calls onPickNewProject + marks onboarded", async () => {
    const { onPickNewProject, onClose } = setup();
    fireEvent.click(screen.getByText(/下一步/));
    await screen.findByText(/API Key/);
    fireEvent.click(screen.getByText(/稍后再配，继续/));
    await screen.findByText(/三种用法/);
    await act(async () => {
      fireEvent.click(screen.getByText("新建项目"));
    });
    await waitFor(() => expect(onPickNewProject).toHaveBeenCalled());
    expect(saveMock).toHaveBeenCalledWith(
      expect.objectContaining({ onboarded: true }),
    );
    expect(onClose).toHaveBeenCalled();
  });

  it("× button dismisses + sets onboarded=true so overlay doesn't return", async () => {
    const { onClose } = setup();
    const closeBtn = screen.getByTitle("跳过引导");
    await act(async () => { fireEvent.click(closeBtn); });
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith(
      expect.objectContaining({ onboarded: true }),
    ));
    expect(onClose).toHaveBeenCalled();
  });

});
