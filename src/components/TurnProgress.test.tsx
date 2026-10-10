// SPDX-License-Identifier: Apache-2.0

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { TurnPlan, TurnTimingProfile } from "../lib/chatPlan";
import { TurnProgress } from "./TurnProgress";

const plan: TurnPlan = {
  rootTurnId: "root",
  revision: 4,
  explanation: "开始验证",
  waitingReason: "等待 Windows 构建",
  changeReason: "发现发布前需要补一轮安装 smoke",
  createdAt: 100,
  steps: [
    { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
    { id: "implement", title: "实现修改", kind: "implementation", status: "completed" },
    { id: "verify", title: "验证四视口", kind: "verification", status: "in_progress" },
    { id: "deliver", title: "交付 PR", kind: "delivery", status: "pending" },
  ],
};

const timing: TurnTimingProfile = {
  phases: {
    verification: { sampleCount: 6, p25Ms: 120_000, p75Ms: 240_000 },
    delivery: { sampleCount: 4, p25Ms: 60_000, p75Ms: 180_000 },
  },
  build: null,
  externalJob: null,
};

describe("TurnProgress", () => {
  it("shows sourced progress, current/next steps, waiting and plan changes", () => {
    render(<TurnProgress plan={plan} timingProfile={timing} externalJobs={[]} elapsedMs={90_000} />);

    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText("已完成 2/4")).toBeInTheDocument();
    expect(screen.getByText("50%")).toBeInTheDocument();
    expect(screen.getByText(/当前 · 验证四视口/)).toBeInTheDocument();
    expect(screen.getByText(/下一步 · 交付 PR/)).toBeInTheDocument();
    expect(screen.getByText(/来自 4 个计划步骤/)).toBeInTheDocument();
    expect(screen.getByText(/预计还需 3–7 分钟/)).toBeInTheDocument();
    expect(screen.getByText(/最少 4 个历史样本/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "展开执行路线" }));
    expect(screen.getByText(/等待 Windows 构建/)).toBeInTheDocument();
    expect(screen.getByText(/发现发布前需要补一轮安装 smoke/)).toBeInTheDocument();
  });

  it("renders a waiting reason as human text and never as the raw internal code", () => {
    const stopped: TurnPlan = { ...plan, waitingReason: "objective_failed" };
    render(
      <TurnProgress plan={stopped} timingProfile={timing} externalJobs={[]} elapsedMs={90_000} />,
    );

    const banner = screen.getByTestId("turn-progress");
    expect(banner).not.toHaveTextContent("objective_failed");
    expect(banner).toHaveTextContent(/这件事没做成/);
  });

  it("stops quoting a remaining time once the turn is no longer running", () => {
    const stopped: TurnPlan = { ...plan, waitingReason: "objective_failed" };
    render(
      <TurnProgress plan={stopped} timingProfile={timing} externalJobs={[]} elapsedMs={90_000} />,
    );

    expect(screen.queryByText(/预计还需/)).not.toBeInTheDocument();
    expect(screen.queryByText(/个历史样本/)).not.toBeInTheDocument();
  });

  it("omits the time estimate when the sample profile is unavailable", () => {
    render(<TurnProgress plan={plan} timingProfile={null} externalJobs={[]} elapsedMs={90_000} />);
    expect(screen.queryByText(/预计还需/)).not.toBeInTheDocument();
  });

  // M37: real progress only when the plan is actually tracked.
  it("hides the step count and the percentage while the plan is not tracked", () => {
    const untracked: TurnPlan = {
      ...plan,
      waitingReason: null,
      steps: plan.steps.map((step) => ({ ...step, status: "pending" })),
    };
    render(
      <TurnProgress plan={untracked} timingProfile={timing} externalJobs={[]} elapsedMs={449_000} />,
    );

    const bar = screen.getByTestId("turn-progress");
    expect(bar).not.toHaveTextContent("0/4");
    expect(bar).not.toHaveTextContent("0%");
    expect(bar).not.toHaveTextContent(/个计划步骤/);
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    // Elapsed time and the current activity are the real content left.
    expect(bar).toHaveTextContent("7m29s");
    expect(bar).toHaveTextContent(/当前 · /);
    expect(bar).toHaveAttribute("data-status-tone", "progress");
  });

  it("shows completed/total and the percentage once the plan is tracked", () => {
    render(<TurnProgress plan={plan} timingProfile={timing} externalJobs={[]} elapsedMs={90_000} />);

    expect(screen.getByTestId("turn-progress")).toHaveTextContent("已完成 2/4");
    expect(screen.getByTestId("turn-progress")).toHaveTextContent("50%");
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
  });

  it("keeps the completion-gate verification stage off the warning tone", () => {
    render(
      <TurnProgress
        plan={{ ...plan, waitingReason: null }}
        timingProfile={timing}
        externalJobs={[]}
        elapsedMs={90_000}
        activityLabel="正在补跑检查"
        activityWaitingReason="验证证据不足"
      />,
    );

    const bar = screen.getByTestId("turn-progress");
    expect(bar).toHaveAttribute("data-status-tone", "progress");
    expect(bar).not.toHaveTextContent("验证证据不足");
    expect(bar).toHaveTextContent("正在补跑检查");
    // The bar itself must not paint the warning colour either.
    expect(bar.className).not.toContain("border-status-warning");
  });

  it.each([
    ["authorization_required", /需要你先授权才能继续/],
    ["needs_business_decision", /需要你先做一个决定才能继续/],
    ["objective_failed", /这件事没做成/],
  ] as const)("keeps a real warning for %s", (reason, expected) => {
    render(<TurnProgress plan={{ ...plan, waitingReason: reason }} timingProfile={timing} externalJobs={[]} elapsedMs={90_000} />);

    const bar = screen.getByTestId("turn-progress");
    expect(bar).toHaveAttribute("data-status-tone", "warning");
    expect(bar).toHaveTextContent(expected);
  });

  it.each([
    ["untracked", { waitingReason: null, steps: plan.steps.map((step) => ({ ...step, status: "pending" as const })) }],
    ["tracked", { waitingReason: null }],
    ["verification stage", { waitingReason: null }],
  ] as const)("never leaks internal vocabulary in the %s state", (label, patch) => {
    render(
      <TurnProgress
        plan={{ ...plan, ...patch }}
        timingProfile={timing}
        externalJobs={[]}
        elapsedMs={90_000}
        activityLabel={label === "tracked" ? null : undefined}
        activityWaitingReason="验证证据不足"
      />,
    );

    const text = screen.getByTestId("turn-progress").textContent ?? "";
    for (const word of ["证据", "复核", "当前边界", "恢复耗尽", "安全上限", "系统故障", "objective", "remediation", "recovery"]) {
      expect(text.toLowerCase()).not.toContain(word.toLowerCase());
    }
  });

  it("shows the real status of a linked external job", () => {
    const externalPlan: TurnPlan = {
      ...plan,
      steps: [
        {
          id: "ci",
          title: "等待 CI",
          kind: "external_job",
          status: "in_progress",
          externalJobId: "job-1",
        },
        { id: "deliver", title: "交付 PR", kind: "delivery", status: "pending" },
      ],
    };
    render(
      <TurnProgress
        plan={externalPlan}
        timingProfile={timing}
        externalJobs={[{ id: "job-1", status: "running", startedAt: 1 }]}
        elapsedMs={90_000}
      />,
    );

    expect(screen.getByText("外部任务 · 运行中")).toBeInTheDocument();
  });
});
