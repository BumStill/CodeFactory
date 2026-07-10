// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";

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

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders the AI Coding OS snapshot sections", async () => {
    const snapshot = {
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
    };
    mocks.invoke.mockImplementation(async () => {
      await new Promise((resolve) => setTimeout(resolve, 25));
      return snapshot;
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    expect(screen.getByText("加载控制面…")).toBeInTheDocument();
    expect(await screen.findByText("Authority Surfaces")).toBeInTheDocument();
    expect(mocks.invoke).toHaveBeenCalledWith("get_control_plane_snapshot", {
      cwd: "/Users/leo/Projects/CodeFactory",
    });

    expect(screen.getByText("AI Coding OS")).toBeInTheDocument();
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

  it("renders a partial Git observation without calling it a non-Git project", async () => {
    mocks.invoke.mockResolvedValue({
      generated_at: "2026-07-10T02:00:00Z",
      cwd: "/Users/leo/Projects/slow-repo",
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
        git_branch: null,
        is_dirty: false,
        dirty_count: 0,
        sync_gate_present: true,
        sync_gate_configured: false,
        release_workflow_present: true,
        auto_release_present: true,
        latest_release_tag: null,
        git_probe: {
          status: "partial",
          timeout_ms: 2000,
          timed_out: ["repository"],
          failed: [],
        },
      },
      risks: [
        {
          id: "git-probe-partial",
          severity: "warning",
          message: "Git observation is partial; timed out: repository.",
        },
      ],
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    expect((await screen.findAllByText("Git 状态部分可用")).length).toBeGreaterThan(0);
    expect(screen.getByText("partial · repository timed out")).toBeInTheDocument();
    expect(screen.getByText("git-probe-partial")).toBeInTheDocument();
    expect(screen.queryByText("not a git repo")).not.toBeInTheDocument();
    expect(screen.queryByText("clean")).not.toBeInTheDocument();
    expect(screen.queryByText("not configured")).not.toBeInTheDocument();
    expect(screen.queryByText("加载控制面…")).not.toBeInTheDocument();
  });

  it("reenables refresh after the watchdog and ignores the late stale response", async () => {
    vi.useFakeTimers();
    let resolveFirst: ((value: unknown) => void) | undefined;
    const staleSnapshot = {
      generated_at: "2026-07-10T02:00:00Z",
      cwd: "/Users/leo/Projects/slow-repo",
      authority: [],
      memory: { pending: 0, accepted: 0, rejected: 0, preference_pending: 0, latest_pending: [] },
      capabilities: [],
      delivery: {
        git_branch: null,
        is_dirty: null,
        dirty_count: null,
        sync_gate_present: true,
        sync_gate_configured: null,
        release_workflow_present: true,
        auto_release_present: true,
        latest_release_tag: null,
        git_probe: {
          status: "partial",
          timeout_ms: 2000,
          timed_out: ["repository"],
          failed: [],
        },
      },
      risks: [{ id: "git-probe-partial", severity: "warning", message: "stale" }],
    };
    const recoveredSnapshot = {
      ...staleSnapshot,
      generated_at: "2026-07-10T02:01:00Z",
      delivery: {
        ...staleSnapshot.delivery,
        git_branch: "main",
        is_dirty: false,
        dirty_count: 0,
        sync_gate_configured: true,
        latest_release_tag: "v1.42.6",
        git_probe: {
          status: "ok",
          timeout_ms: 2000,
          timed_out: [],
          failed: [],
        },
      },
      risks: [],
    };
    mocks.invoke
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockResolvedValueOnce(recoveredSnapshot);

    render(<ControlPlanePage onBack={vi.fn()} />);
    expect(screen.getByText("加载控制面…")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(8_000);
    });

    expect(screen.getByText("控制面请求超过 8 秒，请重试。")).toBeInTheDocument();
    const refresh = screen.getByRole("button", { name: "刷新" });
    expect(refresh).toBeEnabled();

    fireEvent.click(refresh);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getAllByText("main").length).toBeGreaterThan(0);
    expect(screen.getByText("complete")).toBeInTheDocument();

    await act(async () => {
      resolveFirst?.(staleSnapshot);
      await Promise.resolve();
    });
    expect(screen.getAllByText("main").length).toBeGreaterThan(0);
    expect(screen.queryByText("git-probe-partial")).not.toBeInTheDocument();
  });
});
