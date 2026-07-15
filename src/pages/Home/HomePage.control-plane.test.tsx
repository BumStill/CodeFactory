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
  loadLearning: vi.fn(async () => {}),
}));

vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({
    sessions: [{
      id: "project-1",
      title: "Project",
      cwd: "/proj",
      model_id: "test",
      created_at: 1,
      updated_at: 2,
      total_input_tokens: 0,
      total_output_tokens: 0,
      kind: "project",
    }],
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

vi.mock("../../stores/learning", () => ({
  useLearningStore: (selector: (state: unknown) => unknown) => selector({
    events: {
      "/proj": [{ id: "candidate-1", status: "pending" }],
    },
    load: mocks.loadLearning,
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
    mocks.loadLearning.mockClear();
  });

  it("opens the control plane from the top bar", async () => {
    const onOpenControlPlane = vi.fn();

    render(
      <HomePage
        onOpenProject={() => {}}
        onOpenSkills={() => {}}
        onOpenControlPlane={onOpenControlPlane}
        onOpenBenchmarks={() => {}}
        onOpenSettings={() => {}}
        onOpenProfile={() => {}}
        onOpenEvolution={() => {}}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "AI Coding OS" }));

    expect(onOpenControlPlane).toHaveBeenCalledTimes(1);
  });

  it("opens the benchmark page from the primary entries", async () => {
    const onOpenBenchmarks = vi.fn();

    render(
      <HomePage
        onOpenProject={() => {}}
        onOpenSkills={() => {}}
        onOpenControlPlane={() => {}}
        onOpenBenchmarks={onOpenBenchmarks}
        onOpenSettings={() => {}}
        onOpenProfile={() => {}}
        onOpenEvolution={() => {}}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: /能力评测/ }));

    expect(onOpenBenchmarks).toHaveBeenCalledTimes(1);
  });

  it("opens the explicit evolution review workbench from the primary entries", async () => {
    const onOpenEvolution = vi.fn();

    render(
      <HomePage
        onOpenProject={() => {}}
        onOpenSkills={() => {}}
        onOpenControlPlane={() => {}}
        onOpenBenchmarks={() => {}}
        onOpenSettings={() => {}}
        onOpenProfile={() => {}}
        onOpenEvolution={onOpenEvolution}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: /进化审查/ }));

    expect(onOpenEvolution).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("1 待审")).toBeInTheDocument();
  });
});
