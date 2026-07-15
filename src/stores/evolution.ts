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

export interface EvolutionCandidateState {
  candidate_id: string;
  source_learning_event_id: string | null;
  cwd: string;
  kind: "memory" | "pattern" | "preference";
  revision: number;
  state: "approved" | "eval_failed" | "eval_error" | "eval_stale" | "pending_activation" | "active" | "rolled_back" | "rollback_conflict";
  state_version: number;
  suggestion: string;
  pref_key: string | null;
  pref_value: string | null;
  payload_hash: string;
  auto_activate: boolean;
  eval_run_id: string | null;
  eval_status: "running" | "passed" | "failed" | "inconclusive" | "error" | null;
  eval_manifest_hash: string | null;
  eval_required_count: number;
  eval_passed_count: number;
  eval_failed_count: number;
  activation_id: string | null;
  activation_status: "active" | "rolled_back" | "rollback_conflict" | null;
  activated_at: string | null;
  rolled_back_at: string | null;
  updated_at: string;
}

export interface EvolutionEvalCaseResult {
  id: string;
  run_id: string;
  case_id: string;
  title: string;
  status: string;
  hard_gate: boolean;
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

export async function listEvolutionCandidateStates(cwd: string): Promise<EvolutionCandidateState[]> {
  return invoke<EvolutionCandidateState[]>("list_evolution_candidate_states", { cwd });
}

export async function listEvolutionEvalCaseResults(
  cwd: string,
  runId: string,
): Promise<EvolutionEvalCaseResult[]> {
  return invoke<EvolutionEvalCaseResult[]>("list_evolution_eval_case_results", { cwd, runId });
}

export async function rerunEvolutionEval(cwd: string, candidateId: string): Promise<EvolutionCandidateState> {
  return invoke<EvolutionCandidateState>("rerun_evolution_eval", { cwd, candidateId });
}

export async function activateEvolutionCandidate(cwd: string, candidateId: string): Promise<EvolutionCandidateState> {
  return invoke<EvolutionCandidateState>("activate_evolution_candidate", { cwd, candidateId });
}

export async function rollbackEvolutionActivation(cwd: string, activationId: string): Promise<EvolutionCandidateState> {
  return invoke<EvolutionCandidateState>("rollback_evolution_activation", { cwd, activationId });
}
