// SPDX-License-Identifier: Apache-2.0
// Lock-safe browser acceptance entry. This HTML is not part of the production
// Vite bundle; it mounts the real workbench against bounded Tauri mock IPC.

import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import "../styles/globals.css";
import type { LearningEvent } from "../stores/learning";
import type { EvolutionCandidateState, EvolutionEvalCaseResult, EvolutionJob, EvolutionJobEvent } from "../stores/evolution";
import type { Session } from "../lib/tauri";

const cwd = "/lock-safe-evolution-fixture";
const now = "2026-07-15T04:30:00Z";

const preferenceCandidate: LearningEvent = {
  id: "headless-preference",
  session_id: "",
  cwd,
  observation: "多次人工决定均要求使用简体中文交付。",
  suggestion: "将当前项目回复语言固定为简体中文。",
  status: "pending",
  created_at: now,
  decided_at: null,
  kind: "preference",
  pref_key: "response_language",
  pref_value: "zh-CN",
  support_count: 4,
  evidence_json: JSON.stringify({
    support_unit: "decisions",
    decision_count: 4,
    accepted: 4,
    accept_rate: 100,
  }),
  job_id: "headless-analysis",
};

const memoryCandidate: LearningEvent = {
  id: "headless-memory",
  session_id: "",
  cwd,
  observation: "工具读取文件在 2 个会话中重复失败 3 次。",
  suggestion: "执行读取前先确认文件路径与当前工作目录。",
  status: "pending",
  created_at: "2026-07-15T04:30:01Z",
  decided_at: null,
  kind: "pattern",
  pref_key: null,
  pref_value: null,
  support_count: 2,
  evidence_json: JSON.stringify({
    support_unit: "sessions",
    session_count: 2,
    total_calls: 11,
    errors: 3,
    rate: 27.3,
  }),
  job_id: "headless-analysis",
};

let candidates: LearningEvent[] = [preferenceCandidate, memoryCandidate];
let candidateStates: EvolutionCandidateState[] = [];
const evalCases: EvolutionEvalCaseResult[] = [
  "冻结版本完整性", "项目范围隔离", "隐私与长度合同", "低风险目标白名单",
  "Baseline 未提前生效", "Treatment 精确注入一次", "回滚准备度",
].map((title, index) => ({
  id: `headless-eval-case-${index}`,
  run_id: "headless-eval-run",
  case_id: `case-${index}`,
  title,
  status: "passed",
  hard_gate: true,
  detail_json: JSON.stringify({ reason: "ok" }),
  created_at: `2026-07-15T04:32:0${index}Z`,
}));

const analysisJob: EvolutionJob = {
  id: "headless-analysis",
  cwd,
  trigger: "cross_session",
  candidate_id: null,
  status: "succeeded",
  input_session_count: 2,
  input_trace_count: 11,
  candidate_count: 2,
  started_at: "2026-07-15T04:29:58Z",
  completed_at: "2026-07-15T04:30:02Z",
  error: null,
};

let decisionJobs: EvolutionJob[] = [];
const eventsByJob = new Map<string, EvolutionJobEvent[]>([
  [analysisJob.id, [
    ["scope", "completed", "分析范围已确定", { session_count: 2 }],
    ["trace_read", "completed", "轨迹读取完成", { session_count: 2, trace_count: 11 }],
    ["privacy", "completed", "隐私处理完成", { redacted: true }],
    ["extract", "completed", "候选提取完成", { candidate_count: 2 }],
    ["deduplicate", "completed", "候选去重完成", { candidate_count: 2 }],
    ["review", "waiting", "等待人工审核", {}],
    ["job", "completed", "分析完成", { candidate_count: 2 }],
  ].map(([stage, status, title, detail], index) => ({
    id: `headless-analysis-${index}`,
    cwd,
    job_id: analysisJob.id,
    candidate_id: null,
    stage: String(stage),
    status: String(status),
    title: String(title),
    detail_json: JSON.stringify(detail),
    created_at: `2026-07-15T04:30:0${index}.000Z`,
  }))],
]);

function reject(eventId: string) {
  candidates = candidates.map((candidate) => candidate.id === eventId
    ? { ...candidate, status: "rejected", decided_at: new Date().toISOString() }
    : candidate);
  const id = "headless-reject";
  const job: EvolutionJob = {
    id,
    cwd,
    trigger: "review_reject",
    candidate_id: eventId,
    status: "succeeded",
    input_session_count: 0,
    input_trace_count: 0,
    candidate_count: 0,
    started_at: "2026-07-15T04:31:00Z",
    completed_at: "2026-07-15T04:31:01Z",
    error: null,
  };
  decisionJobs = [job, ...decisionJobs.filter((existing) => existing.id !== id)];
  eventsByJob.set(id, [
    eventFor(id, "review", "completed", "人工已拒绝候选", 0),
    eventFor(id, "job", "completed", "拒绝决定已保存", 1),
  ]);
}

