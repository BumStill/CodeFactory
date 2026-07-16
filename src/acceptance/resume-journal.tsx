// SPDX-License-Identifier: Apache-2.0
// Lock-safe browser acceptance entry for the resume-journal surface. Mounts
// the REAL TaskDashboard against bounded Tauri mock IPC and replays a real
// `resume_summary` event through the mocked event system — no Rust backend,
// no git, no unlocked desktop. Driven by scripts/verify-resume-journal-headless.mjs.

import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";

import "../styles/globals.css";
import type { TaskRun } from "../lib/tauri";
import type { ResumeReport } from "../stores/tasks";

const sessionId = "headless-session";
const cwd = "/lock-safe-resume-fixture";

function fixtureTask(id: string, title: string, status: string): TaskRun {
  return {
    id,
    session_id: sessionId,
    title,
    description: `${title} 的任务描述`,
    status: status as TaskRun["status"],
    cwd,
    parent_task_id: null,
    sub_session_id: null,
    created_at: "2026-07-16T04:00:00Z",
    started_at: status === "completed" ? "2026-07-16T04:01:00Z" : null,
    completed_at: status === "completed" ? "2026-07-16T04:05:00Z" : null,
    result:
      status === "completed"
        ? JSON.stringify({ summary: `${title} 已完成`, files_changed: [], tool_calls_count: 3 })
        : null,
    error: null,
    attempt_count: 1,
    verification_results: null,
    task_context_json: null,
    acceptance_criteria_json: null,
    spec_req_id: null,
    spec_title: null,
  } as unknown as TaskRun;
}

// 3 restored-from-cache (completed), 2 invalidated re-runs (pending), 1
// recovered orphan (pending) — the exact mix the design doc's test matrix asks
// the headless gate to prove.
const tasks: TaskRun[] = [
  fixtureTask("t-restore-1", "解析配置文件", "completed"),
  fixtureTask("t-restore-2", "生成数据模型", "completed"),
  fixtureTask("t-restore-3", "编写单元测试", "completed"),
  fixtureTask("t-input-changed", "实现导出功能", "pending"),
  fixtureTask("t-reverted", "更新文档", "pending"),
  fixtureTask("t-orphan", "集成回归验证", "pending"),
];

const report: ResumeReport = {
  restored: [
    { task_id: "t-restore-1", title: "解析配置文件", key_short: "a1b2c3d4e5f6" },
    { task_id: "t-restore-2", title: "生成数据模型", key_short: "0f9e8d7c6b5a" },
    { task_id: "t-restore-3", title: "编写单元测试", key_short: "123456abcdef" },
  ],
  invalidated: [
    { task_id: "t-input-changed", title: "实现导出功能", reason: "input_changed" },
    { task_id: "t-reverted", title: "更新文档", reason: "checkpoint_reverted" },
  ],
  recovered: [{ task_id: "t-orphan", title: "集成回归验证", outcome: "reset" }],
};

mockWindows("main");
mockIPC(
  (command) => {
    switch (command) {
      case "list_tasks":
        return tasks.map((t) => ({ ...t }));
      case "get_task_dependencies":
        return [];
      case "list_sessions":
        return [];
      default:
        return null;
    }
  },
  { shouldMockEvents: true },
);

const { TaskDashboard } = await import("../components/TaskDashboard");

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <TaskDashboard sessionId={sessionId} cwd={cwd} onClose={() => {}} />
  </React.StrictMode>,
);

// Replay the scheduler's resume_summary through the REAL event path (the
// store's listen() registration), exactly as the Rust side emits it. Delayed a
// tick so the dashboard has subscribed.
setTimeout(() => {
  void emit(`resume_summary:${sessionId}`, report);
}, 300);
