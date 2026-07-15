// SPDX-License-Identifier: Apache-2.0
// Lock-safe browser acceptance entry. This HTML is not part of the production
// Vite bundle; it mounts the real workbench against bounded Tauri mock IPC.

import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import "../styles/globals.css";
import type { LearningEvent } from "../stores/learning";
import type { EvolutionJob, EvolutionJobEvent } from "../stores/evolution";
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

function decide(eventId: string, status: "accepted" | "rejected") {
  candidates = candidates.map((candidate) => candidate.id === eventId
    ? { ...candidate, status, decided_at: new Date().toISOString() }
    : candidate);
  const accepted = status === "accepted";
  const id = accepted ? "headless-accept" : "headless-reject";
  const job: EvolutionJob = {
    id,
    cwd,
    trigger: accepted ? "review_accept" : "review_reject",
    candidate_id: eventId,
    status: "succeeded",
    input_session_count: 0,
    input_trace_count: 0,
    candidate_count: accepted ? 1 : 0,
    started_at: accepted ? "2026-07-15T04:32:00Z" : "2026-07-15T04:31:00Z",
    completed_at: accepted ? "2026-07-15T04:32:01Z" : "2026-07-15T04:31:01Z",
    error: null,
  };
  decisionJobs = [job, ...decisionJobs.filter((existing) => existing.id !== id)];
  eventsByJob.set(id, accepted ? [
    eventFor(id, "review", "completed", "人工审核通过，准备物化", 0),
    eventFor(id, "materialize", "started", "开始应用候选", 1),
    eventFor(id, "materialize", "completed", "候选已物化并生效", 2),
    eventFor(id, "job", "completed", "审核与物化完成", 3),
  ] : [
    eventFor(id, "review", "completed", "人工已拒绝候选", 0),
    eventFor(id, "job", "completed", "拒绝决定已保存", 1),
  ]);
}

function eventFor(jobId: string, stage: string, status: string, title: string, index: number): EvolutionJobEvent {
  return {
    id: `${jobId}-${index}`,
    cwd,
    job_id: jobId,
    candidate_id: jobId === "headless-accept" ? memoryCandidate.id : preferenceCandidate.id,
    stage,
    status,
    title,
    detail_json: JSON.stringify({ schema_version: 1, target: stage === "materialize" ? "memory" : undefined }),
    created_at: `2026-07-15T04:3${jobId === "headless-accept" ? 2 : 1}:0${index}.000Z`,
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
    case "accept_learning_event":
      decide(String(payload.eventId), "accepted");
      return {};
    case "reject_learning_event":
      decide(String(payload.eventId), "rejected");
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
useChatStore.setState({ sessions: [session], quickSessions: [] });
const { EvolutionWorkbenchPage } = await import("../pages/Evolution/EvolutionWorkbenchPage");

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <EvolutionWorkbenchPage onBack={() => {}} initialCwd={cwd} />
  </React.StrictMode>,
);
