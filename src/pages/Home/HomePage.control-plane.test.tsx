// SPDX-License-Identifier: Apache-2.0
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HomePage } from "./HomePage";

const mocks = vi.hoisted(() => ({
  loadSessions: vi.fn(),
  createSession: vi.fn(),
  setTheme: vi.fn(),
  listQuickSessions: vi.fn(),
  createQuickSession: vi.fn(),
}));

vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({
    sessions: [],
    loadSessions: mocks.loadSessions,
    createSession: mocks.createSession,
    activeModel: "anthropic/claude-opus-4-7",
  }),
}));

vi.mock("../../stores/settings", () => ({
  useSettingsStore: () => ({
    settings: { theme: "dark" },
    setTheme: mocks.setTheme,
  }),
}));

vi.mock("../../lib/tauri", async (orig) => {
  const real = await orig<typeof import("../../lib/tauri")>();
  return {
    ...real,
    listQuickSessions: mocks.listQuickSessions,
    createQuickSession: mocks.createQuickSession,
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

describe("HomePage AI Coding OS entry", () => {
  beforeEach(() => {
    mocks.loadSessions.mockReset();
    mocks.createSession.mockReset();
    mocks.setTheme.mockReset();
    mocks.listQuickSessions.mockReset();
    mocks.listQuickSessions.mockResolvedValue([]);
    mocks.createQuickSession.mockReset();
  });

  it("opens the control plane from the top bar", async () => {
    const onOpenControlPlane = vi.fn();

    render(
      <HomePage
        onOpenProject={() => {}}
        onOpenSkills={() => {}}
        onOpenControlPlane={onOpenControlPlane}
        onOpenSettings={() => {}}
        onOpenProfile={() => {}}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "AI Coding OS" }));

    expect(onOpenControlPlane).toHaveBeenCalledTimes(1);
  });
});
