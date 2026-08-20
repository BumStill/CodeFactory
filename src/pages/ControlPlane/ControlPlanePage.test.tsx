// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen } from "@testing-library/react";

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

  it("automatically reobserves after the watchdog and ignores the late stale response", async () => {
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
      .mockResolvedValueOnce(recoveredSnapshot)
      .mockResolvedValueOnce({
        generated_at_ms: 100_000_000,
        window_start_ms: 13_600_000,
        availability: "available",
        unavailable_reason: null,
        metrics: {
          open: 0,
          system_owned: 0,
          typed_user_attention: 0,
          invalid_user_attention_requests: 0,
          technical_user_handoff_violations: 0,
          overdue_ownerless_remediations: 0,
          invalid_completions: 0,
          invalid_terminal_convergences: 0,
          duplicate_committed_side_effect_receipts: 0,
          recovery_decisions: 0,
          recovered_objectives: 0,
          recovery_latency_p50_ms: null,
          recovery_latency_p95_ms: null,
          recovery_decisions_24h: 0,
          recovered_objectives_24h: 0,
          recovery_latency_p50_ms_24h: null,
          recovery_latency_p95_ms_24h: null,
        },
      });

    render(<ControlPlanePage onBack={vi.fn()} />);
    expect(screen.getByText("加载控制面…")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(8_000);
    });

    expect(
      screen.getByText("控制面请求超过 8 秒；观测状态已保留，系统将在 3 秒后自动重新观测。"),
    ).toBeInTheDocument();
    const refresh = screen.getByRole("button", { name: "刷新" });
    expect(refresh).toBeEnabled();
    expect(mocks.invoke).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_999);
    });
    expect(mocks.invoke).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(mocks.invoke).toHaveBeenCalledTimes(3);
    expect(mocks.invoke).toHaveBeenNthCalledWith(3, "get_objective_health");
    expect(screen.getAllByText("main").length).toBeGreaterThan(0);
    expect(screen.getByText("complete")).toBeInTheDocument();

    await act(async () => {
      resolveFirst?.(staleSnapshot);
      await Promise.resolve();
    });
    expect(screen.getAllByText("main").length).toBeGreaterThan(0);
    expect(screen.queryByText("git-probe-partial")).not.toBeInTheDocument();
  });

  it("renders Objective continuity health and marks technical violations as risks", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_objective_health") {
        return {
          generated_at_ms: 100_000_000,
          window_start_ms: 13_600_000,
          build_git_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          availability: "available",
          unavailable_reason: null,
          metrics: {
            open: 7,
            system_owned: 5,
            typed_user_attention: 2,
            invalid_user_attention_requests: 1,
            technical_user_handoff_violations: 3,
            technical_user_handoff_violations_24h: 2,
            avoidable_user_reprompts_24h: 1,
            overdue_ownerless_remediations: 1,
            stalled_system_owned_objectives: 2,
            unavailable_domain_adapter_objectives: 4,
            invalid_completions: 2,
            invalid_completions_24h: 1,
            invalid_terminal_convergences: 1,
            duplicate_committed_side_effect_receipts: 1,
            duplicate_committed_side_effect_receipts_24h: 1,
            requested_ceiling_downgrades_24h: 1,
            recovery_decisions: 9,
            recovered_objectives: 8,
            recovery_latency_p50_ms: 1200,
            recovery_latency_p95_ms: 4800,
            recovery_decisions_24h: 4,
            recovered_objectives_24h: 3,
            recovery_latency_p50_ms_24h: 900,
            recovery_latency_p95_ms_24h: 2400,
          },
        };
      }
      return {
        generated_at: "2026-07-10T02:00:00Z",
        cwd: "/Users/leo/Projects/CodeFactory",
        authority: [],
        memory: { pending: 0, accepted: 0, rejected: 0, preference_pending: 0, latest_pending: [] },
        capabilities: [],
        delivery: {
          git_branch: "main",
          is_dirty: false,
          dirty_count: 0,
          sync_gate_present: true,
          sync_gate_configured: true,
          release_workflow_present: true,
          auto_release_present: true,
          latest_release_tag: "v1.42.6",
        },
        risks: [],
      };
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    expect(await screen.findByText("Objective Continuity")).toBeInTheDocument();
    expect(screen.getByTestId("objective-release-gate")).toHaveAttribute(
      "data-status",
      "blocked",
    );
    expect(screen.getByText("24h non-interruption gate blocked")).toBeInTheDocument();
    expect(screen.getByText(/Build aaaaaaaaaaaa/)).toBeInTheDocument();
    expect(screen.getByTestId("objective-open")).toHaveTextContent("7");
    expect(screen.getByTestId("objective-system-owned")).toHaveTextContent("5");
    expect(screen.getByTestId("objective-typed-attention")).toHaveTextContent("2");
    expect(screen.getByTestId("objective-technical-handoffs")).toHaveAttribute(
      "data-severity",
      "risk",
    );
    expect(screen.getByTestId("objective-ownerless-remediations")).toHaveAttribute(
      "data-severity",
      "risk",
    );
    expect(screen.getByTestId("objective-stalled-system-owned")).toHaveTextContent("2");
    expect(screen.getByTestId("objective-stalled-system-owned")).toHaveAttribute(
      "data-severity",
      "risk",
    );
    expect(screen.getByTestId("objective-unavailable-adapters")).toHaveTextContent("4");
    expect(screen.getByTestId("objective-unavailable-adapters")).toHaveAttribute(
      "data-severity",
      "risk",
    );
    expect(screen.getByTestId("objective-invalid-completions")).toHaveAttribute(
      "data-severity",
      "risk",
    );
    expect(screen.getByTestId("objective-duplicate-receipts")).toHaveAttribute(
      "data-severity",
      "risk",
    );
    expect(screen.getByTestId("objective-24h-technical-handoffs")).toHaveTextContent("2");
    expect(screen.getByTestId("objective-24h-avoidable-reprompts")).toHaveTextContent("1");
    expect(screen.getByTestId("objective-24h-duplicate-receipts")).toHaveTextContent("1");
    expect(screen.getByTestId("objective-24h-ceiling-downgrades")).toHaveTextContent("1");
    expect(screen.getByText("3 / 4")).toBeInTheDocument();
    expect(screen.getByText("0.9s")).toBeInTheDocument();
    expect(screen.getByText("2.4s")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /retry|continue|重试|继续/i })).not.toBeInTheDocument();
  });

  it("blocks an identified zero-violation build until its full production window is observed", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_objective_health") {
        return {
          generated_at_ms: 100_000_000,
          window_start_ms: 13_600_000,
          build_git_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          build_observation_started_at_ms: 99_000_000,
          production_window_covered: false,
          availability: "available",
          unavailable_reason: null,
          metrics: {
            open: 0,
            system_owned: 0,
            typed_user_attention: 0,
            invalid_user_attention_requests: 0,
            technical_user_handoff_violations: 0,
            technical_user_handoff_violations_24h: 0,
            avoidable_user_reprompts_24h: 0,
            overdue_ownerless_remediations: 0,
            stalled_system_owned_objectives: 0,
            unavailable_domain_adapter_objectives: 0,
            invalid_completions: 0,
            invalid_completions_24h: 0,
            invalid_terminal_convergences: 0,
            duplicate_committed_side_effect_receipts: 0,
            duplicate_committed_side_effect_receipts_24h: 0,
            requested_ceiling_downgrades_24h: 0,
            recovery_decisions: 0,
            recovered_objectives: 0,
            recovery_latency_p50_ms: null,
            recovery_latency_p95_ms: null,
            recovery_decisions_24h: 0,
            recovered_objectives_24h: 0,
            recovery_latency_p50_ms_24h: null,
            recovery_latency_p95_ms_24h: null,
          },
        };
      }
      return {
        generated_at: "2026-07-10T02:00:00Z",
        cwd: "/Users/leo/Projects/CodeFactory",
        authority: [],
        memory: { pending: 0, accepted: 0, rejected: 0, preference_pending: 0, latest_pending: [] },
        capabilities: [],
        delivery: {
          git_branch: "main",
          is_dirty: false,
          dirty_count: 0,
          sync_gate_present: true,
          sync_gate_configured: true,
          release_workflow_present: true,
          auto_release_present: true,
          latest_release_tag: null,
        },
        risks: [],
      };
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    expect(await screen.findByText("Objective Continuity")).toBeInTheDocument();
    expect(screen.getByTestId("objective-release-gate")).toHaveAttribute(
      "data-status",
      "blocked",
    );
    expect(screen.getByText("24h non-interruption gate blocked")).toBeInTheDocument();
    expect(screen.getByText(/production observation window incomplete/i)).toBeInTheDocument();
    expect(screen.getByText(/Build bbbbbbbbbbbb/)).toBeInTheDocument();
  });

  it("shows Objective health as unavailable without rendering missing metrics as zero", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_objective_health") {
        return {
          generated_at_ms: 100_000_000,
          window_start_ms: 13_600_000,
          build_git_sha: null,
          availability: "unavailable",
          unavailable_reason: "objective health unavailable: missing required schema: objectives",
          metrics: null,
        };
      }
      return {
        generated_at: "2026-07-10T02:00:00Z",
        cwd: "/Users/leo/Projects/CodeFactory",
        authority: [],
        memory: { pending: 0, accepted: 0, rejected: 0, preference_pending: 0, latest_pending: [] },
        capabilities: [],
        delivery: {
          git_branch: "main",
          is_dirty: false,
          dirty_count: 0,
          sync_gate_present: true,
          sync_gate_configured: true,
          release_workflow_present: true,
          auto_release_present: true,
          latest_release_tag: null,
        },
        risks: [],
      };
    });

    render(<ControlPlanePage onBack={vi.fn()} />);

    expect(await screen.findByText("Objective Continuity")).toBeInTheDocument();
    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    expect(
      screen.getByText("objective health unavailable: missing required schema: objectives"),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("objective-open")).not.toBeInTheDocument();
    expect(screen.queryByTestId("objective-recovery-24h")).not.toBeInTheDocument();
  });
});