function approve(eventId: string, autoActivate: boolean) {
  const source = candidates.find((candidate) => candidate.id === eventId);
  if (!source) throw new Error("candidate not found");
  candidates = candidates.filter((candidate) => candidate.id !== eventId);
  const state: EvolutionCandidateState = {
    candidate_id: source.id,
    source_learning_event_id: source.id,
    cwd,
    kind: source.kind,
    revision: 1,
    state: autoActivate ? "active" : "pending_activation",
    state_version: autoActivate ? 4 : 3,
    suggestion: source.suggestion,
    pref_key: source.pref_key,
    pref_value: source.pref_value,
    payload_hash: "headless-payload-hash",
    auto_activate: autoActivate,
    eval_run_id: "headless-eval-run",
    eval_status: "passed",
    eval_manifest_hash: "headless-context-integrity-manifest",
    eval_required_count: 7,
    eval_passed_count: 7,
    eval_failed_count: 0,
    activation_id: autoActivate ? "headless-activation" : null,
    activation_status: autoActivate ? "active" : null,
    activated_at: autoActivate ? "2026-07-15T04:32:09Z" : null,
    rolled_back_at: null,
    updated_at: "2026-07-15T04:32:09Z",
  };
  candidateStates = [state, ...candidateStates];
  const job: EvolutionJob = {
    id: "headless-approve",
    cwd,
    trigger: "review_eval",
    candidate_id: eventId,
    status: "succeeded",
    input_session_count: 0,
    input_trace_count: 0,
    candidate_count: 1,
    started_at: "2026-07-15T04:32:00Z",
    completed_at: "2026-07-15T04:32:09Z",
    error: null,
  };
  decisionJobs = [job, ...decisionJobs.filter((existing) => existing.id !== job.id)];
  eventsByJob.set(job.id, [
    eventFor(job.id, "review", "completed", "人工批准完成", 0),
    eventFor(job.id, "stage", "completed", "候选 revision 已冻结，live target 未改变", 1),
    eventFor(job.id, "eval", "completed", "激活安全 Evals 全部通过", 2),
    ...(autoActivate ? [eventFor(job.id, "activation", "completed", "Eval 通过后已激活，下一次 Agent 调用生效", 3)] : []),
  ]);
  return state;
}

function eventFor(jobId: string, stage: string, status: string, title: string, index: number): EvolutionJobEvent {
  return {
    id: `${jobId}-${index}`,
    cwd,
    job_id: jobId,
    candidate_id: jobId === "headless-approve" ? memoryCandidate.id : preferenceCandidate.id,
    stage,
    status,
    title,
    detail_json: JSON.stringify({ schema_version: 1, target: stage === "activation" ? "memory" : undefined }),
    created_at: `2026-07-15T04:3${jobId === "headless-approve" ? 2 : 1}:0${index}.000Z`,
  };
}

mockWindows("main");
mockIPC((command, args) => {
  const payload = (args ?? {}) as Record<string, unknown>;
  switch (command) {
    case "list_sessions":
      return [session];
    case "list_quick_sessions":
      return [];
    case "list_learning_events":
      return candidates.map((candidate) => ({ ...candidate }));
    case "read_project_memory":
      return {
        path: `${cwd}/.codefactory/memory.md`,
        content: candidates.some((candidate) => candidate.id === memoryCandidate.id && candidate.status === "accepted")
          ? `<!-- codefactory-learning-event:${memoryCandidate.id} -->`
          : "",
        exists: candidates.some((candidate) => candidate.id === memoryCandidate.id && candidate.status === "accepted"),
      };
    case "get_effective_user_preference":
      return { cwd: "_global_", key: "response_language", value: "zh-CN" };
    case "list_evolution_jobs":
      return [...decisionJobs, analysisJob];
    case "list_evolution_decision_jobs":
      return decisionJobs;
    case "get_evolution_job": {
      const job = [...decisionJobs, analysisJob].find((candidate) => candidate.id === payload.jobId);
      if (!job) throw new Error("job not found");
      return job;
    }
    case "list_evolution_job_events":
      return eventsByJob.get(String(payload.jobId)) ?? [];
    case "list_evolution_candidate_states":
      return candidateStates.map((candidate) => ({ ...candidate }));
    case "list_evolution_eval_case_results":
      return String(payload.runId) === "headless-eval-run" ? evalCases : [];
    case "approve_learning_event":
      return approve(String(payload.eventId), Boolean(payload.autoActivate));
    case "rollback_evolution_activation": {
      candidateStates = candidateStates.map((candidate) => candidate.activation_id === payload.activationId
        ? { ...candidate, state: "rolled_back", activation_status: "rolled_back", rolled_back_at: "2026-07-15T04:33:00Z" }
        : candidate);
      return candidateStates.find((candidate) => candidate.activation_id === payload.activationId) ?? null;
    }
    case "reject_learning_event":
      reject(String(payload.eventId));
      return {};
    case "mine_cross_session_patterns":
      return [];
    default:
      return null;
  }
}, { shouldMockEvents: true });

const session: Session = {
  id: "headless-session",
  title: "锁屏验收",
  cwd,
  model_id: "acceptance",
  created_at: 1,
  updated_at: 2,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};

const { useChatStore } = await import("../stores/chat");
useChatStore.setState({ sessions: [session] });
const { EvolutionWorkbenchPage } = await import("../pages/Evolution/EvolutionWorkbenchPage");

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <EvolutionWorkbenchPage onBack={() => {}} initialCwd={cwd} />
  </React.StrictMode>,
);
