// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("../../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

const fakeChatState = {
  activeSession: { id: "s1", cwd: "/Users/leo/Projects/CodeFactory", title: "CodeFactory" },
};

vi.mock("../../stores/chat", () => ({
  useChatStore: <T,>(selector: (s: typeof fakeChatState) => T): T => selector(fakeChatState),
}));

import { ControlPlanePage } from "./ControlPlanePage";

describe("ControlPlanePage", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
  });

  it("renders the AI Coding OS snapshot sections", async () => {
    mocks.invoke.mockResolvedValue({
      generated_at: "2026-06-26T13:03:18Z",
      cwd: "/Users/leo/Projects/CodeFactory",
      authority: [
        {
          id: "agents-md",
          label: "AGENTS.md",
          status: "ok",
          path: "/Users/leo/Projects/CodeFactory/AGENTS.md",
          detail: "Project agent rules are present.",
        },
      ],
      memory: {
        pending: 2,
        accepted: 3,
        rejected: 1,
        preference_pending: 1,
        latest_pending: ["Keep release evidence visible."],
      },
      capabilities: [
        {
          id: "skills",
          label: "Skills",
          total: 4,
          enabled: 2,
          status: "ok",
          detail: "Prompt and slash-command capability packs.",
        },
      ],
      delivery: {
        git_branch: "codex/ai-coding-os-control-plane",
        is_dirty: false,
        dirty_count: 0,
        sync_gate_present: true,
        sync_gate_configured: true,
        release_workflow_present: true,
        auto_release_present: true,
        latest_release_tag: "v1.39.1",
      },
      risks: [],
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("get_control_plane_snapshot", {
        cwd: "/Users/leo/Projects/CodeFactory",
      }),
    );

    expect(screen.getByText("AI Coding OS")).toBeInTheDocument();
    expect(screen.getByText("Authority Surfaces")).toBeInTheDocument();
    expect(screen.getByText("Memory Lifecycle")).toBeInTheDocument();
    expect(screen.getByText("Capability Registry")).toBeInTheDocument();
    expect(screen.getByText("Delivery Gates")).toBeInTheDocument();
    expect(screen.getByText("AGENTS.md")).toBeInTheDocument();
    expect(screen.getByText("Keep release evidence visible.")).toBeInTheDocument();
    expect(screen.getByText("v1.39.1")).toBeInTheDocument();
  });

  it("shows when the sync hook exists but is not configured for this checkout", async () => {
    mocks.invoke.mockResolvedValue({
      generated_at: "2026-06-26T13:03:18Z",
      cwd: "/Users/leo/Projects/CodeFactory",
      authority: [],
      memory: {
        pending: 0,
        accepted: 0,
        rejected: 0,
        preference_pending: 0,
        latest_pending: [],
      },
      capabilities: [],
      delivery: {
        git_branch: "main",
        is_dirty: false,
        dirty_count: 0,
        sync_gate_present: true,
        sync_gate_configured: false,
        release_workflow_present: true,
        auto_release_present: true,
        latest_release_tag: "v1.39.1",
      },
      risks: [
        {
          id: "sync-gate-not-configured",
          severity: "warning",
          message: "Versioned pre-commit hook exists but this checkout is not using it.",
        },
      ],
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    expect(await screen.findByText("Sync hook config")).toBeInTheDocument();
    expect(screen.getByText("not configured")).toBeInTheDocument();
    expect(screen.getByText("sync-gate-not-configured")).toBeInTheDocument();
  });
});
