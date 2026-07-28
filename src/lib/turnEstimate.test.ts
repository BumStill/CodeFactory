// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import type { TurnPlan, TurnTimingProfile } from "./chatPlan";
import { estimateTurnRemaining } from "./turnEstimate";

const plan: TurnPlan = {
  rootTurnId: "root-1",
  revision: 2,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  createdAt: 100,
  steps: [
    { id: "inspect", title: "确认现状", kind: "analysis", status: "completed" },
    { id: "build", title: "构建应用", kind: "verification", status: "in_progress" },
    { id: "release", title: "交付结果", kind: "delivery", status: "pending" },
  ],
};

function profile(samples: number): TurnTimingProfile {
  return {
    phases: {
      verification: { sampleCount: samples, p25Ms: 120_000, p75Ms: 240_000 },
      delivery: { sampleCount: samples, p25Ms: 60_000, p75Ms: 180_000 },
    },
    build: { sampleCount: samples, p25Ms: 100_000, p75Ms: 220_000 },
    externalJob: { sampleCount: samples, p25Ms: 300_000, p75Ms: 600_000 },
  };
}

describe("turn remaining-time estimator", () => {
  it("shows no estimate when relevant history has fewer than three samples", () => {
    expect(estimateTurnRemaining(plan, profile(2), [], 1_000)).toBeNull();
  });

  it("sums deterministic phase ranges and exposes their sample sources", () => {
    const estimate = estimateTurnRemaining(plan, profile(5), [], 1_000);

    expect(estimate).toEqual({
      lowMs: 180_000,
      highMs: 420_000,
      sources: [
        { kind: "verification", sampleCount: 5 },
        { kind: "delivery", sampleCount: 5 },
      ],
    });
  });

  it("uses an active external job's elapsed time without inventing a percentage", () => {
    const external: TurnPlan = {
      ...plan,
      steps: [
        {
          id: "ci",
          title: "等待 CI",
          kind: "external_job",
          status: "in_progress",
          externalJobId: "job-1",
        },
      ],
    };
    const now = 500_000;
    const estimate = estimateTurnRemaining(
      external,
      profile(4),
      [{ id: "job-1", status: "running", startedAt: now - 120_000 }],
      now,
    );

    expect(estimate?.lowMs).toBe(180_000);
    expect(estimate?.highMs).toBe(480_000);
    expect(estimate?.sources).toEqual([{ kind: "external_job", sampleCount: 4 }]);
  });
});
