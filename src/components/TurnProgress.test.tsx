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
    const stopped: TurnPlan = { ...plan, waitingReason: "technical_recovery_exhausted" };
    render(
      <TurnProgress plan={stopped} timingProfile={timing} externalJobs={[]} elapsedMs={90_000} />,
    );

    const banner = screen.getByTestId("turn-progress");
    expect(banner).not.toHaveTextContent("technical_recovery_exhausted");
    expect(banner).toHaveTextContent(/自动恢复/);
  });

  it("stops quoting a remaining time once the turn is no longer running", () => {
    const stopped: TurnPlan = { ...plan, waitingReason: "technical_recovery_exhausted" };
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
