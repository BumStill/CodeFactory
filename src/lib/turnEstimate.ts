// SPDX-License-Identifier: Apache-2.0

import type {
  DurationSampleSummary,
  ExternalJobState,
  PlanStep,
  PlanStepKind,
  TurnPlan,
  TurnTimingProfile,
} from "./chatPlan";

export interface TurnEstimateSource {
  kind: PlanStepKind;
  sampleCount: number;
}

export interface TurnRemainingEstimate {
  lowMs: number;
  highMs: number;
  sources: TurnEstimateSource[];
}

function sampleForStep(
  step: PlanStep,
  profile: TurnTimingProfile,
): DurationSampleSummary | null {
  const phase = profile.phases[step.kind];
  if (phase) return phase;
  if (step.kind === "verification") return profile.build;
  if (step.kind === "external_job") return profile.externalJob;
  return null;
}

export function estimateTurnRemaining(
  plan: TurnPlan,
  profile: TurnTimingProfile | null,
  externalJobs: ExternalJobState[],
  nowMs: number,
): TurnRemainingEstimate | null {
  if (!profile) return null;
  const remaining = plan.steps.filter((step) => step.status !== "completed");
  if (remaining.length === 0) return null;

  let lowMs = 0;
  let highMs = 0;
  const sources: TurnEstimateSource[] = [];
  for (const step of remaining) {
    const sample = sampleForStep(step, profile);
    if (!sample || sample.sampleCount < 3) return null;

    let stepLow = sample.p25Ms;
    let stepHigh = sample.p75Ms;
    if (step.kind === "external_job" && step.externalJobId) {
      const job = externalJobs.find((candidate) => candidate.id === step.externalJobId);
      if (job?.status === "running" && job.startedAt != null) {
        const elapsed = Math.max(0, nowMs - job.startedAt);
        stepLow = Math.max(0, stepLow - elapsed);
        stepHigh = Math.max(0, stepHigh - elapsed);
      }
    }
    lowMs += stepLow;
    highMs += stepHigh;
    sources.push({ kind: step.kind, sampleCount: sample.sampleCount });
  }

  return { lowMs, highMs: Math.max(lowMs, highMs), sources };
}
