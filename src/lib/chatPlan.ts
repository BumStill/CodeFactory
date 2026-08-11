// SPDX-License-Identifier: Apache-2.0

export type PlanStepKind =
  | "analysis"
  | "implementation"
  | "verification"
  | "delivery"
  | "external_job"
  | "other";

export type PlanStepStatus = "pending" | "in_progress" | "completed";
export type NextActionOwner = "system" | "external" | "user";

export interface PlanStep {
  id: string;
  title: string;
  kind: PlanStepKind;
  status: PlanStepStatus;
  externalJobId?: string | null;
}

export interface TurnPlan {
  rootTurnId: string;
  revision: number;
  steps: PlanStep[];
  explanation: string | null;
  waitingReason: string | null;
  /** Missing legacy state is fail-safe system-owned. */
  nextActionOwner?: NextActionOwner;
  changeReason: string | null;
  waitingHistory?: string[];
  changeHistory?: string[];
  createdAt: number;
}

export interface DurationSampleSummary {
  sampleCount: number;
  p25Ms: number;
  p75Ms: number;
}

export interface TurnTimingProfile {
  phases: Partial<Record<PlanStepKind, DurationSampleSummary>>;
  build: DurationSampleSummary | null;
  externalJob: DurationSampleSummary | null;
}

export interface ExternalJobState {
  id: string;
  status: string;
  startedAt?: number | null;
  completedAt?: number | null;
}

export interface PlanProgress {
  completed: number;
  total: number;
  percent: number;
  current: PlanStep | null;
  next: PlanStep | null;
}

export function planProgress(plan: TurnPlan): PlanProgress {
  const completed = plan.steps.filter((step) => step.status === "completed").length;
  const currentIndex = plan.steps.findIndex((step) => step.status === "in_progress");
  const current = currentIndex >= 0 ? plan.steps[currentIndex] : null;
  const next =
    plan.steps.find(
      (step, index) =>
        step.status === "pending" && (currentIndex < 0 || index > currentIndex),
    ) ?? null;
  const total = plan.steps.length;
  return {
    completed,
    total,
    percent: total === 0 ? 0 : Math.round((completed / total) * 100),
    current,
    next,
  };
}

export function normalizeNextActionOwner(value: unknown): NextActionOwner {
  return value === "external" || value === "user" ? value : "system";
}

export function turnPlanFromEvent(event: {
  root_turn_id: string;
  revision: number;
  steps: Array<{
    id: string;
    title: string;
    kind: PlanStepKind;
    status: PlanStepStatus;
    external_job_id?: string | null;
  }>;
  explanation?: string | null;
  waiting_reason?: string | null;
  next_action_owner?: NextActionOwner | null;
  change_reason?: string | null;
  waiting_history?: string[];
  change_history?: string[];
  created_at: number;
}): TurnPlan {
  return {
    rootTurnId: event.root_turn_id,
    revision: event.revision,
    steps: event.steps.map((step) => ({
      id: step.id,
      title: step.title,
      kind: step.kind,
      status: step.status,
      externalJobId: step.external_job_id ?? null,
    })),
    explanation: event.explanation ?? null,
    waitingReason: event.waiting_reason ?? null,
    nextActionOwner: normalizeNextActionOwner(event.next_action_owner),
    changeReason: event.change_reason ?? null,
    waitingHistory: event.waiting_history ?? (
      event.waiting_reason ? [event.waiting_reason] : []
    ),
    changeHistory: event.change_history ?? (
      event.change_reason ? [event.change_reason] : []
    ),
    createdAt: event.created_at,
  };
}
