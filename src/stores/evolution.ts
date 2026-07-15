// SPDX-License-Identifier: Apache-2.0

import { invoke } from "../lib/tauri";

export type EvolutionJobStatus =
  | "queued"
  | "running"
  | "partial"
  | "succeeded"
  | "no_candidates"
  | "failed"
  | "cancelled";

export interface EvolutionJob {
  id: string;
  cwd: string;
  trigger: string;
  candidate_id?: string | null;
  status: EvolutionJobStatus;
  input_session_count: number;
  input_trace_count: number;
  candidate_count: number;
  started_at: string;
  completed_at: string | null;
  error: string | null;
}

export interface EvolutionJobEvent {
  id: string;
  cwd: string;
  job_id: string;
  candidate_id: string | null;
  stage: string;
  status: string;
  title: string;
  detail_json: string;
  created_at: string;
}

export async function listEvolutionJobs(cwd: string): Promise<EvolutionJob[]> {
  return invoke<EvolutionJob[]>("list_evolution_jobs", { cwd });
}

export async function listEvolutionDecisionJobs(cwd: string): Promise<EvolutionJob[]> {
  return invoke<EvolutionJob[]>("list_evolution_decision_jobs", { cwd });
}

export async function getEvolutionJob(cwd: string, jobId: string): Promise<EvolutionJob> {
  return invoke<EvolutionJob>("get_evolution_job", { cwd, jobId });
}

export async function listEvolutionJobEvents(
  cwd: string,
  jobId?: string,
): Promise<EvolutionJobEvent[]> {
  return invoke<EvolutionJobEvent[]>("list_evolution_job_events", {
    cwd,
    jobId: jobId ?? null,
  });
}
